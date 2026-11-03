//! A narrow, always-on agent list in a pane down the side of a window.

use crate::agents::{ProcessTable, RawState, ScanContext};
use crate::commands;
use crate::index::{self, Agent, RecentEvent, State};
use crate::tmux;
use crate::ui::{
    LAVENDER, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, age, colour, counts, hints, icon,
    message, name_style, panel, state_style,
};
use anyhow::{Context, Result, bail};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::collections::{BTreeMap, HashSet};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const WIDTH: &str = "38";
const REFRESH: Duration = Duration::from_millis(500);
/// Pane option marking a sidebar pane, so the toggle finds it.
const MARK: &str = "@agent-plane-sidebar";
/// Global option set while the sidebar is open, in every window of every session.
const OPEN: &str = "@agent-plane-sidebar-open";
/// Global options every sidebar shares: pane IDs in row order, and the agent last used.
const ORDER: &str = "@agent-plane-sidebar-order";
const LAST_AGENT: &str = "@agent-plane-last-agent";

/// The sidebar key: close the sidebar everywhere when it is open, wherever you are; otherwise
/// open it everywhere and move into this window's.
pub fn toggle(window: &str) -> Result<()> {
    if is_open()? {
        return close();
    }
    tmux::run(&["set-option", "-g", OPEN, "1"])?;
    // The pane you open it from is the one the new sidebars should highlight.
    let active = tmux::run(&["display-message", "-p", "-t", window, "#{pane_id}"])?;
    tmux::run(&["set-option", "-g", LAST_AGENT, active.trim()])?;
    add_everywhere()?;
    let pane = add(window)?;
    tmux::run(&["select-pane", "-t", &pane]).map(drop)
}

/// Run by the plugin's hooks when a window comes on screen: add the sidebar to it while open.
pub fn follow(window: &str) -> Result<()> {
    if is_open()? {
        add(window)?;
    }
    Ok(())
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
    match result? {
        // `q` closes the sidebar everywhere, this pane included.
        Exit::Quit => close(),
        // The window's other panes are gone; leaving lets tmux close the window.
        Exit::Alone => Ok(()),
    }
}

enum Exit {
    Quit,
    Alone,
}

/// What a sidebar reads from tmux on each check, in one command.
struct Observation {
    alone: bool,
    viewed: bool,
    active: String,
    order: Vec<String>,
    last_agent: String,
}

fn parse_observation(output: &str) -> Result<Observation> {
    let line = output.trim_end_matches('\n');
    let fields: Vec<_> = line.split('\x1f').collect();
    let [panes, viewed, active, order, last_agent] = fields.as_slice() else {
        bail!("unexpected sidebar context: {line:?}");
    };
    Ok(Observation {
        alone: *panes == "1",
        viewed: *viewed == "1",
        active: active.to_string(),
        order: order.split_whitespace().map(String::from).collect(),
        last_agent: last_agent.to_string(),
    })
}

fn observe(own: &str) -> Result<Observation> {
    let format = format!(
        "#{{window_panes}}\x1f#{{&&:#{{window_active}},#{{session_attached}}}}\x1f\
         #{{P:#{{?pane_active,#{{pane_id}},}}}}\x1f#{{{ORDER}}}\x1f#{{{LAST_AGENT}}}"
    );
    parse_observation(&tmux::run(&["display-message", "-p", "-t", own, &format])?)
}

#[derive(Default)]
struct Navigation {
    view: bool,
    filter: Option<State>,
    query: String,
    pane: Option<String>,
    search: Option<SearchEdit>,
    order: Vec<String>,
}

struct SearchEdit {
    query: String,
    pane: Option<String>,
}

struct Rename {
    pane: String,
    name: String,
}

fn kill_accepts(key: KeyEvent) -> bool {
    key.modifiers.is_empty() && matches!(key.code, KeyCode::Enter | KeyCode::Char('y'))
}

struct KillConfirmation {
    agent: Agent,
    codex_pid: Option<i32>,
    client: String,
    client_pid: String,
    client_session: String,
    client_window: String,
    ends_sessions: Vec<String>,
}

/// Changes in task/state/turn or native identity require a new deliberate confirmation.
fn same_kill_target(before: &Agent, current: &Agent) -> bool {
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

/// Read the native signal again, rather than trusting a possibly one-tick-old index at y.
/// Codex has no session id in its title signal, so pin its unique live process PID too.
fn kill_identity(agent: &Agent) -> Result<Option<i32>> {
    let panes = tmux::list_panes()?;
    let pane = panes
        .iter()
        .find(|p| p.id == agent.pane && p.window == agent.window && p.session == agent.session)
        .context("agent exited; kill cancelled")?;
    if pane.name != agent.name
        && !(pane.name.is_empty()
            && agent.name
                == format!(
                    "{}-{}",
                    agent.kind.as_str(),
                    pane.id.trim_start_matches('%')
                ))
    {
        bail!("agent renamed; press x again");
    }
    let processes = ProcessTable::load()?;
    let observations = (agent.kind.adapter().scan)(&ScanContext {
        panes: &panes,
        processes: &processes,
    })?;
    let observation = observations
        .iter()
        .find(|observation| observation.pane_id == agent.pane)
        .context("agent exited or changed; press x again")?;
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
        bail!("agent state/identity changed; press x again");
    }
    if agent.kind == index::Kind::Codex {
        let pids = processes.descendants_named(pane.pid, "codex");
        if pids.len() != 1 {
            bail!("agent process changed; press x again");
        }
        Ok(Some(pids[0].pid))
    } else {
        if agent.session_id.is_none() {
            bail!("agent has no native identity; kill cancelled");
        }
        Ok(None)
    }
}

