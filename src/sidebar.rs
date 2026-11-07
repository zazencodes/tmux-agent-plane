//! A narrow, always-on agent list in a pane down the side of a window.

use crate::agents::{ProcessTable, RawState, ScanContext};
use crate::commands;
use crate::index::{self, Agent, Index, Kind, RecentEvent, State, WindowView};
use crate::lifecycle;
use crate::navigation::{CLIENT_FORMAT, Client, guard_destination, parse_client};
use crate::tmux;
use crate::ui::{
    LAVENDER, NO_AGENTS, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, age, colour, daemon_down,
    fill_viewport, group_colour, hints, icon, marked, message, name_style, panel, rank,
    search_matches, state_style, task, truncate,
};
use anyhow::{Context, Result, bail};
use nucleo_matcher::Matcher;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame, Terminal};
use std::collections::{BTreeMap, HashSet};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

pub(crate) const WIDTH: &str = "38";
/// How often a sidebar reads the index. Reading a file spawns nothing; the daemon rewrites it
/// once a second.
const REFRESH: Duration = Duration::from_millis(500);
/// Pane option marking a sidebar pane, so the toggle finds it.
const MARK: &str = "@agent-plane-sidebar";
/// Global option set while the sidebar is open, in every window of every session.
const OPEN: &str = "@agent-plane-sidebar-open";

/// Where a list's keys come from: a sidebar knows its own pane, the popup is told its client.
#[derive(Clone, Copy)]
pub(crate) enum Origin<'a> {
    Sidebar(&'a str),
    Popup(&'a str),
}

impl Origin<'_> {
    fn is_sidebar(self, pane: &str) -> bool {
        matches!(self, Origin::Sidebar(own) if own == pane)
    }
}

/// The sidebar key: close the sidebar everywhere when it is open, wherever you are; otherwise
/// open it everywhere and move into this window's.
pub fn toggle(window: &str) -> Result<()> {
    if is_open()? {
        return close();
    }
    enter(window)
}

/// The in key: move into this window's sidebar, opening it everywhere if it is closed. From
/// inside the sidebar this changes nothing.
pub fn enter(window: &str) -> Result<()> {
    if !is_open()? {
        tmux::run(&["set-option", "-g", OPEN, "1"])?;
        add_everywhere()?;
    }
    let pane = add(window)?;
    tmux::run(&["select-pane", "-t", &pane]).map(drop)
}

/// Run by the plugin's hook when a window is created: add the sidebar to it while open, so its
/// agent has its final width before anyone looks at it.
pub fn follow(window: &str) -> Result<()> {
    if !is_open()? {
        return Ok(());
    }
    match add(window) {
        Ok(_) => Ok(()),
        Err(error) => {
            // A window can close as soon as it opens, and a failed hook would open tmux's view
            // mode over the pane on screen. Any other failure is real.
            let windows = tmux::run(&["list-windows", "-a", "-F", "#{window_id}"])?;
            if windows.lines().any(|id| id == window) {
                return Err(error);
            }
            Ok(())
        }
    }
}

fn is_open() -> Result<bool> {
    Ok(tmux::run(&["show-option", "-gqv", OPEN])?.trim() == "1")
}

fn add_everywhere() -> Result<()> {
    let windows = tmux::run(&["list-windows", "-a", "-F", "#{window_id}"])?;
    let mut done = Vec::new();
    // A window linked into several sessions is listed once per session.
    for window in windows.lines() {
        if !done.contains(&window) {
            add(window)?;
            done.push(window);
        }
    }
    Ok(())
}

/// The window's sidebar pane, added without moving you when the window has none.
fn add(window: &str) -> Result<String> {
    // Hooks fire in bursts (a jump changes session and window at once); one at a time, so a
    // window never gets two sidebars.
    let _lock = Lock::take()?;
    let panes = tmux::run(&[
        "list-panes",
        "-t",
        window,
        "-F",
        &format!("#{{pane_id}} #{{{MARK}}}"),
    ])?;
    if let Some(pane) = panes.lines().find_map(|line| line.strip_suffix(" 1")) {
        return Ok(pane.to_string());
    }
    let pane = tmux::run(&[
        "split-window",
        "-h",
        "-b",
        "-f",
        "-d",
        "-l",
        WIDTH,
        "-t",
        window,
        "-P",
        "-F",
        "#{pane_id}",
        "tmux-agent-plane sidebar",
    ])?;
    let pane = pane.trim().to_string();
    tmux::run(&["set-option", "-p", "-t", &pane, MARK, "1"])?;
    Ok(pane)
}

/// Close the sidebar in every window.
fn close() -> Result<()> {
    tmux::run(&["set-option", "-gu", OPEN])?;
    let panes = tmux::run(&[
        "list-panes",
        "-a",
        "-F",
        &format!("#{{pane_id}} #{{{MARK}}}"),
    ])?;
    let mut panes: Vec<&str> = panes
        .lines()
        .filter_map(|line| line.strip_suffix(" 1"))
        .collect();
    // From `q`, this process runs in one of them: kill that pane last, or it dies halfway.
    let own = std::env::var("TMUX_PANE").ok();
    panes.sort_by_key(|pane| Some(*pane) == own.as_deref());
    for pane in panes {
        tmux::run(&["kill-pane", "-t", pane])?;
    }
    Ok(())
}

/// An exclusive lock on a file in the state directory, released on drop.
struct Lock(std::fs::File);

impl Lock {
    fn take() -> Result<Lock> {
        let dir = index::state_dir()?;
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::File::create(dir.join("sidebar.lock"))?;
        // SAFETY: flock on a file descriptor we own.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Lock(file))
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: as above.
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}

pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    ratatui::restore();
    result?;
    // `q` closes the sidebar everywhere, this pane included.
    close()
}

#[derive(Default)]
pub(crate) struct Navigation {
    /// `d`: the state rows must be in.
    pub(crate) filter: Option<State>,
    /// `f`: the text filter, which hides rows that do not match.
    pub(crate) query: String,
    pub(crate) pane: Option<String>,
    /// The text filter while it is being typed.
    pub(crate) search: Option<SearchEdit>,
    /// `/`: the Vim-style search, which moves the selection and hides nothing.
    pub(crate) pattern: String,
    /// The search while it is being typed.
    pub(crate) jump: Option<SearchEdit>,
    pub(crate) order: Vec<String>,
}

/// What Esc restores: the text and selection from before the edit.
pub(crate) struct SearchEdit {
    query: String,
    pane: Option<String>,
}

/// Esc, or Ctrl-C.
fn cancels(key: KeyEvent) -> bool {
    key.code == KeyCode::Esc
        || (key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL))
}

/// Whether `/` finds this row: the text shown on it.
fn jump_matches(agent: &Agent, pattern: &str) -> bool {
    !search_matches(&agent.name, pattern).is_empty()
        || !search_matches(task(agent).0, pattern).is_empty()
}

pub(crate) struct Rename {
    pane: String,
    pub(crate) name: String,
}

fn kill_accepts(key: KeyEvent) -> bool {
    key.modifiers.is_empty() && matches!(key.code, KeyCode::Enter | KeyCode::Char('y'))
}

pub(crate) struct KillConfirmation {
    agent: Agent,
    codex_pid: Option<i32>,
    client: String,
    client_pid: String,
    client_session: String,
    client_window: String,
    ends_sessions: Vec<String>,
}

/// The kinds `c` offers. Antigravity needs a task to start, and this picker starts agents empty.
const NEW_AGENT_KINDS: [Kind; 5] = [
    Kind::Claude,
    Kind::Codex,
    Kind::Pi,
    Kind::Opencode,
    Kind::Grok,
];

/// The project is pinned when `c` is pressed; rows moving underneath cannot retarget it.
pub(crate) struct NewAgent {
    session: String,
    cwd: String,
    selected: usize,
}

/// Changes in task/state/turn or native identity require a new deliberate confirmation.
fn same_action_target(before: &Agent, current: &Agent) -> bool {
    before.pane == current.pane
        && before.window == current.window
        && before.session == current.session
        && before.kind == current.kind
        && before.name == current.name
        && before.session_id == current.session_id
        && before.completion_epoch == current.completion_epoch
        && before.state == current.state
        && before.state_since == current.state_since
        && before.completions == current.completions
        && before.reported_completions == current.reported_completions
        && before.title == current.title
        && before.detail == current.detail
}

/// Read the native signal again at confirmation, rather than trusting a one-tick-old index.
/// Codex has no session id in its title signal, so pin its unique live process PID too.
fn action_identity(agent: &Agent) -> Result<Option<i32>> {
    let panes = tmux::list_panes()?;
    let pane = panes
        .iter()
        .find(|p| p.id == agent.pane && p.window == agent.window && p.session == agent.session)
        .context("agent exited; action cancelled")?;
    if pane.name != agent.name
        && !(pane.name.is_empty()
            && agent.name
                == format!(
                    "{}-{}",
                    agent.kind.as_str(),
                    pane.id.trim_start_matches('%')
                ))
    {
        bail!("agent renamed; refresh and confirm again");
    }
    let processes = ProcessTable::load()?;
    let observations = (agent.kind.adapter().scan)(&ScanContext {
        panes: &panes,
        processes: &processes,
    })?;
    let observation = observations
        .iter()
        .find(|observation| observation.pane_id == agent.pane)
        .context("agent exited or changed; refresh and confirm again")?;
    let raw_state = match agent.state {
        State::Working => RawState::Working,
        State::Blocked => RawState::Blocked,
        State::Done | State::Idle => RawState::Idle,
    };
    if observation.state != raw_state
        || observation.session_id != agent.session_id
        || observation.completion_epoch != agent.completion_epoch
        || observation.completed_turns != agent.reported_completions
        || observation.detail != agent.detail
        || (!observation.preserve_title && observation.title != agent.title)
    {
        bail!("agent state/identity changed; refresh and confirm again");
    }
    if agent.kind == index::Kind::Codex {
        let pid = crate::agents::codex::agent_pid(&processes, pane.pid)
            .context("agent process changed; refresh and confirm again")?;
        Ok(Some(pid))
    } else {
        if agent.session_id.is_none() {
            bail!("agent has no native identity; action cancelled");
        }
        Ok(None)
    }
}

/// Include the layout hook closing the lone sidebar, and every session linking this window.
/// Refuse to remove the session the client will still be in, even indirectly. A sidebar moves to
/// the next row before it kills, so `leaving` says the client gets out first.
fn ending_sessions(agent: &Agent, client: &Client, leaving: bool) -> Result<Vec<String>> {
    let panes = tmux::run(&[
        "list-panes",
        "-t",
        &agent.window,
        "-F",
        "#{pane_id}\x1f#{@agent-plane-sidebar}",
    ])?;
    if !window_will_end(&agent.pane, &panes)? {
        return Ok(Vec::new());
    }
    let windows = tmux::run(&[
        "list-windows",
        "-a",
        "-F",
        "#{session_id}\x1f#{session_name}\x1f#{window_id}",
    ])?;
    let stays_in = (!leaving).then_some(client.session.as_str());
    session_endings(&agent.window, stays_in, &windows)
}

fn window_will_end(target: &str, panes: &str) -> Result<bool> {
    let mut found = false;
    let mut remaining = Vec::new();
    for line in panes.lines() {
        let (pane, sidebar) = line
            .split_once('\x1f')
            .context("unexpected pane context while checking kill")?;
        if pane == target {
            found = true;
        } else {
            remaining.push(sidebar == "1");
        }
    }
    if !found {
        bail!("agent exited; kill cancelled");
    }
    Ok(remaining.is_empty() || remaining == [true])
}

fn session_endings(window: &str, stays_in: Option<&str>, windows: &str) -> Result<Vec<String>> {
    let mut sessions: BTreeMap<String, (String, HashSet<String>)> = BTreeMap::new();
    for line in windows.lines() {
        let fields: Vec<_> = line.split('\x1f').collect();
        if fields.len() != 3 {
            bail!("unexpected window context: {line:?}");
        }
        let (name, windows) = sessions
            .entry(fields[0].into())
            .or_insert_with(|| (fields[1].into(), HashSet::new()));
        if name != fields[1] {
            bail!("session identity changed while checking kill");
        }
        windows.insert(fields[2].into());
    }
    let mut ending = Vec::new();
    for (id, (name, windows)) in sessions {
        if windows.len() == 1 && windows.contains(window) {
            if stays_in == Some(id.as_str()) {
                bail!("would end your attached session; kill refused");
            }
            ending.push(name);
        }
    }
    ending.sort();
    Ok(ending)
}

