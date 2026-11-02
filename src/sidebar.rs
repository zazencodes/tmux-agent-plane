//! A narrow, always-on agent list in a pane down the side of a window.

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
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const WIDTH: &str = "38";
const REFRESH: Duration = Duration::from_millis(500);
/// Pane option marking a sidebar pane, so the toggle finds it.
const MARK: &str = "@agent-plane-sidebar";
/// Global option set while the sidebar is open, in every window of every session.
const OPEN: &str = "@agent-plane-sidebar-open";
/// A one-shot transfer to the sidebar in the previewed window. Never global: only the
/// named client may adopt it, after that client has arrived in this pane.
const HANDOFF: &str = "@agent-plane-sidebar-navigation";

/// The sidebar key: close the sidebar everywhere when it is open, wherever you are; otherwise
/// open it everywhere and move into this window's.
pub fn toggle(window: &str) -> Result<()> {
    if is_open()? {
        return close();
    }
    tmux::run(&["set-option", "-g", OPEN, "1"])?;
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

/// The sidebar is the last pane left in its window.
fn alone() -> Result<bool> {
    let pane = std::env::var("TMUX_PANE")?;
    let panes = tmux::run(&["display-message", "-p", "-t", &pane, "#{window_panes}"])?;
    Ok(panes.trim() == "1")
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct Navigation {
    view: bool,
    filter: Option<State>,
    query: String,
    pane: Option<String>,
    search: Option<SearchEdit>,
    order: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
struct SearchEdit {
    query: String,
    pane: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Handoff {
    client: String,
    client_pid: String,
    navigation: Navigation,
}

struct Rename {
    pane: String,
    name: String,
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
    focused: bool,
}

impl Navigation {
    fn matches_query(&self, agent: &Agent) -> bool {
        let query = self.query.to_lowercase();
        [&agent.name, &agent.title, &agent.location, &agent.cwd]
            .into_iter()
            .chain(agent.detail.as_ref())
            .any(|field| field.to_lowercase().contains(&query))
    }

    /// Only a deliberate F skips categories. Refreshing must never change the filter.
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
    fn update_focus(&mut self, focused: bool) {
        if focused && !self.focused {
            // A fresh keyboard visit ranks current state; V handoffs set focused directly.
            self.navigation.order.clear();
        }
        self.focused = focused;
    }

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

    /// Keep existing rows in place; append arrivals within their existing session group.
    fn sync_order(&mut self) {
        if self.navigation.order.is_empty() {
            self.rank_order();
            return;
        }
        let mut sessions: Vec<String> = Vec::new();
        for pane in &self.navigation.order {
            if let Some(agent) = self.all_agents.iter().find(|a| &a.pane == pane)
                && !sessions.contains(&agent.session)
            {
                sessions.push(agent.session.clone());
            }
        }
        for agent in &self.all_agents {
            if !sessions.contains(&agent.session) {
                sessions.push(agent.session.clone());
            }
        }
        let mut order = Vec::new();
        for session in sessions {
            for pane in &self.navigation.order {
                if self
                    .all_agents
                    .iter()
                    .any(|a| &a.pane == pane && a.session == session)
                {
                    order.push(pane.clone());
                }
            }
            for agent in &self.all_agents {
                if agent.session == session && !order.contains(&agent.pane) {
                    order.push(agent.pane.clone());
                }
            }
        }
        self.navigation.order = order;
    }

    fn resort(&mut self) {
        self.rank_order();
        self.rebuild();
    }

    fn refresh(&mut self) {
        match index::read() {
            Ok(index) => {
                self.all_agents = index.agents;
                self.recent_event = index.recent_event;
                self.error = None;
            }
            Err(e) => {
                self.all_agents.clear();
                self.recent_event = None;
                self.error = Some(e.to_string());
            }
        }
        if !self.focused {
            self.rank_order();
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
                self.rank_order();
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
            KeyCode::Char('V') => self.navigation.view = !self.navigation.view,
            KeyCode::Char('F') => {
                self.navigation.cycle_filter(&self.all_agents);
                self.resort();
            }
            KeyCode::Char('S') => self.resort(),
            KeyCode::Char('\\') => {
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

fn clients() -> Result<Vec<Client>> {
    let output = tmux::run(&[
        "list-clients",
        "-F",
        "#{client_name}\x1f#{client_pid}\x1f#{session_id}\x1f#{window_id}\x1f#{pane_id}",
    ])?;
    output
        .lines()
        .map(|line| {
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
        })
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

fn navigate(sidebar: &Sidebar, own: &str, focus: bool) -> Result<Option<Client>> {
    let Some(agent) = sidebar.selected() else {
        return Ok(None);
    };
    let clients = clients()?;
    let client = input_client(&clients, own)?;
    let window_target = format!("={}:{}", agent.session, agent.window);
    // Resolve the indexed session explicitly: a pane can be linked into several sessions.
    let session = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &window_target,
        "#{session_id}",
    ])?;
    guard_destination(&clients, client, session.trim(), &agent.window)?;
    let target = if focus {
        agent.pane.clone()
    } else {
        add(&window_target)?
    };
    if target == own {
        return Ok(None);
    }
    if !focus {
        let handoff = Handoff {
            client: client.name.clone(),
            client_pid: client.pid.clone(),
            navigation: sidebar.navigation.clone(),
        };
        tmux::run(&[
            "set-option",
            "-p",
            "-t",
            &target,
            HANDOFF,
            &serde_json::to_string(&handoff)?,
        ])?;
    }
    // One targeted tmux command changes session/window/pane without an intermediate agent focus.
    // Preview targets only the sidebar, so neither agent keys nor seen state are manufactured.
    let pane_target = format!("={}:{}.{}", agent.session, agent.window, target);
    if let Err(error) = tmux::run(&["switch-client", "-c", &client.name, "-t", &pane_target]) {
        if !focus {
            tmux::run(&["set-option", "-pu", "-t", &target, HANDOFF])
                .context(format!("{error}; also failed to clear sidebar handoff"))?;
        }
        return Err(error);
    }
    Ok((!focus).then(|| Client {
        name: client.name.clone(),
        pid: client.pid.clone(),
        session: client.session.clone(),
        window: client.window.clone(),
        pane: target,
    }))
}

fn receive_handoff(sidebar: &mut Sidebar, own: &str) -> Result<bool> {
    let value = tmux::run(&["show-option", "-pqv", "-t", own, HANDOFF])?;
    if value.trim().is_empty() {
        return Ok(false);
    }
    let handoff: Handoff =
        serde_json::from_str(value.trim()).context("invalid sidebar navigation handoff")?;
    let clients = clients()?;
    if clients.iter().any(|client| {
        client.name == handoff.client && client.pid == handoff.client_pid && client.pane == own
    }) {
        // Consume before handling buffered keys in the destination pane.
        tmux::run(&["set-option", "-pu", "-t", own, HANDOFF])?;
        sidebar.navigation = handoff.navigation;
        sidebar.focused = true;
        sidebar.editing = None;
        sidebar.notice = None;
        sidebar.refresh();
        return Ok(true);
    } else if !clients
        .iter()
        .any(|client| client.name == handoff.client && client.pid == handoff.client_pid)
    {
        tmux::run(&["set-option", "-pu", "-t", own, HANDOFF])?;
    }
    Ok(false)
}

/// Keys may reach the old PTY while tmux is executing the switch. Relay those only to
/// the same live client's current sidebar. A pane marker and a unique client are required;
/// deliberate Enter focus ends the relay rather than sending any subsequent key to an agent.
fn relay_key(key: KeyEvent, own: &str, origin: &Client) -> Result<bool> {
    let clients = clients()?;
    if clients.iter().any(|client| client.pane == own) {
        return Ok(false);
    }
    let client = clients
        .iter()
        .find(|client| client.name == origin.name && client.pid == origin.pid)
        .context("navigation client disconnected; buffered key discarded")?;
    input_client(&clients, &client.pane)?;
    if tmux::run(&["show-option", "-pqv", "-t", &client.pane, MARK])?.trim() != "1" {
        bail!("client left sidebar; buffered key discarded");
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let (literal, code) = match key.code {
        KeyCode::Char(c) => (!ctrl && !alt, c.to_string()),
        KeyCode::Enter => (false, "Enter".into()),
        KeyCode::Esc => (false, "Escape".into()),
        KeyCode::Backspace => (false, "BSpace".into()),
        KeyCode::Up => (false, "Up".into()),
        KeyCode::Down => (false, "Down".into()),
        _ => return Ok(true), // Other keys have no sidebar action.
    };
    let code = format!(
        "{}{}{code}",
        if ctrl { "C-" } else { "" },
        if alt { "M-" } else { "" }
    );
    let mut args = vec!["send-keys", "-t", &client.pane];
    if literal {
        args.push("-l");
    }
    args.push(&code);
    tmux::run(&args)?;
    Ok(true)
}

fn event_loop(terminal: &mut DefaultTerminal) -> Result<Exit> {
    // This is the native sidebar process's own pane, never a caller inferred by the CLI.
    let own = std::env::var("TMUX_PANE")?;
    let mut sidebar = Sidebar::default();
    sidebar.refresh();
    sidebar.focused = clients()?.iter().any(|client| client.pane == own);
    receive_handoff(&mut sidebar, &own)?;
    let mut refreshed = Instant::now();
    let mut relay = None;
    loop {
        terminal.draw(|f| draw(f, &mut sidebar))?;
        if event::poll(Duration::from_millis(100))? {
            if receive_handoff(&mut sidebar, &own)? {
                terminal.draw(|f| draw(f, &mut sidebar))?;
            } else {
                let focused = clients()?.iter().any(|client| client.pane == own);
                if focused && !sidebar.focused {
                    sidebar.update_focus(focused);
                    sidebar.refresh();
                } else {
                    sidebar.focused = focused;
                }
            }
            let mut focus = false;
            let mut preview = false;
            // Apply the complete buffered burst before changing panes. Otherwise a fast jjj
            // would strand the second and third keys in the sidebar we just left.
            loop {
                if let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    if let Some(origin) = &relay {
                        match relay_key(key, &own, origin) {
                            Ok(true) => {
                                if !event::poll(Duration::ZERO)? {
                                    break;
                                }
                                continue;
                            }
                            Ok(false) => relay = None,
                            Err(error) => {
                                sidebar.notice = Some(error.to_string());
                                break;
                            }
                        }
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
                                    | KeyCode::Char('j' | 'k' | 'F' | 'V')
                            ));
                    if sidebar.key(key) {
                        return Ok(Exit::Quit);
                    }
                }
                if focus || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            if focus || (preview && sidebar.navigation.view && sidebar.navigation.search.is_none())
            {
                match navigate(&sidebar, &own, focus) {
                    Ok(destination) => relay = destination,
                    Err(error) => sidebar.notice = Some(error.to_string()),
                }
            }
        }
        if refreshed.elapsed() >= REFRESH {
            if alone()? {
                return Ok(Exit::Alone);
            }
            // Refresh may change selection when an agent vanishes. Only input triggers preview.
            if !receive_handoff(&mut sidebar, &own)? {
                sidebar.update_focus(clients()?.iter().any(|client| client.pane == own));
                sidebar.refresh();
            }
            refreshed = Instant::now();
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
    let [list_area, help_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let inner = list_area.width.saturating_sub(2) as usize;
    let block = panel(Line::from(" agents ")).title_top(counts(&sidebar.all_agents));
    let content = block.inner(list_area);
    f.render_widget(block, list_area);
    let [mode_area, query_area, agents_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(content);
    let mode = format!(
        " V {}{}  F {}",
        if sidebar.navigation.view { "on" } else { "off" },
        if sidebar.navigation.view && sidebar.navigation.search.is_some() {
            " · paused"
        } else {
            ""
        },
        sidebar.navigation.filter.map_or("all", State::as_str)
    );
    f.render_widget(Paragraph::new(Line::styled(mode, LAVENDER)), mode_area);
    let query = &sidebar.navigation.query;
    let search = sidebar.navigation.search.is_some();
    let query = if search {
        let tail: String = query.chars().rev().take(inner.saturating_sub(4)).collect();
        format!(" \\ {}▏", tail.chars().rev().collect::<String>())
    } else if query.is_empty() {
        " \\ search".into()
    } else {
        truncate(&format!(" \\ {query}"), inner)
    };
    f.render_widget(
        Paragraph::new(Line::styled(query, if search { TEXT } else { SUBTEXT })),
        query_area,
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
            ["no matching agents", "F changes state · esc clears search"]
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

    f.render_widget(
        Paragraph::new(footer(sidebar, inner + 2, index::now_ms())),
        help_area,
    );
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
                ("S", "sort"),
                ("r", "rename"),
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
        press(&mut sidebar, KeyCode::Char('V'));
        assert!(sidebar.navigation.view);
        press(&mut sidebar, KeyCode::Char('V'));
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
            press(&mut sidebar, KeyCode::Char('F'));
            assert_eq!(sidebar.navigation.filter, expected);
        }
        sidebar.navigation.query = "AGENT-%2".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('F'));
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        sidebar.all_agents[1].state = State::Idle;
        sidebar.rebuild();
        assert_eq!(sidebar.navigation.filter, Some(State::Done));
        assert!(sidebar.selected().is_none());
        press(&mut sidebar, KeyCode::Char('F'));
        assert_eq!(sidebar.navigation.filter, Some(State::Idle));
        assert_eq!(sidebar.selected().unwrap().pane, "%2");
        sidebar.navigation.query = "no such agent".into();
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('F'));
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
            focused: true,
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
        press(&mut sidebar, KeyCode::Char('S'));
        assert_eq!(order(&sidebar), ["%1", "%5", "%3", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
        // A new keyboard visit rebuilds current priorities after focus left.
        sidebar.update_focus(false);
        sidebar.all_agents[0].state = State::Working;
        sidebar.update_focus(true);
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%5", "%3", "%1", "%4"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%3");
    }

    #[test]
    fn session_groups_and_search_acceptance_carry_a_stable_handoff_snapshot() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle), agent("%2", State::Working)],
            focused: true,
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
        press(&mut sidebar, KeyCode::Char('\\'));
        sidebar.rebuild();
        assert_eq!(order(&sidebar), ["%3", "%2", "%1", "%4"]);
        press(&mut sidebar, KeyCode::Enter);
        assert_eq!(order(&sidebar), ["%3", "%4", "%2", "%1"]);
        assert_eq!(sidebar.selected().unwrap().pane, "%1");
        let nav: Navigation =
            serde_json::from_str(&serde_json::to_string(&sidebar.navigation).unwrap()).unwrap();
        let mut destination = Sidebar {
            all_agents: sidebar.all_agents.clone(),
            navigation: nav,
            focused: true,
            ..Sidebar::default()
        };
        destination.all_agents[2].state = State::Idle;
        destination.rebuild();
        assert_eq!(order(&destination), ["%3", "%4", "%2", "%1"]);
        assert_eq!(destination.selected().unwrap().pane, "%1");
        assert!(footer(&destination, 38, 100).to_string().contains("sort"));
    }

    #[test]
    fn search_edit_cancel_keep_clear_and_literal_keys() {
        let mut sidebar = Sidebar {
            all_agents: vec![agent("%1", State::Idle), agent("%2", State::Working)],
            ..Sidebar::default()
        };
        sidebar.rebuild();
        press(&mut sidebar, KeyCode::Char('\\'));
        for c in "PaYmEnT".chars() {
            press(&mut sidebar, KeyCode::Char(c));
        }
        assert_eq!(sidebar.agents.len(), 2);
        press(&mut sidebar, KeyCode::Enter);
        assert!(sidebar.navigation.search.is_none());
        assert_eq!(sidebar.navigation.query, "PaYmEnT");
        press(&mut sidebar, KeyCode::Char('\\'));
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
    fn accepted_handoff_preserves_query_mode_and_identity() {
        let original = Handoff {
            client: "/dev/pts/1".into(),
            client_pid: "42".into(),
            navigation: Navigation {
                view: true,
                query: "review".into(),
                pane: Some("%8".into()),
                filter: Some(State::Done),
                search: None,
                order: vec!["%8".into()],
            },
        };
        let restored: Handoff =
            serde_json::from_str(&serde_json::to_string(&original).unwrap()).unwrap();
        assert!(restored.navigation.view);
        assert_eq!(restored.navigation.filter, Some(State::Done));
        assert_eq!(restored.navigation.pane.as_deref(), Some("%8"));
        assert!(restored.navigation.search.is_none());
        assert_eq!(restored.navigation.order, vec!["%8"]);
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