/// Include the sidebar's automatic alone exit and every session linking this window.
/// Refuse to remove the originating client's attached session, even indirectly.
fn ending_sessions(agent: &Agent, client: &Client) -> Result<Vec<String>> {
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
    session_endings(&agent.window, &client.session, &windows)
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

fn session_endings(window: &str, attached_session: &str, windows: &str) -> Result<Vec<String>> {
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
            if id == attached_session {
                bail!("would end your attached session; kill refused");
            }
            ending.push(name);
        }
    }
    ending.sort();
    Ok(ending)
}

#[derive(Default)]
struct Sidebar {
    all_agents: Vec<Agent>,
    agents: Vec<Agent>,
    navigation: Navigation,
    recent_event: Option<RecentEvent>,
    list: ListState,
    error: Option<String>,
    editing: Option<Rename>,
    notice: Option<String>,
    killing: Option<KillConfirmation>,
    /// Hide successfully closed panes until the daemon has removed them from the index.
    closed: HashSet<String>,
    /// The order as last read from or written to tmux.
    shared_order: Vec<String>,
    /// A client is viewing this sidebar with the keyboard in it: its selection is theirs.
    steering: bool,
    /// The agent a client is viewing here, once announced as the last one used.
    used: Option<String>,
}

impl Navigation {
    fn matches_query(&self, agent: &Agent) -> bool {
        let query = self.query.to_lowercase();
        [&agent.name, &agent.title, &agent.location, &agent.cwd]
            .into_iter()
            .chain(agent.detail.as_ref())
            .any(|field| field.to_lowercase().contains(&query))
    }

    /// Only a deliberate f skips categories. Refreshing must never change the filter.
    fn cycle_filter(&mut self, agents: &[Agent]) {
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
            .find(|state| {
                state.is_none()
                    || agents
                        .iter()
                        .any(|a| Some(a.state) == *state && self.matches_query(a))
            })
            .unwrap(); // All is always available, including an empty index/query.
    }
}

impl Sidebar {
    fn rank_order(&mut self) {
        let attention = |state| match state {
            State::Blocked => 0,
            State::Done => 1,
            State::Working => 2,
            State::Idle => 3,
        };
        let mut agents: Vec<_> = self.all_agents.iter().collect();
        agents.sort_by_key(|a| {
            (
                &a.session,
                attention(a.state),
                a.state_since,
                &a.location,
                &a.pane,
            )
        });
        self.navigation.order = agents.into_iter().map(|a| a.pane.clone()).collect();
    }

    /// Rows never move: arrivals append. A pane that leaves keeps its place, so an agent
    /// withdrawn by a brief scan fault returns to the same row. Only s rewrites the list.
    fn sync_order(&mut self) {
        if self.navigation.order.is_empty() {
            self.rank_order();
            return;
        }
        for agent in &self.all_agents {
            if !self.navigation.order.contains(&agent.pane) {
                self.navigation.order.push(agent.pane.clone());
            }
        }
    }

    /// Share one order across sidebars and show the agent in use, ahead of any client arriving.
    fn apply(&mut self, observation: &Observation, own: &str) {
        if !observation.order.is_empty() {
            self.navigation.order = observation.order.clone();
            self.shared_order = observation.order.clone();
        }
        self.refresh();
        self.follow(observation, own);
    }

    /// Highlight the agent in use. A client steering this sidebar owns its selection, which
    /// starts from the last agent used.
    fn follow(&mut self, observation: &Observation, own: &str) {
        let in_use = observation.active != own
            && self.all_agents.iter().any(|a| a.pane == observation.active);
        let steering = observation.viewed && observation.active == own;
        let target = if in_use {
            Some(&observation.active)
        } else if steering && self.steering {
            None
        } else {
            Some(&observation.last_agent)
        };
        self.steering = steering;
        let position = target.and_then(|pane| self.agents.iter().position(|a| &a.pane == pane));
        if position.is_some() {
            self.select(position);
        }
    }

    /// The pane to announce as last used, once per visit so that clients never contend.
    fn newly_used(&mut self, observation: &Observation, own: &str) -> Option<String> {
        let used = (observation.viewed
            && observation.active != own
            && self.all_agents.iter().any(|a| a.pane == observation.active))
        .then(|| observation.active.clone());
        if used == self.used {
            return None;
        }
        self.used = used.clone();
        used
    }

    fn publish_order(&mut self) -> Result<()> {
        if self.navigation.order != self.shared_order {
            tmux::run(&["set-option", "-g", ORDER, &self.navigation.order.join(" ")])?;
            self.shared_order = self.navigation.order.clone();
        }
        Ok(())
    }

    fn resort(&mut self) {
        self.rank_order();
        self.rebuild();
    }