/// The agent list and its actions: the sidebar's own state, and the picker popup's model.
#[derive(Default)]
pub(crate) struct Sidebar {
    // Auto-naming setup is local UI state; it never performs periodic work.
    pub(crate) auto_name: crate::autoname_setup::Setup,
    pub(crate) help: crate::sidebar_help::Help,
    pub(crate) all_agents: Vec<Agent>,
    pub(crate) agents: Vec<Agent>,
    pub(crate) navigation: Navigation,
    matcher: Matcher,
    /// The picker starts on the top row; a sidebar starts on what wants attention.
    pub(crate) opens_on_top: bool,
    /// `c` took the client to its new agent, so a popup has nothing left to show.
    pub(crate) left: bool,
    recent_event: Option<RecentEvent>,
    pub(crate) list: ListState,
    pub(crate) error: Option<String>,
    /// Enter asked for a daemon restart, which runs once the screen says so.
    pub(crate) restarting: bool,
    /// Why the last restart failed, until the daemon is back.
    pub(crate) restart_failure: Option<String>,
    pub(crate) editing: Option<Rename>,
    pub(crate) notice: Option<String>,
    killing: Option<KillConfirmation>,
    new_agent: Option<NewAgent>,
    /// Hide successfully closed panes until the daemon has removed them from the index.
    closed: HashSet<String>,
    /// A client is viewing this sidebar with the keyboard in it: its selection is theirs.
    steering: bool,
    /// The agent active in this sidebar's window, shown in bold. Known while the window is hidden,
    /// so it is right the moment a client arrives.
    in_use: Option<String>,
    /// When a key last reached this sidebar, which proves the keyboard is here.
    pressed_at: u64,
    /// The last key was the first `g` of `gg`.
    pending_g: bool,
}

impl Navigation {
    /// Only a deliberate d skips categories. Refreshing must never change the filter.
    fn cycle_filter(&mut self, matches: &[Agent]) {
        let cycle = [
            None,
            Some(State::Blocked),
            Some(State::Done),
            Some(State::Working),
            Some(State::Idle),
        ];
        let current = cycle
            .iter()
            .position(|state| *state == self.filter)
            .unwrap();
        self.filter = (1..=cycle.len())
            .map(|step| cycle[(current + step) % cycle.len()])
            .find(|state| state.is_none() || matches.iter().any(|a| Some(a.state) == *state))
            .unwrap(); // All is always available, including an empty index/query.
    }
}

impl Sidebar {
    /// Highlight the agent in use. A client steering this sidebar owns its selection, which
    /// starts from the last agent used. A window with an agent active needs no fresh data on
    /// arrival: its active pane cannot change while it is hidden.
    fn follow(&mut self, window: Option<&WindowView>, last_agent: Option<&str>, own: &str) {
        let in_use = window
            .map(|w| w.active_pane.as_str())
            .filter(|active| *active != own && self.all_agents.iter().any(|a| a.pane == *active));
        self.in_use = in_use.map(str::to_string);
        let steering = window.is_some_and(|w| w.viewed && w.active_pane == own);
        let target = if in_use.is_some() {
            in_use
        } else if steering && self.steering {
            None
        } else {
            last_agent
        };
        self.steering = steering;
        let position = target.and_then(|pane| self.agents.iter().position(|a| a.pane == pane));
        if position.is_some() {
            self.select(position);
        }
    }

    /// A key proves the client is in this sidebar, ahead of the daemon seeing it there.
    pub(crate) fn pressed(&mut self, now: u64) {
        self.pressed_at = now;
        self.steering = true;
    }

    /// This sidebar is about to move the client: Enter lands in the agent, a preview keeps the
    /// keyboard here.
    fn switching(&mut self, focus: bool) {
        self.in_use = self
            .selected()
            .filter(|_| focus)
            .map(|agent| agent.pane.clone());
    }

    pub(crate) fn refresh(&mut self, origin: Origin<'_>) {
        match index::read() {
            Ok(index) => self.apply(index, origin),
            Err(e) => {
                self.all_agents.clear();
                self.recent_event = None;
                self.error = Some(e.to_string());
                self.rebuild();
            }
        }
    }

    fn apply(&mut self, index: Index, origin: Origin<'_>) {
        self.closed
            .retain(|pane| index.agents.iter().any(|a| &a.pane == pane));
        self.all_agents = index
            .agents
            .into_iter()
            .filter(|a| !self.closed.contains(&a.pane))
            .collect();
        self.recent_event = index.recent_event;
        self.error = None;
        self.restart_failure = None;
        self.navigation.order = index.order;
        self.rebuild();
        // An index stamped before the last key cannot know the keyboard is here.
        if let Origin::Sidebar(own) = origin
            && index.updated_at > self.pressed_at
        {
            let window = index
                .windows
                .iter()
                .find(|w| w.sidebar.as_deref() == Some(own));
            self.follow(window, index.last_agent.as_deref(), own);
        }
    }

    pub(crate) fn rebuild(&mut self) {
        let mut listed: Vec<Agent> = self
            .navigation
            .order
            .iter()
            .filter_map(|pane| self.all_agents.iter().find(|a| &a.pane == pane))
            .filter(|a| self.navigation.filter.is_none_or(|state| a.state == state))
            .cloned()
            .collect();
        // Sessions by name: a stable sort keeps the list's order within each.
        listed.sort_by(|a, b| a.session.cmp(&b.session));
        self.agents = rank(&mut self.matcher, &self.navigation.query, &listed);
        let selected = self
            .navigation
            .pane
            .as_ref()
            .and_then(|pane| self.agents.iter().position(|a| &a.pane == pane))
            // Search results put the best match on the top row.
            .or_else(|| {
                [State::Blocked, State::Done]
                    .into_iter()
                    .filter(|_| !self.opens_on_top && self.navigation.query.is_empty())
                    .find_map(|state| self.agents.iter().position(|a| a.state == state))
            })
            .or_else(|| (!self.agents.is_empty()).then_some(0));
        self.select(selected);
        if self
            .editing
            .as_ref()
            .is_some_and(|edit| !self.agents.iter().any(|a| a.pane == edit.pane))
        {
            self.editing = None;
            self.notice = Some("agent left list; rename cancelled".into());
        }
    }

    fn select(&mut self, selected: Option<usize>) {
        self.list.select(selected);
        self.navigation.pane = self.selected().map(|a| a.pane.clone());
    }

    pub(crate) fn selected(&self) -> Option<&Agent> {
        self.list.selected().and_then(|i| self.agents.get(i))
    }

    /// Rename is pinned to the original pane, even if an agent exits during editing.
    fn edit_key(&mut self, key: KeyEvent) {
        let Some(edit) = self.editing.as_mut() else {
            return;
        };
        self.notice = None;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.editing = None,
            KeyCode::Enter => {
                let pane = edit.pane.clone();
                let name = edit.name.clone();
                match commands::set_name(&pane, &name) {
                    Ok(_) => {
                        if let Some(agent) = self.all_agents.iter_mut().find(|a| a.pane == pane) {
                            agent.name = name;
                        }
                        self.editing = None;
                        self.rebuild();
                    }
                    Err(e) => self.notice = Some(e.to_string()),
                }
            }
            KeyCode::Char('w') if ctrl => delete_word(&mut edit.name),
            KeyCode::Char('u') if ctrl => edit.name.clear(),
            KeyCode::Backspace => {
                edit.name.pop();
            }
            KeyCode::Char(' ') => edit.name.push('-'),
            KeyCode::Char(c) if !ctrl => edit.name.push(c),
            _ => {}
        }
    }

    fn search_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let before = self.navigation.query.clone();
        match key.code {
            _ if cancels(key) => {
                let before = self.navigation.search.take().unwrap();
                self.navigation.query = before.query;
                self.navigation.pane = before.pane;
                self.rebuild();
                return;
            }
            // `/` again freezes the search: its matches stay, and the keys are commands again.
            KeyCode::Char('/') if key.modifiers.is_empty() => {
                self.navigation.search = None;
                return;
            }
            KeyCode::Down => {
                self.step(1);
                return;
            }
            KeyCode::Up => {
                self.step(-1);
                return;
            }
            KeyCode::Char('j') if ctrl => {
                self.step(1);
                return;
            }
            KeyCode::Char('k') if ctrl => {
                self.step(-1);
                return;
            }
            KeyCode::Char('w') if ctrl => delete_word(&mut self.navigation.query),
            KeyCode::Char('u') if ctrl => self.navigation.query.clear(),
            KeyCode::Backspace => {
                self.navigation.query.pop();
            }
            KeyCode::Char(c) if !ctrl => self.navigation.query.push(c),
            _ => return,
        }
        // Results select the best match after every edit.
        if self.navigation.query != before {
            self.navigation.pane = None;
            self.rebuild();
        }
    }

    /// Vim's incremental search: every edit moves the selection to the first match after the row
    /// the search started on, and Enter keeps it for `n` and `N`.
    fn jump_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            _ if cancels(key) => {
                let before = self.navigation.jump.take().unwrap();
                self.navigation.pattern = before.query;
                self.navigation.pane = before.pane;
                self.rebuild();
                return;
            }
            KeyCode::Enter => {
                self.navigation.jump = None;
                let pattern = &self.navigation.pattern;
                if !self.agents.iter().any(|a| jump_matches(a, pattern)) {
                    self.navigation.pattern.clear();
                }
                return;
            }
            KeyCode::Char('w') if ctrl => delete_word(&mut self.navigation.pattern),
            KeyCode::Char('u') if ctrl => self.navigation.pattern.clear(),
            KeyCode::Backspace => {
                self.navigation.pattern.pop();
            }
            KeyCode::Char(c) if !ctrl => self.navigation.pattern.push(c),
            _ => return,
        }
        self.navigation.pane = self.navigation.jump.as_ref().unwrap().pane.clone();
        self.rebuild();
        self.next_match(1);
    }

    /// Select the next row matching the search, wrapping; the current row is the last one tried.
    fn next_match(&mut self, delta: isize) {
        if self.navigation.pattern.is_empty() || self.agents.is_empty() {
            return;
        }
        let pattern = &self.navigation.pattern;
        let len = self.agents.len() as isize;
        let from = self.list.selected().unwrap_or(0) as isize;
        let found = (1..=len)
            .map(|step| (from + delta * step).rem_euclid(len) as usize)
            .find(|row| jump_matches(&self.agents[*row], pattern));
        match found {
            Some(row) => self.select(Some(row)),
            None => self.notice = Some("no match".into()),
        }
    }

    fn start_jump(&mut self) {
        self.navigation.jump = Some(SearchEdit {
            query: std::mem::take(&mut self.navigation.pattern),
            pane: self.navigation.pane.clone(),
        });
    }

    /// The whole list, with no search or filter for Esc or Ctrl-C to clear.
    pub(crate) fn unfiltered(&self) -> bool {
        !self.typing_query()
            && self.navigation.pattern.is_empty()
            && self.navigation.query.is_empty()
            && self.navigation.filter.is_none()
    }

    /// Letters are query text, for the filter or the search.
    pub(crate) fn typing_query(&self) -> bool {
        self.navigation.search.is_some() || self.navigation.jump.is_some()
    }

    pub(crate) fn start_search(&mut self) {
        self.navigation.search = Some(SearchEdit {
            query: self.navigation.query.clone(),
            pane: self.navigation.pane.clone(),
        });
    }

    /// Enter: the agent to open, if any. A search that opened something is finished, and so is a
    /// filter being typed. A frozen filter stays, since keeping it was deliberate.
    pub(crate) fn open_selected(&mut self) -> Option<Agent> {
        let agent = self.selected()?.clone();
        self.navigation.jump = None;
        self.navigation.pattern.clear();
        if self.navigation.search.take().is_some() {
            self.navigation.query.clear();
            self.rebuild();
        }
        Some(agent)
    }

    fn step(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let len = self.agents.len() as isize;
        let i = self.list.selected().unwrap_or(0) as isize;
        self.select(Some((i + delta).rem_euclid(len) as usize));
    }

    /// Confirmation consumes every key. Enter/y confirm; all others only cancel.
    fn kill_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        if let Some(confirmation) = self.killing.take() {
            self.notice = None;
            if kill_accepts(key) {
                match self.confirm_kill(&confirmation, origin) {
                    Ok(()) => {
                        let next = self
                            .next_row(&confirmation.agent.pane)
                            .map(|a| a.pane.clone());
                        self.closed.insert(confirmation.agent.pane.clone());
                        self.all_agents
                            .retain(|a| a.pane != confirmation.agent.pane);
                        self.navigation.pane = next;
                        self.rebuild();
                    }
                    Err(error) => self.notice = Some(error.to_string()),
                }
            }
            return true;
        }
        if key.code != KeyCode::Char('x') || self.editing.is_some() || self.typing_query() {
            return false;
        }
        self.notice = None;
        let result = (|| {
            let Some(selected) = self
                .selected()
                .filter(|a| !origin.is_sidebar(&a.pane))
                .cloned()
            else {
                return Ok(None);
            };
            let index = index::read()?;
            let current = index
                .agents
                .iter()
                .find(|a| a.pane == selected.pane)
                .context("agent exited; kill cancelled")?;
            if !same_action_target(&selected, current) {
                bail!("agent changed; refresh and press x again");
            }
            let clients = clients()?;
            let client = input_client(&clients, origin)?;
            let ends_sessions = ending_sessions(current, client, self.leaves(current, origin))?;
            Ok(Some(KillConfirmation {
                agent: current.clone(),
                codex_pid: action_identity(current)?,
                client: client.name.clone(),
                client_pid: client.pid.clone(),
                client_session: client.session.clone(),
                client_window: client.window.clone(),
                ends_sessions,
            }))
        })();
        match result {
            Ok(confirmation) => self.killing = confirmation,
            Err(error) => self.notice = Some(error.to_string()),
        }
        true
    }

    fn confirm_kill(&self, confirmation: &KillConfirmation, origin: Origin<'_>) -> Result<()> {
        if origin.is_sidebar(&confirmation.agent.pane) {
            bail!("cannot kill the sidebar");
        }
        let clients = clients()?;
        let client = input_client(&clients, origin)?;
        if client.name != confirmation.client
            || client.pid != confirmation.client_pid
            || client.session != confirmation.client_session
            || client.window != confirmation.client_window
        {
            bail!("confirmation client changed; press x again");
        }
        let index = index::read()?;
        let current = index
            .agents
            .iter()
            .find(|a| a.pane == confirmation.agent.pane)
            .context("agent exited; kill cancelled")?;
        if !same_action_target(&confirmation.agent, current) {
            bail!("agent changed; press x again");
        }
        if ending_sessions(current, client, self.leaves(current, origin))?
            != confirmation.ends_sessions
        {
            bail!("session/window changed; press x again");
        }
        // Read the native signal last, after the topology queries. Pane IDs are
        // server-lifetime identities, so an exited target cannot resolve to its neighbour.
        if action_identity(current)? != confirmation.codex_pid {
            bail!("agent changed; press x again");
        }
        // Move off first: the agent's window may close with it, and with it this sidebar, which
        // would drop your keys into whatever pane tmux shows next.
        if let (Origin::Sidebar(own), Some(next)) = (origin, self.next_row(&current.pane))
            && let Some(command) = switch_to(own, &next.session, &next.window, &next.pane, false)?
        {
            let command: Vec<_> = command.iter().map(String::as_str).collect();
            tmux::run(&command)?;
        }
        commands::kill_pane(&current.pane)
    }

    /// The row the selection moves to when this agent goes.
    fn next_row(&self, pane: &str) -> Option<&Agent> {
        let i = self.agents.iter().position(|a| a.pane == pane)?;
        self.agents
            .get(i + 1)
            .or_else(|| i.checked_sub(1).and_then(|i| self.agents.get(i)))
    }

    /// Killing this agent takes the client elsewhere first: a sidebar moves to the next row.
    fn leaves(&self, agent: &Agent, origin: Origin<'_>) -> bool {
        matches!(origin, Origin::Sidebar(_)) && self.next_row(&agent.pane).is_some()
    }

    /// While the picker is open it consumes every key: Enter starts, Esc or `c` cancels.
    fn new_agent_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        if let Some(picker) = &mut self.new_agent {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    picker.selected = (picker.selected + 1) % NEW_AGENT_KINDS.len();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    picker.selected =
                        (picker.selected + NEW_AGENT_KINDS.len() - 1) % NEW_AGENT_KINDS.len();
                }
                KeyCode::Enter => {
                    let picker = self.new_agent.take().expect("picker is open");
                    match start_new_agent(&picker, origin) {
                        Ok(()) => self.left = true,
                        Err(error) => self.notice = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Esc | KeyCode::Char('c') => self.new_agent = None,
                _ => {}
            }
            return true;
        }
        if key.code != KeyCode::Char('c')
            || !key.modifiers.is_empty()
            || self.editing.is_some()
            || self.typing_query()
            || self.killing.is_some()
        {
            return false;
        }
        self.notice = None;
        match self.selected() {
            Some(agent) => {
                self.new_agent = Some(NewAgent {
                    session: agent.session.clone(),
                    cwd: agent.cwd.clone(),
                    selected: 0,
                })
            }
            None => self.notice = Some("select an agent to choose its project".into()),
        }
        true
    }

    /// Help, setup, new agent and kill, in priority order. True when one consumed
    /// the key.
    pub(crate) fn action_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        let consumed = self.help_key(key)
            // Setup consumes its keys before navigation/kill, including Enter.
            || (self.killing.is_none()
                && self.new_agent.is_none()
                && self.editing.is_none()
                && !self.typing_query()
                && self.auto_name.key(key))
            || self.new_agent_key(key, origin)
            || self.kill_key(key, origin)
            || self.restart_key(key);
        self.pending_g &= !consumed;
        consumed
    }

    /// With the daemon down the list has no rows, so Enter has nothing else to open.
    fn restart_key(&mut self, key: KeyEvent) -> bool {
        let restart = key.code == KeyCode::Enter && self.error.is_some() && self.editing.is_none();
        self.restarting |= restart;
        restart
    }

    /// Waits for the new daemon's first index, or for the reason it could not start.
    pub(crate) fn restart_daemon(&mut self, origin: Origin<'_>) {
        self.restarting = false;
        match lifecycle::run(lifecycle::Action::Restart) {
            Ok(()) => self.refresh(origin),
            Err(error) => self.restart_failure = Some(format!("{error:#}")),
        }
    }

    fn help_key(&mut self, key: KeyEvent) -> bool {
        self.help.key(
            key,
            self.editing.is_none()
                && !self.typing_query()
                && !self.auto_name.open
                && self.new_agent.is_none(),
        )
    }

    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {
        if self.help_key(key) {
            return false;
        }
        if self.editing.is_some() {
            self.edit_key(key);
            return false;
        }
        self.notice = None;
        if self.navigation.search.is_some() {
            self.search_key(key);
            return false;
        }
        if self.navigation.jump.is_some() {
            self.jump_key(key);
            return false;
        }
        let pending_g = std::mem::take(&mut self.pending_g);
        match key.code {
            KeyCode::Char('q') => return true,
            KeyCode::Char('r') => {
                self.editing = self.selected().map(|a| Rename {
                    pane: a.pane.clone(),
                    name: a.name.clone(),
                })
            }
            KeyCode::Char('j') | KeyCode::Down => self.step(1),
            KeyCode::Char('k') | KeyCode::Up => self.step(-1),
            KeyCode::Char('g') if pending_g => self.select((!self.agents.is_empty()).then_some(0)),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Char('G') => self.select(self.agents.len().checked_sub(1)),
            KeyCode::Char('n') => self.next_match(1),
            KeyCode::Char('N') => self.next_match(-1),
            KeyCode::Char('d') => {
                let matches = rank(&mut self.matcher, &self.navigation.query, &self.all_agents);
                self.navigation.cycle_filter(&matches);
                self.rebuild();
            }
            KeyCode::Char('f') => self.start_search(),
            KeyCode::Char('/') => self.start_jump(),
            // The search first, then every filter, back to the whole list.
            _ if cancels(key) && !self.navigation.pattern.is_empty() => {
                self.navigation.pattern.clear()
            }
            _ if cancels(key) => {
                self.navigation.query.clear();
                self.navigation.filter = None;
                self.rebuild();
            }
            _ => {}
        }
        false
    }
}

pub(crate) fn clients() -> Result<Vec<Client>> {
    tmux::run(&["list-clients", "-F", CLIENT_FORMAT])?
        .lines()
        .map(parse_client)
        .collect()
}

pub(crate) fn input_client<'a>(clients: &'a [Client], origin: Origin<'_>) -> Result<&'a Client> {
    match origin {
        Origin::Sidebar(pane) => {
            let mut candidates = clients.iter().filter(|client| client.pane == pane);
            let client = candidates.next().context("no client in this sidebar")?;
            if candidates.next().is_some() {
                bail!("multiple clients in sidebar; use a separate session");
            }
            Ok(client)
        }
        Origin::Popup(name) => clients
            .iter()
            .find(|client| client.name == name)
            .context("the picker's client is no longer attached"),
    }
}

/// What a switch must check, read in one tmux command: each command costs a process.
struct SwitchContext {
    clients: Vec<Client>,
    session: String,
    sidebar: Option<String>,
    /// What the client's window had active before this sidebar.
    previous: Option<String>,
}

fn parse_switch_context(output: &str) -> Result<SwitchContext> {
    let mut clients = Vec::new();
    let mut session = None;
    let mut sidebar = None;
    let mut previous = None;
    for line in output.lines() {
        match line.split('\x1f').collect::<Vec<_>>().as_slice() {
            ["session", id] => session = Some(id.to_string()),
            ["pane", id, "1"] => sidebar = Some(id.to_string()),
            ["pane", _, _] => {}
            ["left", id, "1"] => previous = Some(id.to_string()),
            ["left", _, _] => {}
            _ => clients.push(parse_client(line)?),
        }
    }
    Ok(SwitchContext {
        clients,
        session: session.context("switch context has no session")?,
        sidebar,
        previous,
    })
}

/// Carry this sidebar to `destination`'s window with the client. tmux runs the whole sequence
/// before it redraws. Only once the client has left may the window take its previous pane back:
/// nobody is viewing it by then, so that pane gets no focus event and no seen state. A window
/// left this way shows its agent active on the next arrival, however quick.
fn carry_command(
    own: &str,
    destination: &str,
    switch: &[String],
    previous: Option<&str>,
) -> Vec<String> {
    let swap = ["swap-pane", "-s", own, "-t", destination, ";"].map(String::from);
    let mut command: Vec<_> = swap.into_iter().chain(switch.iter().cloned()).collect();
    if let Some(previous) = previous {
        command.extend([";", "select-pane", "-t", previous].map(String::from));
    }
    command
}

/// The tmux command for a preview or Enter jump, or None when the client is already there.
fn switch_command(sidebar: &Sidebar, own: &str, focus: bool) -> Result<Option<Vec<String>>> {
    let Some(agent) = sidebar.selected() else {
        return Ok(None);
    };
    switch_to(own, &agent.session, &agent.window, &agent.pane, focus)
}

/// Start the chosen agent in a new window of the pinned project and take the client there.
/// Checks the client first, so a refused move creates nothing.
fn start_new_agent(picker: &NewAgent, origin: Origin<'_>) -> Result<()> {
    let session = crate::sessions::destination(&picker.session)?;
    let clients = clients()?;
    let client = input_client(&clients, origin)?;
    guard_destination(&clients, client, &session, "")?;
    let (window, pane) =
        commands::open_agent_window(NEW_AGENT_KINDS[picker.selected], &session, &picker.cwd)?;
    let command = match origin {
        Origin::Sidebar(own) => switch_to(own, &picker.session, &window, &pane, true)?
            .context("focus always switches the client")?,
        Origin::Popup(name) => vec![
            "switch-client".to_string(),
            "-c".to_string(),
            name.to_string(),
            "-t".to_string(),
            format!("={}:{window}.{pane}", picker.session),
        ],
    };
    let command: Vec<_> = command.iter().map(String::as_str).collect();
    tmux::run(&command).map(drop)
}

fn switch_to(
    own: &str,
    session: &str,
    window: &str,
    target_pane: &str,
    focus: bool,
) -> Result<Option<Vec<String>>> {
    let window_target = format!("={session}:{window}");
    // Resolve the indexed session explicitly: a pane can be linked into several sessions.
    let context = parse_switch_context(&tmux::run(&[
        "list-clients",
        "-F",
        CLIENT_FORMAT,
        ";",
        "display-message",
        "-p",
        "-t",
        &window_target,
        "session\x1f#{session_id}",
        ";",
        "list-panes",
        "-t",
        &window_target,
        "-F",
        &format!("pane\x1f#{{pane_id}}\x1f#{{{MARK}}}"),
        ";",
        "list-panes",
        "-t",
        own,
        "-F",
        "left\x1f#{pane_id}\x1f#{pane_last}",
    ])?)?;
    let client = input_client(&context.clients, Origin::Sidebar(own))?;
    guard_destination(&context.clients, client, &context.session, window)?;
    let destination = match context.sidebar {
        Some(pane) => pane,
        None => add(&window_target)?,
    };
    // Enter lands on the agent. Preview stays in this sidebar, so neither agent keys nor seen
    // state are manufactured.
    let pane = if focus { target_pane } else { own };
    let switch = [
        "switch-client".to_string(),
        "-c".to_string(),
        client.name.clone(),
        "-t".to_string(),
        format!("{window_target}.{pane}"),
    ];
    if destination != own {
        // This pane travels with the client, so its selection and keyboard input stay continuous
        // on screen.
        return Ok(Some(carry_command(
            own,
            &destination,
            &switch,
            context.previous.as_deref(),
        )));
    }
    Ok(focus.then(|| switch.to_vec()))
}