    fn refresh(&mut self) {
        match index::read() {
            Ok(index) => {
                self.closed
                    .retain(|pane| index.agents.iter().any(|a| &a.pane == pane));
                self.all_agents = index
                    .agents
                    .into_iter()
                    .filter(|a| !self.closed.contains(&a.pane))
                    .collect();
                self.recent_event = index.recent_event;
                self.error = None;
            }
            Err(e) => {
                self.all_agents.clear();
                self.recent_event = None;
                self.error = Some(e.to_string());
            }
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.sync_order();
        self.agents = self
            .navigation
            .order
            .iter()
            .filter_map(|pane| self.all_agents.iter().find(|a| &a.pane == pane))
            .filter(|a| {
                self.navigation.filter.is_none_or(|state| a.state == state)
                    && self.navigation.matches_query(a)
            })
            .cloned()
            .collect();
        // Sessions by name: a stable sort keeps the list's order within each.
        self.agents.sort_by(|a, b| a.session.cmp(&b.session));
        let selected = self
            .navigation
            .pane
            .as_ref()
            .and_then(|pane| self.agents.iter().position(|a| &a.pane == pane))
            .or_else(|| {
                [State::Blocked, State::Done]
                    .into_iter()
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

    fn selected(&self) -> Option<&Agent> {
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
        match key.code {
            KeyCode::Esc => {
                let before = self.navigation.search.take().unwrap();
                self.navigation.query = before.query;
                self.navigation.pane = before.pane;
            }
            KeyCode::Enter => {
                self.navigation.search = None;
            }
            KeyCode::Down => {
                self.step(1);
                return;
            }
            KeyCode::Up => {
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
        self.rebuild();
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
    fn kill_key(&mut self, key: KeyEvent, own: &str) -> bool {
        if let Some(confirmation) = self.killing.take() {
            self.notice = None;
            if kill_accepts(key) {
                match self.confirm_kill(&confirmation, own) {
                    Ok(()) => {
                        let position = self
                            .agents
                            .iter()
                            .position(|a| a.pane == confirmation.agent.pane);
                        let next = position
                            .and_then(|i| {
                                self.agents
                                    .get(i + 1)
                                    .or_else(|| i.checked_sub(1).and_then(|i| self.agents.get(i)))
                            })
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
        if key.code != KeyCode::Char('x')
            || self.editing.is_some()
            || self.navigation.search.is_some()
        {
            return false;
        }
        self.notice = None;
        let result = (|| {
            let Some(selected) = self.selected().filter(|a| a.pane != own).cloned() else {
                return Ok(None);
            };
            let index = index::read()?;
            let current = index
                .agents
                .iter()
                .find(|a| a.pane == selected.pane)
                .context("agent exited; kill cancelled")?;
            if !same_kill_target(&selected, current) {
                bail!("agent changed; refresh and press x again");
            }
            let clients = clients()?;
            let client = input_client(&clients, own)?;
            let ends_sessions = ending_sessions(current, client)?;
            Ok(Some(KillConfirmation {
                agent: current.clone(),
                codex_pid: kill_identity(current)?,
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

    fn confirm_kill(&self, confirmation: &KillConfirmation, own: &str) -> Result<()> {
        if confirmation.agent.pane == own {
            bail!("cannot kill the sidebar");
        }
        let clients = clients()?;
        let client = input_client(&clients, own)?;
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
        if !same_kill_target(&confirmation.agent, current) {
            bail!("agent changed; press x again");
        }
        if ending_sessions(current, client)? != confirmation.ends_sessions {
            bail!("session/window changed; press x again");
        }
        // Read the native signal last, after the topology queries. Pane IDs are
        // server-lifetime identities, so an exited target cannot resolve to its neighbour.
        if kill_identity(current)? != confirmation.codex_pid {
            bail!("agent changed; press x again");
        }
        commands::kill_pane(&current.pane)
    }

    fn key(&mut self, key: KeyEvent) -> bool {
        if self.editing.is_some() {
            self.edit_key(key);
            return false;
        }
        self.notice = None;
        if self.navigation.search.is_some() {
            self.search_key(key);
            return false;
        }
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
            KeyCode::Char('v') => self.navigation.view = !self.navigation.view,
            KeyCode::Char('f') => {
                self.navigation.cycle_filter(&self.all_agents);
                self.rebuild();
            }
            KeyCode::Char('s') => self.resort(),
            KeyCode::Char('/') => {
                self.navigation.search = Some(SearchEdit {
                    query: self.navigation.query.clone(),
                    pane: self.navigation.pane.clone(),
                });
            }
            KeyCode::Esc => {
                self.navigation.query.clear();
                self.rebuild();
            }
            _ => {}
        }
        false
    }
}

/// list-clients expands these fields in each client's context, including active-pane clients.
struct Client {
    name: String,
    pid: String,
    session: String,
    window: String,
    pane: String,
}

const CLIENT_FORMAT: &str =
    "#{client_name}\x1f#{client_pid}\x1f#{session_id}\x1f#{window_id}\x1f#{pane_id}";

fn parse_client(line: &str) -> Result<Client> {
    let fields: Vec<_> = line.split('\x1f').collect();
    if fields.len() != 5 {
        bail!("unexpected client context: {line:?}");
    }
    Ok(Client {
        name: fields[0].into(),
        pid: fields[1].into(),
        session: fields[2].into(),
        window: fields[3].into(),
        pane: fields[4].into(),
    })
}

fn clients() -> Result<Vec<Client>> {
    tmux::run(&["list-clients", "-F", CLIENT_FORMAT])?
        .lines()
        .map(parse_client)
        .collect()
}

fn input_client<'a>(clients: &'a [Client], pane: &str) -> Result<&'a Client> {
    let mut candidates = clients.iter().filter(|client| client.pane == pane);
    let client = candidates.next().context("no client in this sidebar")?;
    if candidates.next().is_some() {
        bail!("multiple clients in sidebar; use a separate session");
    }
    Ok(client)
}

/// Sessions share their current window, and linked windows share their active pane.
/// Explicit -c alone does not isolate those changes. Refuse to disturb another client.
fn guard_destination(
    clients: &[Client],
    client: &Client,
    session: &str,
    window: &str,
) -> Result<()> {
    if clients.iter().any(|other| {
        other.name != client.name && (other.session == session || other.window == window)
    }) {
        bail!("target shared with another client; use a separate session");
    }
    Ok(())
}

/// What a switch must check, read in one tmux command: each command costs a process.
struct SwitchContext {
    clients: Vec<Client>,
    session: String,
    sidebar: Option<String>,
}

fn parse_switch_context(output: &str) -> Result<SwitchContext> {
    let mut clients = Vec::new();
    let mut session = None;
    let mut sidebar = None;
    for line in output.lines() {
        match line.split('\x1f').collect::<Vec<_>>().as_slice() {
            ["session", id] => session = Some(id.to_string()),
            ["pane", id, "1"] => sidebar = Some(id.to_string()),
            ["pane", _, _] => {}
            _ => clients.push(parse_client(line)?),
        }
    }
    Ok(SwitchContext {
        clients,
        session: session.context("switch context has no session")?,
        sidebar,
    })
}

/// The tmux command for a preview or Enter jump, or None when the client is already there.
fn switch_command(sidebar: &Sidebar, own: &str, focus: bool) -> Result<Option<Vec<String>>> {
    let Some(agent) = sidebar.selected() else {
        return Ok(None);
    };
    let window_target = format!("={}:{}", agent.session, agent.window);
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
    ])?)?;
    let client = input_client(&context.clients, own)?;
    guard_destination(&context.clients, client, &context.session, &agent.window)?;
    let destination = match context.sidebar {
        Some(pane) => pane,
        None => add(&window_target)?,
    };
    // Enter lands on the agent. Preview stays in this sidebar, so neither agent keys nor seen
    // state are manufactured.
    let pane = if focus { agent.pane.as_str() } else { own };
    let switch = [
        "switch-client".to_string(),
        "-c".to_string(),
        client.name.clone(),
        "-t".to_string(),
        format!("{window_target}.{pane}"),
    ];
    if destination != own {
        // This pane travels with the client, so its selection and keyboard input stay continuous
        // on screen. tmux runs the whole sequence before it redraws.
        let swap = ["swap-pane", "-s", own, "-t", &destination, ";"].map(String::from);
        return Ok(Some(swap.into_iter().chain(switch).collect()));
    }
    Ok(focus.then(|| switch.to_vec()))
}

fn event_loop(terminal: &mut DefaultTerminal) -> Result<Exit> {
    // This is the native sidebar process's own pane, never a caller inferred by the CLI.
    let own = std::env::var("TMUX_PANE")?;
    let mut sidebar = Sidebar::default();
    let mut refreshed = None;
    loop {
        if refreshed.is_none_or(|at: Instant| at.elapsed() >= REFRESH) {
            // Refresh may change selection when an agent vanishes. Only input triggers preview.
            let observation = observe(&own)?;
            if observation.alone {
                return Ok(Exit::Alone);
            }
            sidebar.apply(&observation, &own);
            sidebar.publish_order()?;
            if let Some(pane) = sidebar.newly_used(&observation, &own) {
                tmux::run(&["set-option", "-g", LAST_AGENT, &pane])?;
            }
            refreshed = Some(Instant::now());
        }
        // tmux rescales panes proportionally when the window or a client resizes, and a
        // dragged border moves them: every sidebar is always exactly WIDTH columns.
        if terminal.size()?.width != WIDTH.parse::<u16>()? {
            tmux::run(&["resize-pane", "-t", &own, "-x", WIDTH])?;
        }
        terminal.draw(|f| draw(f, &mut sidebar))?;
        if event::poll(Duration::from_millis(100))? {
            let mut focus = false;
            let mut preview = false;
            // Apply the complete buffered burst first, so a fast jjj makes one preview switch.
            loop {
                if let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    if sidebar.kill_key(key, &own) {
                        if !event::poll(Duration::ZERO)? {
                            break;
                        }
                        continue;
                    }
                    let command = sidebar.editing.is_none() && sidebar.navigation.search.is_none();
                    focus = key.code == KeyCode::Enter && command;
                    // Search edits, including arrows, stay in this pane. Only accepting the
                    // query previews its final selection; cancellation never switches context.
                    preview |= (key.code == KeyCode::Enter && sidebar.navigation.search.is_some())
                        || (command
                            && matches!(
                                key.code,
                                KeyCode::Up
                                    | KeyCode::Down
                                    | KeyCode::Esc
                                    | KeyCode::Char('j' | 'k' | 'f' | 'v')
                            ));
                    if sidebar.key(key) {
                        return Ok(Exit::Quit);
                    }
                }
                if focus || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            sidebar.publish_order()?;
            if focus || (preview && sidebar.navigation.view && sidebar.navigation.search.is_none())
            {
                match switch_command(&sidebar, &own, focus) {
                    Ok(Some(command)) => {
                        // Draw last, so the selection moves on screen together with the window.
                        terminal.draw(|f| draw(f, &mut sidebar))?;
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

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn draw(f: &mut Frame, sidebar: &mut Sidebar) {
    let confirmation = sidebar
        .killing
        .as_ref()
        .map(|kill| kill_footer(kill, f.area().width as usize));
    let footer_height = confirmation.as_ref().map_or(1, |lines| lines.len() as u16);
    let [list_area, help_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(footer_height)]).areas(f.area());
    let inner = list_area.width.saturating_sub(2) as usize;
    let block = panel(Line::from(" agents ")).title_top(counts(&sidebar.all_agents));
    let content = block.inner(list_area);
    f.render_widget(block, list_area);
    let [mode_area, agents_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(content);
    let mode = format!(
        " v {}{}  f {}",
        if sidebar.navigation.view { "on" } else { "off" },
        if sidebar.navigation.view && sidebar.navigation.search.is_some() {
            " · paused"
        } else {
            ""
        },
        sidebar.navigation.filter.map_or("all", State::as_str)
    );
    let query = &sidebar.navigation.query;
    let search = sidebar.navigation.search.is_some();
    let room = inner.saturating_sub(mode.chars().count() + 4);
    let query = if search {
        let tail: String = query.chars().rev().take(room).collect();
        format!("  / {}▏", tail.chars().rev().collect::<String>())
    } else if query.is_empty() {
        "  / search".into()
    } else {
        truncate(&format!("  / {query}"), room + 4)
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(mode, LAVENDER),
            Span::styled(query, if search { TEXT } else { SUBTEXT }),
        ])),
        mode_area,
    );
    if let Some(e) = &sidebar.error {
        message(
            f,
            agents_area,
            ratatui::widgets::Block::default(),
            colour(State::Blocked),
            &["daemon not running", e],
        );
    } else if sidebar.agents.is_empty() {
        let text = if sidebar.all_agents.is_empty() {
            [
                "no agents running",
                "start claude or codex in any tmux pane",
            ]
        } else {
            ["no matching agents", "f changes state · esc clears search"]
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
            selected,
            sidebar.editing.as_ref().map(|edit| &edit.name),
            inner,
        );
        f.render_stateful_widget(List::new(items), agents_area, &mut sidebar.list);
    }

    let footer = confirmation.map_or_else(
        || Text::from(footer(sidebar, inner + 2, index::now_ms())),
        Text::from,
    );
    f.render_widget(Paragraph::new(footer), help_area);
}

/// Two lines keep both name and state readable in a narrow pane. Remote session
/// destruction gets a third, explicit risk line; Y/n always remains visible.
fn kill_footer(confirmation: &KillConfirmation, width: usize) -> Vec<Line<'static>> {
    let agent = &confirmation.agent;
    let narrow = agent.name.chars().count() + 6 > width;
    let name = if narrow {
        truncate(&agent.name, width)
    } else {
        format!(" kill {}", agent.name)
    };
    let state = format!(
        "{}({}){}",
        if narrow { "kill " } else { " " },
        agent.state.as_str(),
        if confirmation.ends_sessions.is_empty() {
            "? Y/n"
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
            width.saturating_sub(5),
        );
        lines.push(Line::styled(format!("{risk}? Y/n"), colour(State::Blocked)));
    }
    lines
}

/// Local failures and rename controls take priority; a shared event never hides them.
fn footer(sidebar: &Sidebar, width: usize, now: u64) -> Line<'static> {
    match (&sidebar.notice, &sidebar.editing) {
        (Some(notice), _) => Line::styled(
            truncate(&format!(" {notice}"), width),
            colour(State::Blocked),
        ),
        (None, Some(_)) => hints(&[("⏎", "save"), ("esc", "cancel"), ("^w ^u", "clear")]),
        (None, None) if sidebar.navigation.search.is_some() => hints(&[
            (
                "⏎",
                if sidebar.navigation.view {
                    "preview"
                } else {
                    "keep"
                },
            ),
            ("esc", "cancel"),
            ("^w ^u", "clear"),
        ]),
        (None, None) => match sidebar
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
            None => hints(&[
                ("⏎", "jump"),
                ("s", "sort"),
                ("r", "rename"),
                ("x", "kill"),
                ("q", "close"),
            ]),
        },
    }
}

/// One item per agent, grouped under a heading per tmux session, with a blank line between
/// agents. The selected agent gets an accent bar and a background; its heading does not.
fn items<'a>(
    agents: &'a [Agent],
    selected: Option<usize>,
    editing: Option<&String>,
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
                    Span::styled(format!(" {name} "), Style::default().fg(LAVENDER).bold()),
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
                    shown.chars().rev().collect::<String>() + "▏"
                }
                None => truncate(&a.name, name_width),
            };
            let name_style = match renaming {
                Some(_) => Style::default()
                    .fg(colour(State::Done))
                    .add_modifier(Modifier::UNDERLINED),
                None => name_style(a),
            };
            let first = Line::from(vec![
                bar.clone(),
                Span::styled(format!("{} ", icon(a.state)), colour(a.state)),
                Span::styled(name, name_style),
            ]);
            let first = right(first, Span::styled(status, state_style(a.state)), width);

            // The window and pane: the session is in the heading.
            let place = a
                .location
                .strip_prefix(&format!("{}:", a.session))
                .unwrap_or(&a.location);
            let (task, task_style) = match (a.state, &a.detail) {
                (State::Blocked, Some(detail)) => (detail, Style::default().fg(colour(a.state))),
                _ => (&a.title, Style::default().fg(SUBTEXT)),
            };
            let task_width = width.saturating_sub(place.chars().count() + 6);
            let second = Line::from(vec![
                bar,
                Span::raw("  "),
                Span::styled(truncate(task, task_width), task_style),
            ]);
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
    use crate::index::EventState;

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
        assert!(footer(&sidebar, 38, 100).to_string().contains("keep"));
        sidebar.navigation.view = true;
        assert!(footer(&sidebar, 38, 100).to_string().contains("preview"));
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

    fn press(sidebar: &mut Sidebar, code: KeyCode) {
        sidebar.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn selection_priority_identity_and_mode_off() {
        let mut sidebar = Sidebar {
            all_agents: vec![
                agent("%1", State::Working),
                agent("%2", State::Done),
                agent("%3", State::Blocked),
            ],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        assert!(!sidebar.navigation.view);
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%3"));
        press(&mut sidebar, KeyCode::Down);
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%2"));
        sidebar.all_agents.reverse();
        sidebar.rebuild();
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        sidebar.all_agents.retain(|a| a.pane != "%2");
        sidebar.rebuild();
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
        press(&mut sidebar, KeyCode::Char('v'));
        assert!(sidebar.navigation.view);
        press(&mut sidebar, KeyCode::Char('v'));
        assert!(!sidebar.navigation.view);
    }

    #[test]
    fn filters_compose_with_query_and_never_change_on_refresh() {
        let mut sidebar = Sidebar {
            all_agents: vec![
                agent("%1", State::Working),
                agent("%2", State::Done),
                agent("%3", State::Blocked),
                agent("%4", State::Idle),
            ],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        for expected in [
            Some(State::Blocked),
            Some(State::Done),
            Some(State::Working),
            Some(State::Idle),
            None,
        ] {
            press(&mut sidebar, KeyCode::Char('f'));
            assert_eq!(sidebar.navigation.filter, expected);
        }
        sidebar.navigation.query = "AGENT-%2".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('f'));
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        sidebar.all_agents[1].state = State::Idle;
        sidebar.rebuild();
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        assert!(sidebar.selected().is_none());
        press(&mut sidebar, KeyCode::Char('f'));
        assert_eq!(sidebar.navigation.filter, Some(State::Idle));
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        sidebar.navigation.query = "no such agent".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('f'));
        assert_eq!(sidebar.navigation.filter, None);
        assert!(sidebar.agents.is_empty());
    }

    fn order(sidebar: &Sidebar) -> Vec<&str> {
        sidebar.agents.iter().map(|a| a.pane.as_str()).collect()
    }

    #[test]
    fn attention_order_latches_badges_appends_removes_and_resorts_without_retargeting() {
        let mut sidebar = Sidebar {
            all_agents: vec![
                agent("%1", State::Idle),
                agent("%2", State::Done),
                agent("%3", State::Blocked),
                agent("%4", State::Blocked),
            ],
            ..Sidebar::default()
        };
        sidebar.all_agents[2].state_since = 20;
        sidebar.all_agents[3].state_since = 10;
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%4", "%3", "%2", "%1"]);
        sidebar.select(Some(1)); // identity %3 must survive every rebuild.
        sidebar.all_agents[3].state = State::Idle;
        sidebar.all_agents[0].state = State::Blocked;
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%4", "%3", "%2", "%1"]);
        assert_eq!(sidebar.agents[0].state, State::Idle); // live badge, fixed row.
        sidebar.all_agents.push(agent("%5", State::Blocked));
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%4", "%3", "%2", "%1", "%5"]);
        sidebar.all_agents.retain(|a| a.pane != "%2");
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%4", "%3", "%1", "%5"]);
        press(&mut sidebar, KeyCode::Char('s'));
        assert_eq!(order(&sidebar), ["%1", "%5", "%3", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
        // State changes never move rows.
        sidebar.all_agents[0].state = State::Working;
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%1", "%5", "%3", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
    }

    fn observed(viewed: bool, active: &str, order: &[&str], last_agent: &str) -> Observation {
        Observation {
            alone: false,
            viewed,
            active: active.into(),
            order: order.iter().map(|pane| pane.to_string()).collect(),
            last_agent: last_agent.into(),
        }
    }

    #[test]
    fn departed_panes_keep_their_place_and_sessions_stay_sorted() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle), agent("%2", State::Idle)],
            ..Sidebar::default()
        };
        let mut other = agent("%3", State::Idle);
        other.session = "alpha".into();
        sidebar.navigation.order = vec!["%2".into(), "%1".into(), "%3".into()];
        sidebar.all_agents.push(other);
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
        let withdrawn = sidebar.all_agents.remove(1);
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%1"]);
        sidebar.all_agents.push(withdrawn);
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
    }

    #[test]
    fn observation_parses_all_fields_and_fails_loudly() {
        let parsed = parse_observation("2\x1f1\x1f%4\x1f%1 %2\x1f%9\n").unwrap();
        assert!(!parsed.alone && parsed.viewed);
        assert_eq!(parsed.active, "%4");
        assert_eq!(parsed.order, ["%1", "%2"]);
        assert_eq!(parsed.last_agent, "%9");
        assert!(parse_observation("1\x1f0\x1f%4\x1f\x1f\n").unwrap().alone);
        assert!(parse_observation("2\x1f1\n").is_err());
    }

    #[test]
    fn highlight_follows_the_agent_in_use_until_a_client_steers() {
        let mut sidebar = Sidebar {
            all_agents: vec![
                agent("%1", State::Idle),
                agent("%2", State::Idle),
                agent("%3", State::Idle),
            ],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        let selected = |sidebar: &Sidebar| sidebar.selected().unwrap().pane.clone();
        // Hidden with an agent active: ready before any client arrives.
        sidebar.follow(&observed(false, "%2", &[], "%3"), "%9");
        assert_eq!(selected(&sidebar), "%2");
        // Viewed with the agent active.
        sidebar.follow(&observed(true, "%3", &[], "%2"), "%9");
        assert_eq!(selected(&sidebar), "%3");
        // A plain shell in front: the last agent used.
        sidebar.follow(&observed(true, "%7", &[], "%1"), "%9");
        assert_eq!(selected(&sidebar), "%1");
        // Entering the sidebar starts from the last agent used, then the selection is yours.
        sidebar.follow(&observed(true, "%9", &[], "%2"), "%9");
        assert_eq!(selected(&sidebar), "%2");
        press(&mut sidebar, KeyCode::Char('j'));
        assert_eq!(selected(&sidebar), "%3");
        sidebar.follow(&observed(true, "%9", &[], "%2"), "%9");
        assert_eq!(selected(&sidebar), "%3");
        // Hidden with the sidebar active: the last agent used, ready for arrival.
        sidebar.follow(&observed(false, "%9", &[], "%1"), "%9");
        assert_eq!(selected(&sidebar), "%1");
        // An agent hidden by a filter or search leaves the selection alone.
        sidebar.follow(&observed(true, "%5", &[], "%8"), "%9");
        assert_eq!(selected(&sidebar), "%1");
    }

    #[test]
    fn last_used_is_announced_once_per_visit() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle)],
            ..Sidebar::default()
        };
        let viewing = observed(true, "%1", &[], "");
        assert_eq!(sidebar.newly_used(&viewing, "%9").as_deref(), Some("%1"));
        assert_eq!(sidebar.newly_used(&viewing, "%9"), None);
        // Another pane took over while this sidebar was hidden: returning announces again.
        assert_eq!(
            sidebar.newly_used(&observed(false, "%1", &[], "%7"), "%9"),
            None
        );
        assert_eq!(sidebar.newly_used(&viewing, "%9").as_deref(), Some("%1"));
        // A sidebar or shell in front is not an agent in use.
        assert_eq!(
            sidebar.newly_used(&observed(true, "%9", &[], ""), "%9"),
            None
        );
        assert_eq!(
            sidebar.newly_used(&observed(true, "%5", &[], ""), "%9"),
            None
        );
    }

    #[test]
    fn session_groups_and_search_acceptance_keep_a_stable_order() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle), agent("%2", State::Working)],
            ..Sidebar::default()
        };
        let mut remote = agent("%3", State::Blocked);
        remote.session = "other".into();
        sidebar.all_agents.push(remote);
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1"]);
        sidebar.all_agents.push(agent("%4", State::Blocked));
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        sidebar.select(Some(2));
        press(&mut sidebar, KeyCode::Char('/'));
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        press(&mut sidebar, KeyCode::Enter);
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        sidebar.all_agents[2].state = State::Idle;
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        assert!(footer(&sidebar, 38, 100).to_string().contains("sort"));
    }