/// The terminal, written to only when a frame differs from the one on screen. Each frame is
/// rendered offscreen first, because any draw writes to the pane, which wakes tmux.
struct Screen<'a> {
    terminal: &'a mut DefaultTerminal,
    offscreen: Terminal<TestBackend>,
    shown: Option<Buffer>,
}

impl Screen<'_> {
    fn show(&mut self, sidebar: &mut Sidebar) -> Result<()> {
        let size = self.terminal.size()?;
        self.offscreen.backend_mut().resize(size.width, size.height);
        let frame = self.offscreen.draw(|f| draw(f, sidebar))?;
        if self.shown.as_ref() != Some(frame.buffer) {
            self.shown = Some(frame.buffer.clone());
            self.terminal.draw(|f| draw(f, sidebar))?;
        }
        Ok(())
    }
}

/// Runs until `q`. Idle, this runs no tmux command: only keys and a wrong width do.
fn event_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    // This is the native sidebar process's own pane, never a caller inferred by the CLI.
    let own = std::env::var("TMUX_PANE")?;
    let origin = Origin::Sidebar(&own);
    let width = WIDTH.parse::<u16>()?;
    let size = terminal.size()?;
    let mut screen = Screen {
        terminal,
        offscreen: Terminal::new(TestBackend::new(size.width, size.height))?,
        shown: None,
    };
    let mut sidebar = Sidebar::default();
    let mut next_read = Instant::now();
    let mut check_width = true;
    loop {
        if Instant::now() >= next_read {
            // Refresh may change selection when an agent vanishes. Only input triggers preview.
            sidebar.refresh(origin);
            next_read = Instant::now() + REFRESH;
        }
        // tmux rescales panes proportionally when the window or a client resizes, and a
        // dragged border moves them: every sidebar is always exactly WIDTH columns. Asking only
        // after a resize means a window too narrow for WIDTH is asked once, not every wake.
        if std::mem::take(&mut check_width) && screen.terminal.size()?.width != width {
            tmux::run(&["resize-pane", "-t", &own, "-x", WIDTH])?;
        }
        screen.show(&mut sidebar)?;
        if !event::poll(next_read.saturating_duration_since(Instant::now()))? {
            continue;
        }
        let mut focus = false;
        let mut preview = false;
        // Apply the complete buffered burst first, so a fast jjj makes one preview switch.
        loop {
            let event = event::read()?;
            check_width |= matches!(event, Event::Resize(..));
            if let Event::Key(key) = event
                && key.kind == KeyEventKind::Press
            {
                sidebar.pressed(index::now_ms());
                // Help consumes every closing key before setup, kill or navigation.
                if sidebar.action_key(key, origin) {
                    if !event::poll(Duration::ZERO)? {
                        break;
                    }
                    continue;
                }
                let searching = sidebar.navigation.search.is_some();
                let jumping = sidebar.navigation.jump.is_some();
                let command = sidebar.editing.is_none() && !searching && !jumping;
                let before = sidebar.navigation.pane.clone();
                // Enter while typing a search keeps it, so `n` can go on from there.
                let opens = (key.code == KeyCode::Enter && !jumping) || leaves_sidebar(key);
                focus = opens && sidebar.editing.is_none() && sidebar.open_selected().is_some();
                if sidebar.key(key) {
                    return Ok(());
                }
                // Moving the selection previews it. Filter and search edits stay in this pane:
                // only freezing the filter or accepting the search previews the final selection,
                // and cancelling never switches context.
                preview |= (searching && key.code == KeyCode::Char('/'))
                    || (jumping && key.code == KeyCode::Enter)
                    || (command && sidebar.navigation.pane != before);
            }
            if focus || !event::poll(Duration::ZERO)? {
                break;
            }
        }
        if sidebar.restarting {
            screen.show(&mut sidebar)?;
            sidebar.restart_daemon(origin);
        }
        if focus || (preview && !sidebar.typing_query()) {
            match switch_command(&sidebar, &own, focus) {
                Ok(Some(command)) => {
                    sidebar.switching(focus);
                    // Draw last, so the selection moves on screen together with the window.
                    screen.show(&mut sidebar)?;
                    let command: Vec<_> = command.iter().map(String::as_str).collect();
                    if let Err(error) = tmux::run(&command) {
                        sidebar.notice = Some(error.to_string());
                    }
                }
                Ok(None) => {}
                Err(error) => sidebar.notice = Some(error.to_string()),
            }
        }
    }
}

/// Shift-Right leaves like Enter, also mid-search. The plugin's out key sends it here too.
fn leaves_sidebar(key: KeyEvent) -> bool {
    key.code == KeyCode::Right && key.modifiers == KeyModifiers::SHIFT
}

/// The filter and the search on the top line, sharing the room the state filter leaves. An idle
/// one shrinks to its key once the other has text.
fn query_fields(navigation: &Navigation, room: usize) -> Vec<Span<'static>> {
    let fields = [
        (
            "f",
            "filter",
            &navigation.query,
            navigation.search.is_some(),
        ),
        (
            "/",
            "search",
            &navigation.pattern,
            navigation.jump.is_some(),
        ),
    ];
    let active = fields
        .iter()
        .filter(|(_, _, text, editing)| *editing || !text.is_empty())
        .count();
    let share = if active == 2 {
        room / 2
    } else {
        room.saturating_sub(3)
    };
    fields
        .into_iter()
        .map(|(key, idle, text, editing)| {
            if editing {
                let tail: String = text.chars().rev().take(share.saturating_sub(5)).collect();
                let tail: String = tail.chars().rev().collect();
                Span::styled(format!("  {key} {tail}▏"), TEXT)
            } else if !text.is_empty() {
                Span::styled(truncate(&format!("  {key} {text}"), share), SUBTEXT)
            } else if active == 0 {
                Span::styled(format!("  {key} {idle}"), SUBTEXT)
            } else {
                Span::styled(format!("  {key}"), SUBTEXT)
            }
        })
        .collect()
}

/// Ctrl-W: drop trailing separators, then the word before them.
fn delete_word(name: &mut String) {
    while name.ends_with(|c: char| !c.is_alphanumeric()) {
        name.pop();
    }
    while name.ends_with(char::is_alphanumeric) {
        name.pop();
    }
}

fn draw(f: &mut Frame, sidebar: &mut Sidebar) {
    if sidebar.help.open {
        sidebar.help.draw(f, f.area());
        return;
    }
    if sidebar.auto_name.open {
        sidebar.auto_name.draw(f, f.area());
        return;
    }
    let confirmation = confirmation_footer(sidebar, f.area().width as usize);
    let footer_height = confirmation.as_ref().map_or(2, |lines| lines.len() as u16);
    let [list_area, help_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(footer_height)]).areas(f.area());
    let inner = list_area.width.saturating_sub(2) as usize;
    let block = panel(Line::from(" agents "));
    let content = block.inner(list_area);
    f.render_widget(block, list_area);
    let [mode_area, agents_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(content);
    let mode = format!(
        " d {}",
        sidebar.navigation.filter.map_or("all", State::as_str)
    );
    let room = inner.saturating_sub(mode.chars().count());
    let mut top = vec![Span::styled(mode, LAVENDER)];
    top.extend(query_fields(&sidebar.navigation, room));
    f.render_widget(Paragraph::new(Line::from(top)), mode_area);
    if let Some(e) = &sidebar.error {
        daemon_down(
            f,
            agents_area,
            ratatui::widgets::Block::default(),
            sidebar,
            e,
        );
    } else if sidebar.agents.is_empty() {
        let text = if sidebar.all_agents.is_empty() {
            NO_AGENTS
        } else {
            ["no matching agents", "d changes state · esc clears filter"]
        };
        message(
            f,
            agents_area,
            ratatui::widgets::Block::default(),
            TEXT,
            &text,
        );
    } else {
        let selected = sidebar.list.selected();
        let items = items(
            &sidebar.agents,
            &sidebar.all_agents,
            selected,
            sidebar.editing.as_ref().map(|edit| &edit.name),
            sidebar.in_use.as_deref(),
            &sidebar.navigation.pattern,
            inner,
        );
        fill_viewport(&mut sidebar.list, &items, agents_area.height);
        f.render_stateful_widget(List::new(items), agents_area, &mut sidebar.list);
    }

    let footer = confirmation.map_or_else(
        || {
            Text::from(vec![
                if sidebar.navigation.search.is_some() {
                    hints(&[("^j/^k", "browse"), ("^w ^u", "clear")])
                } else if sidebar.navigation.jump.is_some() {
                    hints(&[("type", "find a row"), ("^w ^u", "clear")])
                } else {
                    hints(&[("c", "new"), ("o", "configure")])
                },
                footer(sidebar, inner + 2, index::now_ms()),
            ])
        },
        Text::from,
    );
    f.render_widget(Paragraph::new(footer), help_area);
}

/// The prompt of whichever action is waiting on a key.
pub(crate) fn confirmation_footer(sidebar: &Sidebar, width: usize) -> Option<Vec<Line<'static>>> {
    sidebar
        .killing
        .as_ref()
        .map(|kill| kill_footer(kill, width))
        .or_else(|| {
            sidebar
                .new_agent
                .as_ref()
                .map(|picker| new_agent_footer(picker, width))
        })
}

fn new_agent_footer(picker: &NewAgent, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        truncate(&format!(" new agent in {}", picker.session), width),
        Style::default().fg(TEXT).bold(),
    )];
    lines.extend(NEW_AGENT_KINDS.iter().enumerate().map(|(row, kind)| {
        if row == picker.selected {
            Line::styled(
                truncate(&format!(" ▌ {}", kind.as_str()), width),
                Style::default().fg(TEXT).bold(),
            )
        } else {
            Line::styled(truncate(&format!("   {}", kind.as_str()), width), SUBTEXT)
        }
    }));
    lines.push(hints(&[("⏎", "start"), ("esc", "cancel")]));
    lines
}

/// Two lines keep both name and state readable in a narrow pane. Remote session
/// destruction gets a third, explicit risk line; [Y/n] always remains visible.
fn kill_footer(confirmation: &KillConfirmation, width: usize) -> Vec<Line<'static>> {
    let agent = &confirmation.agent;
    let narrow = agent.name.chars().count() + 6 > width;
    let name = if narrow {
        truncate(&agent.name, width)
    } else {
        format!(" kill {}", agent.name)
    };
    let state = format!(
        "{}{}",
        if narrow {
            format!("kill {}", agent.state.as_str())
        } else {
            format!(" ({})", agent.state.as_str())
        },
        if confirmation.ends_sessions.is_empty() {
            "? [Y/n]"
        } else {
            ""
        }
    );
    let mut lines = vec![
        Line::styled(name, Style::default().fg(TEXT).bold()),
        Line::styled(state, colour(agent.state)),
    ];
    if !confirmation.ends_sessions.is_empty() {
        let risk = truncate(
            &format!(" ends session {}", confirmation.ends_sessions.join(", ")),
            width.saturating_sub(7),
        );
        lines.push(Line::styled(
            format!("{risk}? [Y/n]"),
            colour(State::Blocked),
        ));
    }
    lines
}

/// Local failures and rename controls take priority; a shared event never hides them.
fn footer(sidebar: &Sidebar, width: usize, now: u64) -> Line<'static> {
    status_footer(sidebar, width).unwrap_or_else(|| {
        match sidebar
            .recent_event
            .as_ref()
            .filter(|event| event.is_live(now))
        {
            Some(event) => {
                let state = event.state.state();
                Line::styled(
                    truncate(
                        &format!(" {} {} {}", icon(state), event.name, state.as_str()),
                        width,
                    ),
                    colour(state),
                )
            }
            None => hints(&[("⏎", "jump"), ("x", "kill"), ("?", "keys")]),
        }
    })
}

/// What this list is in the middle of, if anything.
pub(crate) fn status_footer(sidebar: &Sidebar, width: usize) -> Option<Line<'static>> {
    Some(match (&sidebar.notice, &sidebar.editing) {
        (Some(notice), _) => Line::styled(
            truncate(&format!(" {notice}"), width),
            colour(State::Blocked),
        ),
        (None, Some(_)) => hints(&[("⏎", "save"), ("esc", "cancel"), ("^w ^u", "clear")]),
        (None, None) if sidebar.navigation.search.is_some() => {
            hints(&[("⏎", "open"), ("/", "freeze"), ("esc", "cancel")])
        }
        (None, None) if sidebar.navigation.jump.is_some() => {
            hints(&[("⏎", "keep"), ("esc", "cancel")])
        }
        (None, None) if !sidebar.navigation.pattern.is_empty() => {
            hints(&[("n N", "next/prev"), ("⏎", "open"), ("esc", "clear")])
        }
        (None, None) => return None,
    })
}

/// One item per agent, grouped under a heading per tmux session in that session's picker colour,
/// with a blank line between agents. The selected agent gets an accent bar and a background; its
/// heading does not. `all` is every agent, filtered out or not, so no search recolours a session.
fn items<'a>(
    agents: &'a [Agent],
    all: &[Agent],
    selected: Option<usize>,
    editing: Option<&String>,
    in_use: Option<&str>,
    search: &str,
    width: usize,
) -> Vec<ListItem<'a>> {
    let mut session = None;
    agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let mut lines = Vec::new();
            if i > 0 {
                lines.push(Line::default());
            }
            if session != Some(&a.session) {
                session = Some(&a.session);
                let name = truncate(&a.session, width.saturating_sub(4));
                let rule = "─".repeat(width.saturating_sub(name.chars().count() + 3));
                lines.push(Line::from(vec![
                    Span::styled(
                        format!(" {name} "),
                        Style::default().fg(group_colour(all, &a.session)).bold(),
                    ),
                    Span::styled(rule, SURFACE2),
                ]));
            }
            let is_selected = selected == Some(i);
            let renaming = editing.filter(|_| is_selected);
            let bar = Span::styled(if is_selected { "▌" } else { " " }, colour(a.state));

            let status = format!("{} {} ", a.state.as_str(), age(a.state_since));
            let name_width = width.saturating_sub(status.len() + 4);
            let name = match renaming {
                // Keep the end of a long name in view, where the cursor is.
                Some(typed) => {
                    let shown: String = typed
                        .chars()
                        .rev()
                        .take(name_width.saturating_sub(1))
                        .collect();
                    vec![Span::styled(
                        shown.chars().rev().collect::<String>() + "▏",
                        Style::default()
                            .fg(colour(State::Done))
                            .add_modifier(Modifier::UNDERLINED),
                    )]
                }
                None => marked(
                    &a.name,
                    name_width,
                    search,
                    name_style(a, in_use == Some(a.pane.as_str())),
                ),
            };
            let mut first = vec![
                bar.clone(),
                Span::styled(format!("{} ", icon(a.state)), colour(a.state)),
            ];
            first.extend(name);
            let first = Line::from(first);
            let first = right(first, Span::styled(status, state_style(a.state)), width);

            // The window and pane: the session is in the heading.
            let place = a
                .location
                .strip_prefix(&format!("{}:", a.session))
                .unwrap_or(&a.location);
            let (task, task_style) = task(a);
            let task_width = width.saturating_sub(place.chars().count() + 6);
            let mut second = vec![bar, Span::raw("  ")];
            second.extend(marked(task, task_width, search, task_style));
            let second = Line::from(second);
            let second = right(second, Span::styled(format!("{place} "), OVERLAY), width);

            for line in [first, second] {
                lines.push(if is_selected {
                    line.style(Style::default().bg(SURFACE0))
                } else {
                    line
                });
            }
            ListItem::new(lines)
        })
        .collect()
}