    #[test]
    fn search_edit_cancel_keep_clear_and_literal_keys() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle), agent("%2", State::Working)],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('/'));
        for c in "PaYmEnT".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert_eq!(sidebar.agents.len(), 2);
        press(&mut sidebar, KeyCode::Enter);
        assert!(sidebar.navigation.search.is_none());
        assert_eq!(sidebar.navigation.query, "PaYmEnT");
        press(&mut sidebar, KeyCode::Char('/'));
        for c in "qVFr".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert!(!sidebar.navigation.view);
        assert!(sidebar.editing.is_none());
        assert_eq!(sidebar.navigation.filter, None);
        assert!(sidebar.agents.is_empty());
        press(&mut sidebar, KeyCode::Esc);
        assert_eq!(sidebar.navigation.query, "PaYmEnT");
        press(&mut sidebar, KeyCode::Esc);
        assert!(sidebar.navigation.query.is_empty());
        let a = sidebar.all_agents.first_mut().unwrap();
        a.detail = Some("Please approve".into());
        for query in ["approve", "CHECKOUT", "work:1", "AGENT-%1"] {
            sidebar.navigation.query = query.into();
            sidebar.rebuild();
            assert_eq!(sidebar.selected().unwrap().pane, "%1");
        }
    }

    #[test]
    fn rename_cannot_retarget_after_agent_leaves_filtered_list() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Blocked), agent("%2", State::Working)],
            ..Sidebar::default()
        };
        sidebar.rebuild();
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
    fn switch_context_reads_clients_session_and_sidebar_from_one_output() {
        let context = parse_switch_context(
            "/dev/ttys1\x1f42\x1f$1\x1f@1\x1f%1\nsession\x1f$2\npane\x1f%7\x1f\npane\x1f%8\x1f1\n",
        )
        .unwrap();
        assert_eq!(context.clients[0].name, "/dev/ttys1");
        assert_eq!(context.session, "$2");
        assert_eq!(context.sidebar.as_deref(), Some("%8"));
        let context = parse_switch_context("session\x1f$2\npane\x1f%7\x1f\n").unwrap();
        assert!(context.clients.is_empty() && context.sidebar.is_none());
        assert!(parse_switch_context("pane\x1f%7\x1f\n").is_err());
        assert!(parse_switch_context("session\x1f$2\nnot-a-client\n").is_err());
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
        assert!(input_client(&clients, "%9").is_err());
        assert_eq!(input_client(&clients, "%1").unwrap().name, "one");
        assert!(guard_destination(&clients, &clients[0], "$2", "@9").is_err());
        assert!(guard_destination(&clients, &clients[0], "$9", "@2").is_err());
        assert!(guard_destination(&clients, &clients[0], "$3", "@3").is_ok());
        let mut shared = clients;
        shared[1].pane = "%1".into();
        assert!(input_client(&shared, "%1").is_err());
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
            KeyCode::Char('f'),
            KeyCode::Char('v'),
            KeyCode::Char('x'),
            KeyCode::Char('/'),
        ] {
            let key = KeyEvent::new(code, KeyModifiers::NONE);
            assert!(!kill_accepts(key));
            let mut sidebar = Sidebar {
                all_agents: vec![agent("%1", State::Working), agent("%2", State::Idle)],
                killing: Some(confirmation(State::Working)),
                notice: Some("old error".into()),
                ..Sidebar::default()
            };
            sidebar.rebuild();
            assert!(sidebar.kill_key(key, "%sidebar"));
            assert!(sidebar.killing.is_none());
            assert_eq!(sidebar.selected().unwrap().pane, "%1");
            assert!(!sidebar.navigation.view);
            assert!(sidebar.navigation.filter.is_none());
            assert!(sidebar.navigation.search.is_none());
            assert!(sidebar.editing.is_none());
            assert!(sidebar.closed.is_empty());
        }
    }

    #[test]
    fn kill_guard_pins_native_identity_state_turn_name_and_location() {
        let original = agent("%1", State::Working);
        assert!(same_kill_target(&original, &original));
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
            assert!(!same_kill_target(&original, &current), "change {change}");
        }
    }

    #[test]
    fn kill_last_window_refuses_attached_session_and_names_remote_linked_sessions() {
        let windows = "$1\x1fscratch\x1f@1\n$1\x1fscratch\x1f@2\n$2\x1fremote\x1f@3\n";
        assert!(session_endings("@2", "$1", windows).unwrap().is_empty());
        assert_eq!(session_endings("@3", "$1", windows).unwrap(), ["remote"]);
        assert!(
            session_endings("@3", "$2", windows)
                .unwrap_err()
                .to_string()
                .contains("attached session")
        );
        let linked = format!("{windows}$3\x1flinked\x1f@3\n");
        assert_eq!(
            session_endings("@3", "$1", &linked).unwrap(),
            ["linked", "remote"]
        );
        assert!(session_endings("@3", "$3", &linked).is_err());
        assert!(window_will_end("%1", "%1\x1f\n").unwrap());
        assert!(window_will_end("%1", "%1\x1f\n%sidebar\x1f1\n").unwrap());
        assert!(!window_will_end("%1", "%1\x1f\n%shell\x1f\n%sidebar\x1f1\n").unwrap());
        assert!(!window_will_end("%1", "%1\x1f\n%side1\x1f1\n%side2\x1f1\n").unwrap());
        assert!(window_will_end("%gone", "%1\x1f\n").is_err());
        assert!(window_will_end("%1", "malformed").is_err());
    }

    #[test]
    fn kill_never_offers_own_sidebar_and_prompt_keeps_name_state_and_default_yes_legible() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle)],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        assert!(sidebar.kill_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE), "%1"));
        assert!(sidebar.killing.is_none()); // No tmux/index call for the own-pane exclusion.
        let mut kill = confirmation(State::Working);
        kill.agent.name = "kill-name-is-long".into();
        for width in [20, 38] {
            let lines = kill_footer(&kill, width);
            assert_eq!(lines.len(), 2);
            assert!(lines[0].to_string().contains("kill-name-is-long"));
            assert!(lines[1].to_string().contains("working"));
            assert!(lines[1].to_string().ends_with("Y/n"));
            assert!(lines.iter().all(|line| line.width() <= width));
        }
        kill.ends_sessions = vec!["remote".into()];
        let lines = kill_footer(&kill, 38);
        assert_eq!(lines.len(), 3);
        assert!(lines[2].to_string().contains("ends session remote? Y/n"));
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
        assert!(rows[11].contains("ends session remote? Y/n"));
        assert!(!rows.iter().any(|row| row.contains("rename error")));
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