/// `line` with `end` pushed to the right edge of `width` columns.
fn right<'a>(mut line: Line<'a>, end: Span<'a>, width: usize) -> Line<'a> {
    let pad = width.saturating_sub(line.width() + end.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    line.spans.push(end);
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_agent_picker_pins_the_project_and_consumes_its_keys() {
        let mut sidebar = showing(vec![agent("%1", State::Idle)]);
        let selected = sidebar.selected().unwrap().session.clone();
        assert!(sidebar.new_agent_key(KeyEvent::from(KeyCode::Char('c')), Origin::Sidebar("%9")));
        assert_eq!(sidebar.new_agent.as_ref().unwrap().session, selected);
        for code in [KeyCode::Char('x'), KeyCode::Char('q'), KeyCode::Char('?')] {
            assert!(sidebar.new_agent_key(KeyEvent::from(code), Origin::Sidebar("%9")));
        }
        assert!(sidebar.killing.is_none());
        assert!(sidebar.new_agent_key(KeyEvent::from(KeyCode::Up), Origin::Sidebar("%9")));
        assert_eq!(
            sidebar.new_agent.as_ref().unwrap().selected,
            NEW_AGENT_KINDS.len() - 1
        );
        assert!(sidebar.new_agent_key(KeyEvent::from(KeyCode::Down), Origin::Sidebar("%9")));
        assert_eq!(sidebar.new_agent.as_ref().unwrap().selected, 0);
        assert!(sidebar.new_agent_key(KeyEvent::from(KeyCode::Esc), Origin::Sidebar("%9")));
        assert!(sidebar.new_agent.is_none());
        assert!(!sidebar.new_agent_key(KeyEvent::from(KeyCode::Char('j')), Origin::Sidebar("%9")));
    }

    #[test]
    fn new_agent_kinds_all_start_without_a_task() {
        for kind in NEW_AGENT_KINDS {
            assert!((kind.adapter().launch)(None).is_ok());
        }
    }

    #[test]
    fn new_agent_needs_a_selected_project() {
        let mut sidebar = showing(vec![]);
        assert!(sidebar.new_agent_key(KeyEvent::from(KeyCode::Char('c')), Origin::Sidebar("%9")));
        assert!(sidebar.new_agent.is_none());
        assert!(sidebar.notice.is_some());
    }

    use crate::index::EventState;

    #[test]
    fn help_closing_keys_never_act_on_the_list_and_question_mark_is_literal_in_edits() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Working)],
            ..Sidebar::default()
        };
        sidebar.navigation.order = vec!["%1".into()];
        sidebar.rebuild();
        for key in [
            KeyCode::Char('q'),
            KeyCode::Char('x'),
            KeyCode::Char('j'),
            KeyCode::Enter,
            KeyCode::Esc,
        ] {
            assert!(!sidebar.key(KeyEvent::from(KeyCode::Char('?'))));
            assert!(sidebar.help.open);
            assert!(!sidebar.key(KeyEvent::from(key)));
            assert!(!sidebar.help.open);
            assert_eq!(sidebar.selected().unwrap().pane, "%1");
            assert!(sidebar.killing.is_none());
        }
        press(&mut sidebar, KeyCode::Char('f'));
        press(&mut sidebar, KeyCode::Char('?'));
        assert_eq!(sidebar.navigation.query, "?");
        assert!(!sidebar.help.open);
        press(&mut sidebar, KeyCode::Esc);
        press(&mut sidebar, KeyCode::Char('r'));
        let before = sidebar.editing.as_ref().unwrap().name.clone();
        press(&mut sidebar, KeyCode::Char('?'));
        assert_eq!(sidebar.editing.as_ref().unwrap().name, before + "?");
        assert!(!sidebar.help.open);
    }

    #[test]
    fn help_footer_hint_and_confirmation_render_without_overlap() {
        use ratatui::{Terminal, backend::TestBackend};
        for height in [1, 2, 3, 4, 8, 28] {
            let mut terminal = Terminal::new(TestBackend::new(38, height)).unwrap();
            let mut sidebar = Sidebar::default();
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
            assert!(footer(&sidebar, 38, index::now_ms()).width() <= 38);
            assert!(
                footer(&sidebar, 38, index::now_ms())
                    .to_string()
                    .contains("? keys")
            );
            sidebar.help_key(KeyEvent::from(KeyCode::Char('?')));
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
            sidebar.help_key(KeyEvent::from(KeyCode::Char('q')));
            sidebar.killing = Some(confirmation(State::Working));
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
            sidebar.help_key(KeyEvent::from(KeyCode::Char('?')));
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
            let help: String = (0..height)
                .map(|y| {
                    (0..38)
                        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect();
            assert!(!help.contains("o configure"));
            assert!(!help.contains("? [Y/n]"));
            sidebar.help_key(KeyEvent::from(KeyCode::Char('x')));
            assert!(sidebar.killing.is_some());
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
        }
    }

    #[test]
    fn footer_priority_expiry_and_state_colour() {
        let mut sidebar = Sidebar {
            agents: Vec::new(),
            list: ListState::default(),
            error: None,
            editing: None,
            notice: None,
            recent_event: Some(RecentEvent {
                name: "grok-support".into(),
                state: EventState::Done,
                occurred_at: 100,
                expires_at: 5100,
            }),
            ..Sidebar::default()
        };
        for (event_state, state, expected) in [
            (EventState::Done, State::Done, " ✓ grok-support done"),
            (
                EventState::Blocked,
                State::Blocked,
                " ◆ grok-support blocked",
            ),
        ] {
            sidebar.recent_event.as_mut().unwrap().state = event_state;
            let line = footer(&sidebar, 38, 100);
            assert_eq!(line.to_string(), expected);
            assert_eq!(line.style.fg, Some(colour(state)));
        }
        assert!(footer(&sidebar, 38, 5100).to_string().contains("jump"));
        sidebar.navigation.search = Some(SearchEdit {
            query: String::new(),
            pane: None,
        });
        assert!(footer(&sidebar, 38, 100).to_string().contains("open"));
        assert!(footer(&sidebar, 38, 100).to_string().contains("freeze"));
        sidebar.editing = Some(Rename {
            pane: "%1".into(),
            name: "rename".into(),
        });
        assert!(footer(&sidebar, 38, 100).to_string().contains("save"));
        sidebar.notice = Some("rename failed".into());
        assert_eq!(footer(&sidebar, 38, 100).to_string(), " rename failed");
        assert_eq!(footer(&sidebar, 8, 100).to_string(), " rename…");
    }

    fn agent(pane: &str, state: State) -> Agent {
        Agent {
            name: format!("agent-{pane}"),
            pane: pane.into(),
            kind: index::Kind::Pi,
            state,
            detail: None,
            title: "Review payment flow".into(),
            location: "work:1.1".into(),
            session: "work".into(),
            window: "@1".into(),
            cwd: "/tmp/checkout".into(),
            parent: None,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    /// A sidebar showing `agents` in the order given.
    fn showing(agents: Vec<Agent>) -> Sidebar {
        let mut sidebar = Sidebar {
            navigation: Navigation {
                order: agents.iter().map(|a| a.pane.clone()).collect(),
                ..Navigation::default()
            },
            all_agents: agents,
            ..Sidebar::default()
        };
        sidebar.rebuild();
        sidebar
    }

    fn press(sidebar: &mut Sidebar, code: KeyCode) {
        sidebar.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn matches(query: &str, agent: &Agent) -> bool {
        !rank(&mut Matcher::default(), query, std::slice::from_ref(agent)).is_empty()
    }

    #[test]
    fn fuzzy_search_matches_case_insensitive_ordered_characters() {
        let mut a = agent("%1", State::Idle);
        a.name = "Sidebar-Search".into();
        a.title.clear();
        a.session = "pro".into();
        for (query, expected) in [
            ("", true),
            ("SIDEBAR", true),
            ("sbsrch", true),
            ("s-s", true),
            ("rsbs", false),
            ("sss", false),
            ("sidebar-search-search", false),
            ("missing", false),
        ] {
            assert_eq!(matches(query, &a), expected, "{query:?}");
        }
        a.name = "Éclair-Über".into();
        assert!(matches("éLÜB", &a));
    }

    #[test]
    fn fuzzy_search_covers_name_project_and_description_only() {
        let mut a = agent("%1", State::Blocked);
        a.name = "api-builder".into();
        a.detail = Some("Confirm permission".into());
        for query in [
            "ABLD",   // Agent name.
            "WRK",    // Project: the tmux session.
            "RVPMT",  // Task title, retained even while blocked.
            "CNFPRM", // The displayed blocked description.
            "ABLD RVPMT",
            "RVPMT ABLD",
            "CNFPRM WRK ABLD RVPMT",
        ] {
            assert!(matches(query, &a), "{query:?}");
        }
        // Paths would match nearly everything.
        for query in ["TMP/CKT", "work:1"] {
            assert!(!matches(query, &a), "{query:?}");
        }
    }

    #[test]
    fn search_ranks_best_first_and_composes_with_filters_and_edits() {
        let mut agents = vec![
            agent("%1", State::Working),
            agent("%2", State::Blocked),
            agent("%3", State::Idle),
        ];
        agents[0].name = "api-builder".into();
        agents[1].name = "abld".into(); // The tighter match goes first.
        agents[2].name = "zzz".into();
        agents[2].title = "Unrelated task".into();
        let mut sidebar = showing(agents);
        sidebar.navigation.order = ["%1", "%2", "%3"].map(String::from).to_vec();
        sidebar.rebuild();
        sidebar.select(Some(0));
        press(&mut sidebar, KeyCode::Char('f'));
        for c in "ABLD RVPMT".chars() {
            press(&mut sidebar, KeyCode::Char(c));
            assert!(sidebar.navigation.search.is_some());
        }
        assert_eq!(order(&sidebar), ["%2", "%1"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        press(&mut sidebar, KeyCode::Down);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        press(&mut sidebar, KeyCode::Char('/'));
        assert!(sidebar.navigation.search.is_none());
        assert_eq!(order(&sidebar), ["%2", "%1"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        press(&mut sidebar, KeyCode::Char('d'));
        assert_eq!(sidebar.navigation.filter, Some(State::Blocked));
        assert_eq!(order(&sidebar), ["%2"]);
        sidebar.all_agents[1].state = State::Idle;
        sidebar.rebuild();
        assert!(sidebar.agents.is_empty());
        assert_eq!(sidebar.navigation.filter, Some(State::Blocked));
        press(&mut sidebar, KeyCode::Char('d'));
        assert_eq!(sidebar.navigation.filter, Some(State::Working));
        press(&mut sidebar, KeyCode::Char('f'));
        press(&mut sidebar, KeyCode::Char('z'));
        assert!(sidebar.agents.is_empty());
        press(&mut sidebar, KeyCode::Backspace);
        assert_eq!(order(&sidebar), ["%1"]);
        sidebar.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(sidebar.navigation.query, "ABLD ");
        sidebar.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(sidebar.navigation.query.is_empty());
        press(&mut sidebar, KeyCode::Esc);
        assert_eq!(sidebar.navigation.query, "ABLD RVPMT");
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        press(&mut sidebar, KeyCode::Esc);
        assert!(sidebar.navigation.query.is_empty());
    }

    #[test]
    fn selection_priority_and_identity() {
        let mut sidebar = showing(vec![
            agent("%3", State::Blocked),
            agent("%2", State::Done),
            agent("%1", State::Working),
        ]);
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%3"));
        press(&mut sidebar, KeyCode::Down);
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%2"));
        sidebar.all_agents.reverse();
        sidebar.rebuild();
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        sidebar.all_agents.retain(|a| a.pane != "%2");
        sidebar.rebuild();
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
    }

    #[test]
    fn gg_goes_to_the_top_and_capital_g_to_the_bottom() {
        let mut sidebar = showing(vec![
            agent("%1", State::Idle),
            agent("%2", State::Idle),
            agent("%3", State::Idle),
        ]);
        sidebar.select(Some(1));
        press(&mut sidebar, KeyCode::Char('G'));
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%3"));
        press(&mut sidebar, KeyCode::Char('g'));
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%3"));
        press(&mut sidebar, KeyCode::Char('g'));
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%1"));
        // Another key between the two is no gg, whether it reaches the list or an action.
        press(&mut sidebar, KeyCode::Char('G'));
        press(&mut sidebar, KeyCode::Char('g'));
        press(&mut sidebar, KeyCode::Char('k'));
        press(&mut sidebar, KeyCode::Char('g'));
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%2"));
        assert!(sidebar.action_key(KeyEvent::from(KeyCode::Char('?')), Origin::Sidebar("%9")));
        press(&mut sidebar, KeyCode::Char('q'));
        press(&mut sidebar, KeyCode::Char('g'));
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%2"));
        // In a search, g is text.
        press(&mut sidebar, KeyCode::Char('f'));
        press(&mut sidebar, KeyCode::Char('g'));
        press(&mut sidebar, KeyCode::Char('g'));
        assert_eq!(sidebar.navigation.query, "gg");
        // An empty list has nowhere to go.
        let mut empty = showing(vec![]);
        for code in [KeyCode::Char('G'), KeyCode::Char('g'), KeyCode::Char('g')] {
            press(&mut empty, code);
        }
        assert!(empty.selected().is_none());
    }

    #[test]
    fn filters_compose_with_query_and_never_change_on_refresh() {
        let mut sidebar = showing(vec![
            agent("%1", State::Working),
            agent("%2", State::Done),
            agent("%3", State::Blocked),
            agent("%4", State::Idle),
        ]);
        for expected in [
            Some(State::Blocked),
            Some(State::Done),
            Some(State::Working),
            Some(State::Idle),
            None,
        ] {
            press(&mut sidebar, KeyCode::Char('d'));
            assert_eq!(sidebar.navigation.filter, expected);
        }
        sidebar.navigation.query = "AGENT-%2".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('d'));
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        sidebar.all_agents[1].state = State::Idle;
        sidebar.rebuild();
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        assert!(sidebar.selected().is_none());
        press(&mut sidebar, KeyCode::Char('d'));
        assert_eq!(sidebar.navigation.filter, Some(State::Idle));
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        sidebar.navigation.query = "no such agent".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('d'));
        assert_eq!(sidebar.navigation.filter, None);
        assert!(sidebar.agents.is_empty());
    }

    fn order(sidebar: &Sidebar) -> Vec<&str> {
        sidebar.agents.iter().map(|a| a.pane.as_str()).collect()
    }

    fn index_at(updated_at: u64, agents: &[Agent], order: &[&str]) -> Index {
        Index {
            updated_at,
            agents: agents.to_vec(),
            recent_event: None,
            windows: Vec::new(),
            order: order.iter().map(|pane| pane.to_string()).collect(),
            last_agent: None,
        }
    }

    #[test]
    fn rows_come_from_the_published_order_and_only_badges_change() {
        let mut agents = vec![
            agent("%1", State::Idle),
            agent("%2", State::Done),
            agent("%3", State::Blocked),
        ];
        let mut sidebar = showing(agents.clone());
        sidebar.apply(
            index_at(1, &agents, &["%3", "%2", "%1"]),
            Origin::Sidebar("%9"),
        );
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
        sidebar.select(Some(1));
        // Live badges in fixed rows, with identity surviving.
        agents[0].state = State::Blocked;
        agents[2].state = State::Idle;
        sidebar.apply(
            index_at(2, &agents, &["%3", "%2", "%1"]),
            Origin::Sidebar("%9"),
        );
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
        assert_eq!(sidebar.agents[0].state, State::Idle);
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
    }

    #[test]
    fn sessions_stay_sorted_and_a_withdrawn_agent_returns_to_its_row() {
        let mut other = agent("%3", State::Idle);
        other.session = "alpha".into();
        let agents = vec![agent("%1", State::Idle), agent("%2", State::Idle), other];
        let mut sidebar = showing(agents.clone());
        sidebar.apply(
            index_at(1, &agents, &["%2", "%1", "%3"]),
            Origin::Sidebar("%9"),
        );
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
        let without = [agents[0].clone(), agents[2].clone()];
        sidebar.apply(
            index_at(2, &without, &["%2", "%1", "%3"]),
            Origin::Sidebar("%9"),
        );
        assert_eq!(order(&sidebar), ["%3", "%1"]);
        sidebar.apply(
            index_at(3, &agents, &["%2", "%1", "%3"]),
            Origin::Sidebar("%9"),
        );
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
    }

    fn window(active: &str, viewed: bool) -> WindowView {
        WindowView {
            id: "@1".into(),
            active_pane: active.into(),
            viewed,
            sidebar: Some("%9".into()),
        }
    }

    #[test]
    fn bold_is_the_agent_active_in_this_window_even_while_hidden() {
        let mut sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Idle)]);
        sidebar.follow(Some(&window("%2", false)), None, "%9");
        assert_eq!(sidebar.in_use.as_deref(), Some("%2"));
        // The keyboard in the sidebar or a plain shell: no agent is in use.
        sidebar.follow(Some(&window("%9", true)), Some("%2"), "%9");
        assert_eq!(sidebar.in_use, None);
        sidebar.follow(Some(&window("%7", true)), Some("%2"), "%9");
        assert_eq!(sidebar.in_use, None);
        // Enter lands in the selected agent at once; a preview keeps the keyboard here.
        sidebar.select(Some(0));
        sidebar.switching(true);
        assert_eq!(sidebar.in_use.as_deref(), Some("%1"));
        sidebar.switching(false);
        assert_eq!(sidebar.in_use, None);
    }

    #[test]
    fn highlight_follows_the_agent_in_use_until_a_client_steers() {
        let mut sidebar = showing(vec![
            agent("%1", State::Idle),
            agent("%2", State::Idle),
            agent("%3", State::Idle),
        ]);
        let selected = |sidebar: &Sidebar| sidebar.selected().unwrap().pane.clone();
        // Hidden with an agent active: ready before any client arrives.
        sidebar.follow(Some(&window("%2", false)), Some("%3"), "%9");
        assert_eq!(selected(&sidebar), "%2");
        // Viewed with the agent active.
        sidebar.follow(Some(&window("%3", true)), Some("%2"), "%9");
        assert_eq!(selected(&sidebar), "%3");
        // A plain shell in front: the last agent used.
        sidebar.follow(Some(&window("%7", true)), Some("%1"), "%9");
        assert_eq!(selected(&sidebar), "%1");
        // Entering the sidebar starts from the last agent used, then the selection is yours.
        sidebar.follow(Some(&window("%9", true)), Some("%2"), "%9");
        assert_eq!(selected(&sidebar), "%2");
        press(&mut sidebar, KeyCode::Char('j'));
        assert_eq!(selected(&sidebar), "%3");
        sidebar.follow(Some(&window("%9", true)), Some("%2"), "%9");
        assert_eq!(selected(&sidebar), "%3");
        // Hidden with the sidebar active: the last agent used, ready for arrival.
        sidebar.follow(Some(&window("%9", false)), Some("%1"), "%9");
        assert_eq!(selected(&sidebar), "%1");
        // An agent hidden by a filter or search leaves the selection alone.
        sidebar.follow(Some(&window("%5", true)), Some("%8"), "%9");
        assert_eq!(selected(&sidebar), "%1");
        // A new sidebar, in no index yet: the last agent used. No last agent: nothing moves.
        sidebar.follow(None, Some("%3"), "%9");
        assert_eq!(selected(&sidebar), "%3");
        sidebar.follow(None, None, "%9");
        assert_eq!(selected(&sidebar), "%3");
    }

    #[test]
    fn a_key_outranks_any_index_older_than_it() {
        let agents = vec![agent("%1", State::Idle), agent("%2", State::Idle)];
        let mut sidebar = showing(agents.clone());
        let at = |updated_at, active: &str, viewed| Index {
            windows: vec![window(active, viewed)],
            last_agent: Some("%1".into()),
            ..index_at(updated_at, &agents, &["%1", "%2"])
        };
        sidebar.apply(at(1, "%1", true), Origin::Sidebar("%9"));
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        // The keyboard just moved here and the daemon has not seen it: `j` must stand.
        sidebar.pressed(100);
        press(&mut sidebar, KeyCode::Char('j'));
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        for updated_at in [50, 100] {
            sidebar.apply(at(updated_at, "%1", true), Origin::Sidebar("%9"));
            assert_eq!(sidebar.selected().unwrap().pane, "%2");
        }
        // The daemon now agrees the sidebar is active: still the client's selection.
        sidebar.apply(at(101, "%9", true), Origin::Sidebar("%9"));
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        // The client left: the rule resumes.
        sidebar.apply(at(102, "%9", false), Origin::Sidebar("%9"));
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
    }

    #[test]
    fn only_a_changed_frame_reaches_the_terminal() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Idle)]);
        let mut offscreen = Terminal::new(TestBackend::new(38, 12)).unwrap();
        let mut render =
            |sidebar: &mut Sidebar| offscreen.draw(|f| draw(f, sidebar)).unwrap().buffer.clone();
        sidebar
            .agents
            .iter_mut()
            .for_each(|a| a.state_since = u64::MAX);
        let first = render(&mut sidebar);
        assert_eq!(first, render(&mut sidebar));
        press(&mut sidebar, KeyCode::Char('j'));
        assert_ne!(first, render(&mut sidebar));
    }

    #[test]
    fn session_groups_and_search_acceptance_keep_a_stable_order() {
        let mut remote = agent("%3", State::Blocked);
        remote.session = "other".into();
        let mut sidebar = showing(vec![
            agent("%1", State::Idle),
            agent("%2", State::Working),
            remote,
            agent("%4", State::Blocked),
        ]);
        sidebar.navigation.order = ["%3", "%2", "%1", "%4"].map(String::from).to_vec();
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        sidebar.select(Some(2));
        press(&mut sidebar, KeyCode::Char('f'));
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        press(&mut sidebar, KeyCode::Char('/'));
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        sidebar.all_agents[2].state = State::Idle;
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        assert!(footer(&sidebar, 38, 100).to_string().contains("kill"));
    }

    #[test]
    fn search_edit_cancel_keep_clear_and_literal_keys() {
        let mut sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Working)]);
        press(&mut sidebar, KeyCode::Char('f'));
        for c in "PaYmEnT".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert_eq!(sidebar.agents.len(), 2);
        press(&mut sidebar, KeyCode::Char('/'));
        assert!(sidebar.navigation.search.is_none());
        assert_eq!(sidebar.navigation.query, "PaYmEnT");
        press(&mut sidebar, KeyCode::Char('f'));
        for c in "qFr".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert!(sidebar.editing.is_none());
        assert_eq!(sidebar.navigation.filter, None);
        assert!(sidebar.agents.is_empty());
        press(&mut sidebar, KeyCode::Esc);
        assert_eq!(sidebar.navigation.query, "PaYmEnT");
        press(&mut sidebar, KeyCode::Esc);
        assert!(sidebar.navigation.query.is_empty());
        let a = sidebar.all_agents.first_mut().unwrap();
        a.detail = Some("Please approve".into());
        for query in ["approve", "PAYMENT", "work", "AGENT-%1"] {
            sidebar.navigation.query = query.into();
            sidebar.rebuild();
            assert_eq!(sidebar.selected().unwrap().pane, "%1");
        }
    }

    #[test]
    fn enter_opens_from_a_search_and_ends_it_but_keeps_a_frozen_one() {
        let mut sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Idle)]);
        press(&mut sidebar, KeyCode::Char('f'));
        for c in "%2".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert_eq!(sidebar.open_selected().unwrap().pane, "%2");
        assert!(sidebar.navigation.search.is_none() && sidebar.navigation.query.is_empty());
        assert_eq!(order(&sidebar).len(), 2);
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        // A key that edits nothing leaves the selection alone.
        press(&mut sidebar, KeyCode::Char('f'));
        press(&mut sidebar, KeyCode::Backspace);
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        press(&mut sidebar, KeyCode::Esc);
        // No match: nothing to open, and the search stays open to edit.
        press(&mut sidebar, KeyCode::Char('f'));
        press(&mut sidebar, KeyCode::Char('☃'));
        assert!(sidebar.open_selected().is_none());
        press(&mut sidebar, KeyCode::Enter);
        assert_eq!(sidebar.navigation.query, "☃");
        assert!(sidebar.navigation.search.is_some());
        sidebar.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut sidebar, KeyCode::Char('1'));
        press(&mut sidebar, KeyCode::Char('/'));
        assert_eq!(sidebar.open_selected().unwrap().pane, "%1");
        assert_eq!(sidebar.navigation.query, "1");
    }

    /// Rows %1 to %4, with %2 and %4 findable by name and %3 by its task.
    fn searchable() -> Sidebar {
        let mut agents = vec![
            agent("%1", State::Idle),
            agent("%2", State::Idle),
            agent("%3", State::Idle),
            agent("%4", State::Idle),
        ];
        agents[1].name = "Cedar-One".into();
        agents[2].title = "review the CEDAR tree".into();
        agents[3].name = "cedar-two".into();
        let mut sidebar = showing(agents);
        sidebar.select(Some(0));
        sidebar
    }

    fn type_jump(sidebar: &mut Sidebar, text: &str) {
        press(sidebar, KeyCode::Char('/'));
        for c in text.chars() {
            press(sidebar, KeyCode::Char(c));
        }
    }

    #[test]
    fn search_moves_the_selection_ignoring_case_and_hides_nothing() {
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "cEd");
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        assert_eq!(sidebar.agents.len(), 4);
        // Project names are not searched.
        assert!(sidebar.agents.iter().all(|a| a.session == "work"));
        press(&mut sidebar, KeyCode::Esc);
        type_jump(&mut sidebar, "work");
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        assert_eq!(sidebar.notice.as_deref(), Some("no match"));
    }

    #[test]
    fn n_and_capital_n_go_through_the_matches_and_wrap_once_the_search_is_kept() {
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "cedar");
        press(&mut sidebar, KeyCode::Enter);
        assert!(sidebar.navigation.jump.is_none());
        let origin = Origin::Sidebar("%9");
        // `n` only ever goes through matches; it never reaches an action.
        assert!(!sidebar.action_key(KeyEvent::from(KeyCode::Char('n')), origin));
        for pane in ["%3", "%4", "%2"] {
            press(&mut sidebar, KeyCode::Char('n'));
            assert_eq!(sidebar.selected().unwrap().pane, pane);
        }
        press(&mut sidebar, KeyCode::Char('N'));
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        press(&mut sidebar, KeyCode::Esc);
        assert!(sidebar.navigation.pattern.is_empty());
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        // With no search left, `n` does nothing and the selection stays.
        press(&mut sidebar, KeyCode::Char('n'));
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        assert!(sidebar.new_agent.is_none());
    }

    #[test]
    fn cancelling_a_search_restores_the_row_and_the_search_before_it() {
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "two");
        press(&mut sidebar, KeyCode::Enter);
        sidebar.select(Some(0));
        type_jump(&mut sidebar, "cedar");
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        press(&mut sidebar, KeyCode::Esc);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        assert_eq!(sidebar.navigation.pattern, "two");
    }

    #[test]
    fn search_edits_restart_from_the_first_row_and_commands_are_text() {
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "cedar-t");
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        press(&mut sidebar, KeyCode::Backspace);
        press(&mut sidebar, KeyCode::Backspace);
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        let origin = Origin::Sidebar("%9");
        for key in ['x', 'o', 'c', '?'] {
            assert!(!sidebar.action_key(KeyEvent::from(KeyCode::Char(key)), origin));
        }
    }

    #[test]
    fn a_search_with_no_match_ends_on_enter_and_opening_a_match_ends_it() {
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "zzz");
        press(&mut sidebar, KeyCode::Enter);
        assert!(sidebar.navigation.pattern.is_empty());
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        type_jump(&mut sidebar, "cedar");
        press(&mut sidebar, KeyCode::Enter);
        assert_eq!(sidebar.open_selected().unwrap().pane, "%2");
        assert!(sidebar.navigation.pattern.is_empty());
    }

    #[test]
    fn search_only_visits_rows_the_filter_keeps() {
        let mut sidebar = searchable();
        sidebar.all_agents[3].state = State::Blocked;
        sidebar.navigation.filter = Some(State::Blocked);
        sidebar.rebuild();
        type_jump(&mut sidebar, "cedar");
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        press(&mut sidebar, KeyCode::Enter);
        press(&mut sidebar, KeyCode::Char('n'));
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
    }

    #[test]
    fn every_search_match_lights_up_as_it_is_typed_and_stops_when_cleared() {
        let lit = |sidebar: &mut Sidebar| -> String {
            let mut terminal = Terminal::new(TestBackend::new(38, 28)).unwrap();
            terminal.draw(|frame| draw(frame, sidebar)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..28)
                .flat_map(|y| (0..38).map(move |x| (x, y)))
                .filter(|(x, y)| buffer[(*x, *y)].bg == crate::ui::SEARCH_MATCH.bg.unwrap())
                .map(|(x, y)| buffer[(x, y)].symbol())
                .collect()
        };
        let mut sidebar = searchable();
        assert_eq!(lit(&mut sidebar), "");
        type_jump(&mut sidebar, "CEDA");
        // Two names and one task, on rows above and below the selection.
        assert_eq!(lit(&mut sidebar), "CedaCEDAceda");
        press(&mut sidebar, KeyCode::Enter);
        press(&mut sidebar, KeyCode::Char('n'));
        assert_eq!(lit(&mut sidebar), "CedaCEDAceda");
        press(&mut sidebar, KeyCode::Esc);
        assert_eq!(lit(&mut sidebar), "");
    }

    #[test]
    fn ctrl_c_cancels_a_search_as_esc_does() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let mut sidebar = searchable();
        type_jump(&mut sidebar, "two");
        press(&mut sidebar, KeyCode::Enter);
        sidebar.select(Some(0));
        type_jump(&mut sidebar, "cedar");
        sidebar.key(ctrl_c);
        assert!(sidebar.navigation.jump.is_none());
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        assert_eq!(sidebar.navigation.pattern, "two");
        press(&mut sidebar, KeyCode::Char('n'));
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        // Kept: Ctrl-C clears it, where the selection is. Nothing else listens to it.
        sidebar.key(ctrl_c);
        assert!(sidebar.navigation.pattern.is_empty());
        assert_eq!(sidebar.selected().unwrap().pane, "%4");
        sidebar.key(ctrl_c);
        assert!(!sidebar.auto_name.open);
    }

    #[test]
    fn esc_and_ctrl_c_clear_the_search_then_every_filter() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        for cancel in [KeyEvent::from(KeyCode::Esc), ctrl_c] {
            let mut sidebar = searchable();
            sidebar.all_agents[3].state = State::Blocked;
            press(&mut sidebar, KeyCode::Char('d'));
            assert_eq!(sidebar.navigation.filter, Some(State::Blocked));
            sidebar.navigation.query = "cedar".into();
            sidebar.navigation.pattern = "two".into();
            sidebar.rebuild();
            assert_eq!(order(&sidebar), ["%4"]);
            sidebar.key(cancel);
            assert!(sidebar.navigation.pattern.is_empty());
            assert_eq!(order(&sidebar), ["%4"]);
            sidebar.key(cancel);
            assert!(sidebar.unfiltered());
            assert_eq!(order(&sidebar).len(), 4);
            assert_eq!(sidebar.selected().unwrap().pane, "%4");
            // Typing a filter: cancelling restores what it was before.
            sidebar.navigation.query = "two".into();
            press(&mut sidebar, KeyCode::Char('f'));
            press(&mut sidebar, KeyCode::Char('x'));
            sidebar.key(cancel);
            assert!(sidebar.navigation.search.is_none());
            assert_eq!(sidebar.navigation.query, "two");
        }
    }

    #[test]
    fn only_shift_right_leaves_the_sidebar() {
        assert!(leaves_sidebar(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT
        )));
        assert!(!leaves_sidebar(KeyEvent::from(KeyCode::Right)));
        assert!(!leaves_sidebar(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::SHIFT
        )));
    }

    #[test]
    fn top_line_shows_the_filter_and_search_within_the_width() {
        let text = |navigation: &Navigation| -> String {
            query_fields(navigation, 30)
                .iter()
                .map(|span| span.content.to_string())
                .collect()
        };
        let mut navigation = Navigation::default();
        assert_eq!(text(&navigation), "  f filter  / search");
        navigation.pattern = "cedar".into();
        assert_eq!(text(&navigation), "  f  / cedar");
        navigation.query = "x".repeat(50);
        navigation.pattern = "y".repeat(50);
        assert!(text(&navigation).chars().count() <= 30);
    }

    #[test]
    fn rename_cannot_retarget_after_agent_leaves_filtered_list() {
        let mut sidebar = showing(vec![
            agent("%1", State::Blocked),
            agent("%2", State::Working),
        ]);
        press(&mut sidebar, KeyCode::Char('r'));
        sidebar.all_agents.remove(0);
        sidebar.rebuild();
        assert!(sidebar.editing.is_none());
        assert!(
            sidebar
                .notice
                .as_ref()
                .unwrap()
                .contains("rename cancelled")
        );
    }

    #[test]
    fn session_headings_use_the_pickers_colours_whatever_the_search() {
        let mut other = agent("%2", State::Idle);
        other.session = "zeta".into();
        other.name = "elsewhere".into();
        let mut sidebar = showing(vec![agent("%1", State::Idle), other]);
        let heading_colour = |sidebar: &mut Sidebar| {
            let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
            terminal.draw(|frame| draw(frame, sidebar)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            let row = (0..12)
                .find(|&y| (0..38).any(|x| buffer[(x, y)].symbol() == "z"))
                .unwrap();
            let x = (0..38).find(|&x| buffer[(x, row)].symbol() == "z").unwrap();
            buffer[(x, row)].fg
        };
        let zeta = group_colour(&sidebar.all_agents, "zeta");
        assert_ne!(zeta, group_colour(&sidebar.all_agents, "work"));
        assert_eq!(heading_colour(&mut sidebar), zeta);
        sidebar.navigation.query = "elsewhere".into();
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%2"]);
        assert_eq!(heading_colour(&mut sidebar), zeta);
    }

    #[test]
    fn switch_context_reads_clients_session_and_sidebar_from_one_output() {
        let context = parse_switch_context(
            "/dev/ttys1\x1f42\x1f$1\x1f@1\x1f%1\nsession\x1f$2\npane\x1f%7\x1f\npane\x1f%8\x1f1\n\
             left\x1f%1\x1f0\nleft\x1f%2\x1f1\n",
        )
        .unwrap();
        assert_eq!(context.previous.as_deref(), Some("%2"));
        assert_eq!(context.clients[0].name, "/dev/ttys1");
        assert_eq!(context.session, "$2");
        assert_eq!(context.sidebar.as_deref(), Some("%8"));
        let context = parse_switch_context("session\x1f$2\npane\x1f%7\x1f\n").unwrap();
        assert!(context.clients.is_empty() && context.sidebar.is_none());
        assert!(context.previous.is_none());
        assert!(parse_switch_context("pane\x1f%7\x1f\n").is_err());
        assert!(parse_switch_context("session\x1f$2\nnot-a-client\n").is_err());
    }

    #[test]
    fn a_window_takes_its_pane_back_only_after_the_client_has_left_it() {
        let switch = ["switch-client", "-c", "c", "-t", "=s:@2.%9"].map(String::from);
        assert_eq!(
            carry_command("%9", "%8", &switch, Some("%1")).join(" "),
            "swap-pane -s %9 -t %8 ; switch-client -c c -t =s:@2.%9 ; select-pane -t %1"
        );
        // Nothing to go back to: the sidebar swapped in stays active there.
        assert_eq!(
            carry_command("%9", "%8", &switch, None).join(" "),
            "swap-pane -s %9 -t %8 ; switch-client -c c -t =s:@2.%9"
        );
    }

    #[test]
    fn client_ambiguity_and_shared_targets_are_rejected() {
        let clients = vec![
            Client {
                name: "one".into(),
                pid: "1".into(),
                session: "$1".into(),
                window: "@1".into(),
                pane: "%1".into(),
            },
            Client {
                name: "two".into(),
                pid: "2".into(),
                session: "$2".into(),
                window: "@2".into(),
                pane: "%2".into(),
            },
        ];
        assert!(input_client(&clients, Origin::Sidebar("%9")).is_err());
        assert_eq!(
            input_client(&clients, Origin::Sidebar("%1")).unwrap().name,
            "one"
        );
        assert!(guard_destination(&clients, &clients[0], "$2", "@9").is_err());
        assert!(guard_destination(&clients, &clients[0], "$9", "@2").is_err());
        assert!(guard_destination(&clients, &clients[0], "$3", "@3").is_ok());
        let mut shared = clients;
        shared[1].pane = "%1".into();
        assert!(input_client(&shared, Origin::Sidebar("%1")).is_err());
    }

    #[test]
    fn narrow_and_empty_draws_keep_navigation_and_footer_safe() {
        use ratatui::{Terminal, backend::TestBackend};
        for (width, height) in [(38, 28), (20, 9), (8, 4), (1, 1), (0, 0)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut sidebar = Sidebar::default();
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
            sidebar.navigation.query = "very long query with Unicode 界".into();
            sidebar.navigation.search = Some(SearchEdit {
                query: String::new(),
                pane: None,
            });
            sidebar.all_agents.push(agent("%1", State::Done));
            sidebar.navigation.order = vec!["%1".into()];
            sidebar.rebuild();
            terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
        }
    }

    fn confirmation(state: State) -> KillConfirmation {
        KillConfirmation {
            agent: agent("%1", state),
            codex_pid: None,
            client: "client".into(),
            client_pid: "123".into(),
            client_session: "$1".into(),
            client_window: "@1".into(),
            ends_sessions: Vec::new(),
        }
    }

    #[test]
    fn kill_enter_and_lowercase_y_only_and_every_other_key_cancels_without_replay() {
        for code in [KeyCode::Enter, KeyCode::Char('y')] {
            assert!(kill_accepts(KeyEvent::new(code, KeyModifiers::NONE)));
        }
        for key in [
            KeyEvent::new(KeyCode::Char('y'), KeyModifiers::CONTROL),
            KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT),
        ] {
            assert!(!kill_accepts(key));
        }
        for code in [
            KeyCode::Esc,
            KeyCode::Char('n'),
            KeyCode::Char('Y'),
            KeyCode::Char('q'),
            KeyCode::Char('j'),
            KeyCode::Down,
            KeyCode::Char('r'),
            KeyCode::Char('d'),
            KeyCode::Char('v'),
            KeyCode::Char('x'),
            KeyCode::Char('f'),
        ] {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            assert!(!kill_accepts(key));
            let mut sidebar = showing(vec![agent("%1", State::Working), agent("%2", State::Idle)]);
            sidebar.killing = Some(confirmation(State::Working));
            sidebar.notice = Some("old error".into());
            assert!(sidebar.kill_key(key, Origin::Sidebar("%sidebar")));
            assert!(sidebar.killing.is_none());
            assert_eq!(sidebar.selected().unwrap().pane, "%1");
            assert!(sidebar.navigation.filter.is_none());
            assert!(sidebar.navigation.search.is_none());
            assert!(sidebar.editing.is_none());
            assert!(sidebar.closed.is_empty());
        }
    }

    #[test]
    fn a_sidebar_leaves_for_the_next_row_before_a_kill_and_the_picker_stays() {
        let sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Idle)]);
        let first = &sidebar.agents[0];
        let last = &sidebar.agents[1];
        assert_eq!(sidebar.next_row(&first.pane).unwrap().pane, last.pane);
        assert_eq!(sidebar.next_row(&last.pane).unwrap().pane, first.pane);
        assert!(sidebar.leaves(first, Origin::Sidebar("%9")));
        assert!(!sidebar.leaves(first, Origin::Popup("client")));
        // The only row: nowhere to go, so an attached final session stays refused.
        let alone = showing(vec![agent("%1", State::Idle)]);
        assert!(!alone.leaves(&alone.agents[0], Origin::Sidebar("%9")));
    }

    #[test]
    fn kill_guard_pins_native_identity_state_turn_name_and_location() {
        let original = agent("%1", State::Working);
        assert!(same_action_target(&original, &original));
        for change in 0..10 {
            let mut current = original.clone();
            match change {
                0 => current.pane = "%2".into(),
                1 => current.name = "new-name".into(),
                2 => current.state = State::Blocked,
                3 => current.state_since = 1,
                4 => current.session_id = Some("new-native-session".into()),
                5 => current.completion_epoch = Some("new-native-process".into()),
                6 => current.completions = 1,
                7 => current.title = "new task".into(),
                8 => current.window = "@2".into(),
                _ => current.session = "other".into(),
            }
            assert!(!same_action_target(&original, &current), "change {change}");
        }
    }

    #[test]
    fn kill_last_window_refuses_attached_session_and_names_remote_linked_sessions() {
        let windows = "$1\x1fscratch\x1f@1\n$1\x1fscratch\x1f@2\n$2\x1fremote\x1f@3\n";
        assert!(
            session_endings("@2", Some("$1"), windows)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            session_endings("@3", Some("$1"), windows).unwrap(),
            ["remote"]
        );
        assert!(
            session_endings("@3", Some("$2"), windows)
                .unwrap_err()
                .to_string()
                .contains("attached session")
        );
        // A client that leaves first can end the session it was in.
        assert_eq!(session_endings("@3", None, windows).unwrap(), ["remote"]);
        let linked = format!("{windows}$3\x1flinked\x1f@3\n");
        assert_eq!(
            session_endings("@3", Some("$1"), &linked).unwrap(),
            ["linked", "remote"]
        );
        assert!(session_endings("@3", Some("$3"), &linked).is_err());
        assert!(window_will_end("%1", "%1\x1f\n").unwrap());
        assert!(window_will_end("%1", "%1\x1f\n%sidebar\x1f1\n").unwrap());
        assert!(!window_will_end("%1", "%1\x1f\n%shell\x1f\n%sidebar\x1f1\n").unwrap());
        assert!(!window_will_end("%1", "%1\x1f\n%side1\x1f1\n%side2\x1f1\n").unwrap());
        assert!(window_will_end("%gone", "%1\x1f\n").is_err());
        assert!(window_will_end("%1", "malformed").is_err());
    }

    #[test]
    fn kill_never_offers_own_sidebar_and_prompt_keeps_name_state_and_default_yes_legible() {
        let mut sidebar = showing(vec![agent("%1", State::Idle)]);
        assert!(sidebar.kill_key(
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE),
            Origin::Sidebar("%1")
        ));
        assert!(sidebar.killing.is_none()); // No tmux/index call for the own-pane exclusion.
        let mut kill = confirmation(State::Working);
        kill.agent.name = "kill-name-is-long".into();
        for width in [20, 38] {
            let lines = kill_footer(&kill, width);
            assert_eq!(lines.len(), 2);
            assert!(lines[0].to_string().contains("kill-name-is-long"));
            assert!(lines[1].to_string().contains("working"));
            assert!(lines[1].to_string().ends_with("[Y/n]"));
            assert!(lines.iter().all(|line| line.width() <= width));
        }
        kill.ends_sessions = vec!["remote".into()];
        let lines = kill_footer(&kill, 38);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].to_string().contains("ends session remote? [Y/n]"));
        // Render confirmation over a local error/event; no hidden normal key hints.
        use ratatui::{Terminal, backend::TestBackend};
        sidebar.killing = Some(kill);
        sidebar.notice = Some("rename error".into());
        let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &mut sidebar)).unwrap();
        let rows: Vec<String> = (0..12)
            .map(|y| {
                (0..38)
                    .map(|x| terminal.backend().buffer()[(x, y)].symbol())
                    .collect()
            })
            .collect();
        assert!(rows[9].contains("kill-name-is-long"));
        assert!(rows[10].contains("working"));
        assert!(rows[11].contains("ends session remote? [Y/n]"));
        assert!(!rows.iter().any(|row| row.contains("rename error")));
    }

    fn rendered(sidebar: &mut Sidebar) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(38, 20)).unwrap();
        terminal.draw(|frame| draw(frame, sidebar)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..20)
            .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_list_that_fits_again_scrolls_back_to_the_top() {
        use ratatui::{Terminal, backend::TestBackend};
        fn screen(sidebar: &mut Sidebar, height: u16) -> String {
            let mut terminal = Terminal::new(TestBackend::new(38, height)).unwrap();
            terminal.draw(|frame| draw(frame, sidebar)).unwrap();
            terminal.backend().to_string()
        }
        let mut sidebar = showing(
            (0..10)
                .map(|i| agent(&format!("%{i}"), State::Idle))
                .collect(),
        );
        sidebar.select(Some(9));
        assert!(!screen(&mut sidebar, 20).contains("agent-%0"));

        let tall = screen(&mut sidebar, 40);
        for i in 0..10 {
            assert!(tall.contains(&format!("agent-%{i}")), "{tall}");
        }
    }

    #[test]
    fn enter_asks_for_a_restart_only_while_the_daemon_is_down() {
        let enter = KeyEvent::from(KeyCode::Enter);
        let origin = Origin::Sidebar("%9");
        let mut healthy = Sidebar::default();
        assert!(!healthy.action_key(enter, origin));
        assert!(!healthy.restarting);

        let mut down = Sidebar {
            error: Some("not running".into()),
            ..Sidebar::default()
        };
        assert!(rendered(&mut down).contains("enter restarts"));
        assert!(down.action_key(enter, origin));
        assert!(down.restarting);
        let screen = rendered(&mut down);
        assert!(screen.contains("restarting daemon"));
        assert!(!screen.contains("not running"));

        let mut renaming = Sidebar {
            error: Some("not running".into()),
            editing: Some(Rename {
                pane: "%1".into(),
                name: "claude".into(),
            }),
            ..Sidebar::default()
        };
        assert!(!renaming.action_key(enter, origin));
        assert!(!renaming.restarting);
    }

    #[test]
    fn a_failed_restart_shows_why_and_can_be_retried() {
        let mut sidebar = Sidebar {
            error: Some("not running".into()),
            restart_failure: Some("disk full".into()),
            ..Sidebar::default()
        };
        let screen = rendered(&mut sidebar);
        assert!(screen.contains("restart failed"));
        assert!(screen.contains("enter tries again"));
        assert!(screen.contains("disk full"));
        assert!(sidebar.action_key(KeyEvent::from(KeyCode::Enter), Origin::Sidebar("%9")));
        assert!(sidebar.restarting);
    }

    #[test]
    fn ctrl_w_deletes_one_word() {
        let mut name = "claude-154".to_string();
        delete_word(&mut name);
        assert_eq!(name, "claude-");
        delete_word(&mut name);
        assert_eq!(name, "");
        delete_word(&mut name);
        assert_eq!(name, "");
    }
}
