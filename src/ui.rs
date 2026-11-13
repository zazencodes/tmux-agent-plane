//! The agent list: every agent with its state, and a live preview of the selected one.

pub(crate) mod family;

use crate::index::{self, Agent, Index, State};
use crate::navigation::Client;
use crate::sidebar::{
    Origin, Sidebar, clients, confirmation_footer, input_client, keys_off, status_footer,
};
use crate::tmux;
use ansi_to_tui::IntoText;
use anyhow::{Context, Result, bail};
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Matcher, Utf32Str};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::BTreeSet;
use std::ops::{ControlFlow, Range};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(500);
/// How often the preview follows an agent while you are typing to it.
const TALK_REFRESH: Duration = Duration::from_millis(40);

pub(crate) fn colour(state: State) -> Color {
    match state {
        State::Blocked => Color::Rgb(0xf3, 0x8b, 0xa8),
        State::Done => Color::Rgb(0xa6, 0xe3, 0xa1),
        State::Working => Color::Rgb(0xcb, 0xa6, 0xf7),
        State::Idle => Color::Rgb(0x6c, 0x70, 0x86),
    }
}

pub(crate) fn icon(state: State) -> &'static str {
    match state {
        State::Blocked => "◆",
        State::Done => "✓",
        State::Working => "●",
        State::Idle => "○",
    }
}

pub(crate) fn age(since_ms: u64) -> String {
    let secs = index::now_ms().saturating_sub(since_ms) / 1000;
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        _ => format!("{}h", secs / 3600),
    }
}

/// Search matches for `query`, best first. Stable ties preserve the given order. The one search:
/// every list goes through here. Each word of the query can match any field, so a project,
/// name and description can come in any order. An empty query matches every agent.
pub(crate) fn rank(matcher: &mut Matcher, query: &str, agents: &[Agent]) -> Vec<Agent> {
    let pattern = Pattern::new(
        query,
        CaseMatching::Ignore,
        Normalization::Never,
        AtomKind::Fuzzy,
    );
    let mut buffer = Vec::new();
    let mut matches: Vec<_> = agents
        .iter()
        .filter_map(|agent| {
            let text = format!(
                "{} {} {} {}",
                agent.name,
                agent.session,
                agent.title,
                agent.detail.as_deref().unwrap_or("")
            );
            pattern
                .score(Utf32Str::new(&text, &mut buffer), matcher)
                .map(|score| (score, agent.clone()))
        })
        .collect();
    matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
    matches.into_iter().map(|(_, agent)| agent).collect()
}

/// `send-keys` arguments for a key, or None for one tmux has no name for. Plain characters go
/// as bytes: as text, a trailing `;` would end the tmux command.
fn tmux_key(key: KeyEvent) -> Option<Vec<String>> {
    let modifiers = key.modifiers;
    if let KeyCode::Char(c) = key.code
        && !modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        let mut bytes = [0; 4];
        let bytes = c.encode_utf8(&mut bytes).bytes();
        let mut args = vec!["-H".to_string()];
        args.extend(bytes.map(|byte| format!("{byte:02x}")));
        return Some(args);
    }
    let base = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(';') => "\\;".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Escape".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::Insert => "IC".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PPage".to_string(),
        KeyCode::PageDown => "NPage".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        KeyCode::BackTab => return Some(vec!["BTab".to_string()]),
        _ => return None,
    };
    let mut name = String::new();
    if modifiers.contains(KeyModifiers::CONTROL) {
        name.push_str("C-");
    }
    if modifiers.contains(KeyModifiers::ALT) {
        name.push_str("M-");
    }
    if modifiers.contains(KeyModifiers::SHIFT) && !matches!(key.code, KeyCode::Char(_)) {
        name.push_str("S-");
    }
    name.push_str(&base);
    Some(vec![name])
}

/// One tmux command for a burst of keys, in order.
fn send_keys_command(pane: &str, keys: &[KeyEvent]) -> Vec<String> {
    let mut command = Vec::new();
    for args in keys.iter().filter_map(|key| tmux_key(*key)) {
        if !command.is_empty() {
            command.push(";".to_string());
        }
        command.extend(["send-keys", "-t", pane].map(String::from));
        command.extend(args);
    }
    command
}

/// The agent a client is looking at: its own pane, or from a sidebar, the agent in that window,
/// preferring the last one used when the window has several.
fn agent_in_view(index: &Index, client: &Client) -> Option<String> {
    if index.agents.iter().any(|a| a.pane == client.pane) {
        return Some(client.pane.clone());
    }
    if !index
        .windows
        .iter()
        .any(|w| w.sidebar.as_ref() == Some(&client.pane))
    {
        return None;
    }
    let mut here = index.agents.iter().filter(|a| a.window == client.window);
    here.clone()
        .find(|a| index.last_agent.as_ref() == Some(&a.pane))
        .or_else(|| here.next())
        .map(|a| a.pane.clone())
}

/// One pane of the screen behind the picker, as it looked when the picker opened.
struct BackdropPane {
    area: Rect,
    screen: Text<'static>,
}

/// The panes the client is looking at, dimmed, where they sit on its screen. tmux cannot dim what
/// is behind a popup, so the popup covers the screen and draws it. A zoomed window shows one pane.
fn backdrop(client: &Client) -> Result<Vec<BackdropPane>> {
    let panes = tmux::run(&[
        "list-panes",
        "-t",
        &client.pane,
        "-f",
        "#{||:#{!:#{window_zoomed_flag}},#{pane_active}}",
        "-F",
        "#{pane_id} #{pane_left} #{pane_top} #{pane_width} #{pane_height} #{status} #{status-position}",
    ])?;
    panes.lines().map(parse_backdrop_pane).collect()
}

fn parse_backdrop_pane(line: &str) -> Result<BackdropPane> {
    let fields: Vec<_> = line.split(' ').collect();
    let [pane, left, top, width, height, status, position] = fields.as_slice() else {
        bail!("unexpected list-panes line: {line:?}");
    };
    let status_above = match (*status, *position) {
        ("off", _) | (_, "bottom") => 0,
        ("on", "top") => 1,
        (lines, "top") => lines.parse()?,
        _ => bail!("unexpected status line: {line:?}"),
    };
    let mut screen = tmux::capture(pane, true, None)?.into_text()?;
    dim(&mut screen);
    Ok(BackdropPane {
        area: Rect::new(
            left.parse()?,
            top.parse::<u16>()? + status_above,
            width.parse()?,
            height.parse()?,
        ),
        screen,
    })
}

/// On each span: a span's own reset would clear a dim set on its line.
fn dim(text: &mut Text<'_>) {
    for span in text.lines.iter_mut().flat_map(|line| &mut line.spans) {
        span.style = span.style.add_modifier(Modifier::DIM);
    }
}

/// The picker's panel: most of the screen, centred, with the dimmed screen around it.
fn panel_area(screen: Rect) -> Rect {
    screen.centered(Constraint::Percentage(90), Constraint::Percentage(85))
}

/// The sidebar's list and actions, in a popup with a large preview.
struct App {
    sidebar: Sidebar,
    client: String,
    backdrop: Vec<BackdropPane>,
    preview: Text<'static>,
    /// The agent your keys go to, pinned when Tab was pressed.
    talking: Option<String>,
    /// Keys read but not yet sent to that agent.
    outgoing: Vec<KeyEvent>,
    /// The agent's cursor on its own screen, when it shows one.
    cursor: Option<(u16, u16)>,
}

impl App {
    fn new(client: &str) -> Self {
        let mut sidebar = Sidebar::default();
        sidebar.opens_on_top = true;
        Self {
            sidebar,
            client: client.to_string(),
            backdrop: Vec::new(),
            preview: Text::default(),
            talking: None,
            outgoing: Vec::new(),
            cursor: None,
        }
    }

    /// Start on the agent in view, over the screen as it is. The first refresh shows any failure
    /// to read the index.
    fn open_on_agent_in_view(&mut self) -> Result<()> {
        let clients = clients()?;
        let client = input_client(&clients, Origin::Popup(&self.client))?;
        self.backdrop = backdrop(client)?;
        self.sidebar.client_session = Some(client.session.clone());
        self.sidebar.navigation.pane = index::read()
            .ok()
            .and_then(|index| agent_in_view(&index, client));
        Ok(())
    }

    fn refresh(&mut self) {
        self.sidebar.refresh(Origin::Popup(&self.client));
        self.keep_talking_to_selection();
        self.update_preview();
    }

    /// Keys only ever reach the pinned agent: if a refresh moved the selection, stop.
    fn keep_talking_to_selection(&mut self) {
        let selected = self.sidebar.selected().map(|agent| agent.pane.as_str());
        if self.talking.is_some() && self.talking.as_deref() != selected {
            self.stop_talking("agent left the list; back to the menu".to_string());
        }
    }

    fn stop_talking(&mut self, notice: String) {
        self.talking = None;
        self.outgoing.clear();
        self.cursor = None;
        self.sidebar.notice = Some(notice);
    }

    fn send_outgoing(&mut self) {
        let Some(pane) = &self.talking else {
            return;
        };
        let command = send_keys_command(pane, &self.outgoing);
        self.outgoing.clear();
        if command.is_empty() {
            return;
        }
        let command: Vec<_> = command.iter().map(String::as_str).collect();
        if let Err(error) = tmux::run(&command) {
            self.stop_talking(format!("{error:#}"));
        }
    }

    /// The agent's screen and cursor, read in one tmux command.
    fn update_talking_preview(&mut self, pane: &str) -> Result<()> {
        let output = tmux::run(&[
            "capture-pane",
            "-e",
            "-p",
            "-t",
            pane,
            ";",
            "display-message",
            "-p",
            "-t",
            pane,
            "#{cursor_flag} #{cursor_x} #{cursor_y}",
        ])?;
        let (screen, cursor) = output
            .trim_end_matches('\n')
            .rsplit_once('\n')
            .context("unexpected capture output")?;
        self.preview = screen.into_text()?;
        self.cursor = match cursor.split(' ').collect::<Vec<_>>().as_slice() {
            ["1", x, y] => Some((x.parse()?, y.parse()?)),
            ["0", _, _] => None,
            _ => bail!("unexpected cursor: {cursor:?}"),
        };
        Ok(())
    }

    fn update_preview(&mut self) {
        if let Some(pane) = self.talking.clone() {
            if let Err(error) = self.update_talking_preview(&pane) {
                self.stop_talking(format!("{error:#}"));
            }
            return;
        }
        self.preview = match self.sidebar.selected() {
            Some(agent) => match tmux::capture(&agent.pane, true, None) {
                Ok(screen) => screen
                    .into_text()
                    .unwrap_or_else(|e| Text::raw(e.to_string())),
                Err(e) => Text::raw(e.to_string()),
            },
            None => Text::default(),
        };
    }

    fn key(&mut self, key: KeyEvent) -> ControlFlow<Option<Agent>> {
        if self.talking.is_some() {
            if key.code == KeyCode::Tab {
                self.send_outgoing();
                self.talking = None;
                self.cursor = None;
            } else {
                self.outgoing.push(key);
            }
            return ControlFlow::Continue(());
        }
        let key = escape_key(key);
        self.sidebar.pressed(index::now_ms());
        if self.sidebar.action_key(key, Origin::Popup(&self.client)) {
            return if self.sidebar.left {
                ControlFlow::Break(None)
            } else {
                ControlFlow::Continue(())
            };
        }
        if key.code == KeyCode::Tab {
            if self.sidebar.editing.is_none()
                && let Some(agent) = self.sidebar.selected()
            {
                self.talking = Some(agent.pane.clone());
            }
            return ControlFlow::Continue(());
        }
        // Enter while typing a search keeps it, so `n` can go on from there.
        if (key.code == KeyCode::Enter
            && self.sidebar.editing.is_none()
            && self.sidebar.navigation.jump.is_none())
            || self.sidebar.opens_like_enter(key)
        {
            return match self.sidebar.open_selected() {
                Some(agent) => ControlFlow::Break(Some(agent)),
                None => ControlFlow::Continue(()),
            };
        }
        if self.sidebar.editing.is_none() && !self.sidebar.typing_query() {
            match key.code {
                KeyCode::Char('q') => return ControlFlow::Break(None),
                KeyCode::Esc if self.sidebar.unfiltered() => return ControlFlow::Break(None),
                _ => {}
            }
        }
        self.sidebar.key(key);
        ControlFlow::Continue(())
    }
}

pub fn run(client: &str, search: bool) -> Result<()> {
    crate::bindings::require_enabled()?;
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, client, search);
    ratatui::restore();
    let jump = result?;
    if let Some(agent) = jump {
        // Re-resolve only this immutable pane, including any new session/window.
        let index = index::read()?;
        let agent = index
            .agents
            .iter()
            .find(|a| a.pane == agent.pane)
            .context("selected agent exited; focus cancelled")?;
        tmux::run(&[
            "switch-client",
            "-c",
            client,
            "-t",
            &agent.session,
            ";",
            "select-window",
            "-t",
            &agent.window,
            ";",
            "select-pane",
            "-t",
            &agent.pane,
        ])?;
    }
    Ok(())
}

fn event_loop(terminal: &mut DefaultTerminal, client: &str, search: bool) -> Result<Option<Agent>> {
    let mut app = App::new(client);
    app.open_on_agent_in_view()?;
    if search {
        app.sidebar.start_search();
    }
    app.refresh();
    let mut refreshed = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        let wait = if app.talking.is_some() {
            TALK_REFRESH
        } else {
            Duration::from_millis(100)
        };
        let mut pressed = false;
        if event::poll(wait)? {
            // While talking, send everything already typed in one go.
            loop {
                if let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    pressed = true;
                    if let ControlFlow::Break(jump) = app.key(key) {
                        return Ok(jump);
                    }
                    if app.sidebar.restarting {
                        terminal.draw(|f| draw(f, &mut app))?;
                        app.sidebar.restart_daemon(Origin::Popup(&app.client));
                    }
                }
                if app.talking.is_none() || !event::poll(Duration::ZERO)? {
                    break;
                }
            }
            app.send_outgoing();
        }
        if pressed || app.talking.is_some() {
            app.update_preview();
        }
        if refreshed.elapsed() >= REFRESH {
            app.refresh();
            refreshed = Instant::now();
        }
    }
}

/// Colours for the group column, clear of the state colours so neither reads as the other.
const GROUP_COLOURS: [Color; 8] = [
    Color::Rgb(0x89, 0xb4, 0xfa), // blue
    Color::Rgb(0xfa, 0xb3, 0x87), // peach
    Color::Rgb(0x94, 0xe2, 0xd5), // teal
    Color::Rgb(0xf9, 0xe2, 0xaf), // yellow
    Color::Rgb(0xf5, 0xc2, 0xe7), // pink
    Color::Rgb(0x89, 0xdc, 0xeb), // sky
    Color::Rgb(0xb4, 0xbe, 0xfe), // lavender
    Color::Rgb(0xf2, 0xcd, 0xcd), // flamingo
];

/// Colour by position among all sessions, so groups differ until the palette runs out and
/// searching or filtering rows never recolours one.
pub(crate) fn group_colour(agents: &[Agent], session: &str) -> Color {
    let sessions: BTreeSet<&str> = agents.iter().map(|a| a.session.as_str()).collect();
    let position = sessions.iter().position(|s| *s == session).unwrap_or(0);
    GROUP_COLOURS[position % GROUP_COLOURS.len()]
}

fn draw(f: &mut Frame, app: &mut App) {
    for pane in &app.backdrop {
        let area = pane.area.intersection(f.area());
        f.render_widget(Paragraph::new(pane.screen.clone()), area);
    }
    let area = panel_area(f.area());
    f.render_widget(Clear, area);
    let frame = panel(list_title(&app.sidebar));
    let inner = frame.inner(area);
    f.render_widget(frame, area);
    draw_panel(f, inner, app);
}

fn draw_panel(f: &mut Frame, area: Rect, app: &mut App) {
    if app.sidebar.help.open {
        app.sidebar.help.draw(f, area);
        return;
    }
    if app.sidebar.auto_name.open {
        app.sidebar.auto_name.draw(f, area);
        return;
    }
    if app.sidebar.menu.is_open() {
        app.sidebar.draw_menu(f, area);
        return;
    }
    let sidebar = &mut app.sidebar;
    let width = area.width as usize;
    let confirmation = confirmation_footer(sidebar, width);
    let footer_height = confirmation.as_ref().map_or(1, |lines| lines.len() as u16);
    let searching = sidebar.navigation.search.is_some();
    // The search box exists only while there is a search.
    let search_height = if searching || !sidebar.navigation.query.is_empty() {
        3
    } else {
        0
    };
    let list_height = (sidebar.agents.len() as u16 + 2).clamp(3, (area.height / 3).max(3));
    let [search_area, list_area, preview_area, help_area] = Layout::vertical([
        Constraint::Length(search_height),
        Constraint::Length(list_height),
        Constraint::Min(3),
        Constraint::Length(footer_height),
    ])
    .areas(area);

    if search_height > 0 {
        let query = format!("› {}", sidebar.navigation.query);
        let cursor = Line::raw(query.as_str()).width().min(u16::MAX as usize) as u16;
        let scroll = cursor.saturating_sub(search_area.width.saturating_sub(3));
        let search = Paragraph::new(query)
            .scroll((0, scroll))
            .style(Style::default().fg(if searching { TEXT } else { SUBTEXT }))
            .block(
                panel(Line::from(" filter names / projects / descriptions ")).title_top(
                    Line::styled(
                        format!(" {} / {} ", sidebar.agents.len(), sidebar.all_agents.len()),
                        SUBTEXT,
                    )
                    .right_aligned(),
                ),
            );
        f.render_widget(search, search_area);
        if searching && app.talking.is_none() && search_area.width >= 3 && search_area.height >= 3 {
            f.set_cursor_position((search_area.x + 1 + cursor - scroll, search_area.y + 1));
        }
    }

    let selected = sidebar.list.selected();
    let renaming = sidebar
        .editing
        .as_ref()
        .map(|edit| format!("{}▏", edit.name));
    let column = |text: fn(&Agent) -> &str| {
        sidebar
            .agents
            .iter()
            .map(|a| text(a).chars().count())
            .max()
            .unwrap_or(0)
    };
    let name_width =
        column(|a| &a.name).max(renaming.as_ref().map_or(0, |typed| typed.chars().count()));
    let group_width = column(|a| &a.session);
    let row_width = list_area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = sidebar
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_selected = selected == Some(i);
            let search = &sidebar.navigation.pattern;
            let (task, task_style) = task(a);
            let mut spans = vec![
                Span::styled(if is_selected { "▌" } else { " " }, colour(a.state)),
                Span::styled(format!("{} ", icon(a.state)), colour(a.state)),
            ];
            match renaming.as_ref().filter(|_| is_selected) {
                Some(typed) => spans.push(Span::styled(
                    format!("{typed:<name_width$}  "),
                    Style::default()
                        .fg(colour(State::Done))
                        .add_modifier(Modifier::UNDERLINED),
                )),
                None => {
                    spans.extend(marked(
                        &a.name,
                        usize::MAX,
                        search,
                        name_style(a, a.visible),
                    ));
                    let pad = name_width.saturating_sub(a.name.chars().count()) + 2;
                    spans.push(Span::raw(" ".repeat(pad)));
                }
            }
            spans.push(Span::styled(
                format!("{:<7} {:>4}  ", a.state.as_str(), age(a.state_since)),
                state_style(a.state),
            ));
            spans.extend(marked(
                &a.session,
                usize::MAX,
                search,
                Style::default().fg(group_colour(&sidebar.all_agents, &a.session)),
            ));
            spans.push(Span::raw(
                " ".repeat(group_width.saturating_sub(a.session.chars().count()) + 2),
            ));
            spans.extend(marked(task, usize::MAX, search, task_style));
            let mut line = Line::from(spans);
            if is_selected {
                line = fill(line, row_width).style(Style::default().bg(Color::DarkGray));
            }
            ListItem::new(line)
        })
        .collect();
    // The popup's frame already says agents and names the workspace; the list's own title shows
    // only a state filter or search, as the sidebar's top line spells them.
    let mut title = Vec::new();
    if let Some(state) = sidebar.navigation.filter {
        title.push(format!("d {}", state.as_str()));
    }
    let typing = sidebar.navigation.jump.is_some();
    if typing || !sidebar.navigation.pattern.is_empty() {
        let cursor = if typing { "▏" } else { "" };
        title.push(format!("/ {}{cursor}", sidebar.navigation.pattern));
    }
    let title = match title.is_empty() {
        true => Line::default(),
        false => Line::from(format!(" {} ", title.join(" · "))),
    };
    let block = panel(title);
    fill_viewport(&mut sidebar.list, &items, block.inner(list_area).height);
    let list = List::new(items).block(block);
    f.render_stateful_widget(list, list_area, &mut sidebar.list);

    let block = match app.sidebar.selected() {
        Some(a) => {
            let mut title = vec![
                Span::raw(" "),
                Span::styled(a.name.clone(), Style::default().fg(colour(a.state)).bold()),
            ];
            if let (State::Blocked, Some(detail)) = (a.state, &a.detail) {
                title.push(Span::styled(format!(" · {detail}"), colour(State::Blocked)));
            }
            if app.talking.is_some() {
                title.push(Span::styled(" · typing here", LAVENDER));
            }
            title.push(Span::raw(" "));
            let block = panel(Line::from(title))
                .title_top(Line::styled(format!(" {} ", a.cwd), OVERLAY).right_aligned());
            if app.talking.is_some() {
                block.border_style(LAVENDER)
            } else {
                block
            }
        }
        None => panel(Line::from(" preview ")),
    };
    match (&app.sidebar.error, app.sidebar.selected()) {
        (Some(e), _) => daemon_down(f, preview_area, block, &app.sidebar, e),
        (None, None) => {
            let none = no_agents(&app.sidebar);
            message(
                f,
                preview_area,
                block,
                TEXT,
                &if app.sidebar.all_agents.is_empty() {
                    [none[0].as_str(), none[1].as_str()]
                } else {
                    ["no matching agents", "d changes state · f edits filter"]
                },
            )
        }
        (None, Some(selected)) if app.sidebar.navigation.family => {
            let family = family::family(&app.sidebar.all_agents, selected);
            let block = panel(Line::from(" graph "));
            let inner = block.inner(preview_area).inner(Margin::new(1, 0));
            f.render_widget(block, preview_area);
            family.render(inner, f.buffer_mut());
        }
        (None, Some(_)) => {
            // Show the bottom of the screen, where the agent's prompt and status live.
            let mut lines = app.preview.lines.clone();
            // The cursor's own row stays, even when it is blank.
            let keep = app.cursor.map_or(0, |(_, y)| usize::from(y) + 1);
            while lines.len() > keep && lines.last().is_some_and(|l| l.width() == 0) {
                lines.pop();
            }
            if lines.len() < keep {
                lines.resize(keep, Line::default());
            }
            let inner_height = preview_area.height.saturating_sub(2) as usize;
            let start = lines.len().saturating_sub(inner_height);
            let preview = Paragraph::new(Text::from(lines[start..].to_vec())).block(block);
            f.render_widget(preview, preview_area);
            if let Some((x, y)) = app.cursor
                && usize::from(y) >= start
                && x < preview_area.width.saturating_sub(2)
                && inner_height > 0
            {
                f.set_cursor_position((
                    preview_area.x + 1 + x,
                    preview_area.y + 1 + (usize::from(y) - start) as u16,
                ));
            }
        }
    }

    let footer = confirmation.map_or_else(
        || {
            let sidebar = &app.sidebar;
            Text::from(if app.talking.is_some() {
                hints(&[("tab", "back to the menu"), ("keys", "go to the agent")])
            } else if sidebar.notice.is_none() && searching {
                hints(&[
                    ("type", "filter"),
                    ("^j/^k", "browse"),
                    ("⏎", "open"),
                    ("tab", "type to agent"),
                    ("/", "freeze"),
                    ("esc", "cancel"),
                    ("^w ^u", "clear"),
                ])
            } else {
                status_footer(sidebar, width).unwrap_or_else(|| {
                    let mut line = hints(&[
                        ("w", "workspaces"),
                        ("j/k", "browse"),
                        ("/", "search"),
                        ("⏎", "focus"),
                        ("tab", "type to agent"),
                        ("?", "keys"),
                        ("c", "new"),
                        ("x", "kill"),
                        ("r", "rename"),
                        ("f", "filter"),
                        ("d", "state"),
                        ("o", "configure"),
                        ("q", "close"),
                    ]);
                    if sidebar.global_keys.off {
                        line.spans.insert(0, keys_off(&sidebar.global_keys));
                    }
                    line
                })
            })
        },
        Text::from,
    );
    f.render_widget(Paragraph::new(footer), help_area);
}

/// The message for an empty list, in the sidebar and the picker. With several workspaces it names
/// this one and how to reach the others.
pub(crate) fn no_agents(sidebar: &Sidebar) -> [String; 2] {
    match &sidebar.workspace {
        Some(workspace) if sidebar.workspaces.len() > 1 => [
            format!("no agents in {workspace}"),
            "start one in any pane here · w for workspaces".into(),
        ],
        _ => [
            "no agents running".into(),
            "start an agent in any tmux pane".into(),
        ],
    }
}

/// The list's title names the workspace it lists, `main` included, in the sidebar and picker.
pub(crate) fn list_title(sidebar: &Sidebar) -> Line<'static> {
    match &sidebar.workspace {
        Some(workspace) => Line::from(format!(" agents · {workspace} ")),
        None => Line::from(" agents "),
    }
}

// Catppuccin mocha.
pub(crate) const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub(crate) const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub(crate) const OVERLAY: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub(crate) const SURFACE2: Color = Color::Rgb(0x58, 0x5b, 0x70);
pub(crate) const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
pub(crate) const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);
/// Dark text on yellow, as Vim highlights a search.
pub(crate) const SEARCH_MATCH: Style = Style::new()
    .fg(Color::Rgb(0x1e, 0x1e, 0x2e))
    .bg(Color::Rgb(0xf9, 0xe2, 0xaf));

/// A shared heading for the sidebar's unboxed bottom sections.
pub(crate) fn section_heading(label: &str, width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled("── ", SURFACE2),
        Span::styled(format!("{label} "), Style::default().fg(SUBTEXT).bold()),
        Span::styled(
            "─".repeat(width.saturating_sub(label.chars().count() + 5)),
            SURFACE2,
        ),
    ])
}

/// A rounded, quiet frame with a bold title.
pub(crate) fn panel(title: Line<'_>) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(SURFACE2)
        .title(title.style(Style::default().fg(TEXT).bold()))
}

/// Scrolls back any rows hidden above while the viewport has blank rows below.
/// The list widget only ever scrolls as far as the selection needs, so without
/// this a list that has since gained room stays scrolled for good.
pub(crate) fn fill_viewport(list: &mut ListState, items: &[ListItem], height: u16) {
    let mut room = height as usize;
    let mut floor = items.len();
    for item in items.iter().rev() {
        if item.height() > room {
            break;
        }
        room -= item.height();
        floor -= 1;
    }
    *list.offset_mut() = list.offset().min(floor);
}

pub(crate) fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let mut out: String = s.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Where the `/` search finds `pattern` in `text`, ignoring case, as character ranges.
pub(crate) fn search_matches(text: &str, pattern: &str) -> Vec<Range<usize>> {
    let fold = |s: &str| -> Vec<char> {
        s.chars()
            .map(|c| c.to_lowercase().next().unwrap_or(c))
            .collect()
    };
    let (text, pattern) = (fold(text), fold(pattern));
    let mut found = Vec::new();
    let mut at = 0;
    while !pattern.is_empty() && at + pattern.len() <= text.len() {
        if text[at..at + pattern.len()] == pattern[..] {
            found.push(at..at + pattern.len());
            at += pattern.len();
        } else {
            at += 1;
        }
    }
    found
}

/// `text` cut to `width`, with every match of the `/` search highlighted. A match the cut hides
/// lights up the `…`, so each matching row shows it.
pub(crate) fn marked(text: &str, width: usize, pattern: &str, style: Style) -> Vec<Span<'static>> {
    let shown: Vec<char> = truncate(text, width).chars().collect();
    let visible = if text.chars().count() <= width {
        shown.len()
    } else {
        shown.len().saturating_sub(1)
    };
    let piece = |range: Range<usize>| shown[range].iter().collect::<String>();
    let mut spans = Vec::new();
    let mut at = 0;
    for found in search_matches(text, pattern) {
        let start = found.start.min(visible);
        if start < at {
            break;
        }
        let end = if found.end > visible {
            shown.len()
        } else {
            found.end
        };
        if start > at {
            spans.push(Span::styled(piece(at..start), style));
        }
        spans.push(Span::styled(piece(start..end), SEARCH_MATCH));
        at = end;
    }
    if at < shown.len() {
        spans.push(Span::styled(piece(at..shown.len()), style));
    }
    spans
}

/// The state style for an agent's status: blocked and done are bold, they want you.
pub(crate) fn state_style(state: State) -> Style {
    let style = Style::default().fg(colour(state));
    match state {
        State::Blocked | State::Done => style.bold(),
        State::Working | State::Idle => style,
    }
}

/// An agent's name: bold while it is the one in use, faded when idle.
pub(crate) fn name_style(agent: &Agent, in_use: bool) -> Style {
    let style = Style::default().fg(if agent.state == State::Idle {
        SUBTEXT
    } else {
        TEXT
    });
    if in_use { style.bold() } else { style }
}

/// What an agent is doing: a blocked agent's reason, in the colour that asks for attention, or
/// else its task.
pub(crate) fn task(agent: &Agent) -> (&str, Style) {
    match (agent.state, &agent.detail) {
        (State::Blocked, Some(detail)) => (detail, Style::default().fg(colour(State::Blocked))),
        _ => (&agent.title, Style::default().fg(SUBTEXT)),
    }
}

/// UI cancellation uses the same path for Escape and Ctrl-C.
pub(crate) fn escape_key(key: KeyEvent) -> KeyEvent {
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)
    } else {
        key
    }
}

/// Key hints for a footer: keys bright, actions dim.
pub(crate) fn hints(keys: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (key, action) in keys {
        spans.push(Span::styled(format!(" {key} "), SUBTEXT));
        spans.push(Span::styled(format!("{action} "), OVERLAY));
    }
    Line::from(spans)
}

/// Pad a line with spaces to `width`, so a background covers the whole row.
pub(crate) fn fill(mut line: Line<'_>, width: usize) -> Line<'_> {
    let pad = width.saturating_sub(line.width());
    line.spans.push(Span::raw(" ".repeat(pad)));
    line
}

/// A short message in the middle of a panel: a heading and an explanation.
pub(crate) fn message(
    f: &mut Frame,
    area: Rect,
    block: Block,
    heading_colour: Color,
    text: &[&str; 2],
) {
    let [heading, body] = text;
    centred(
        f,
        area,
        block,
        vec![
            Line::styled(
                heading.to_string(),
                Style::default().fg(heading_colour).bold(),
            ),
            Line::default(),
            Line::styled(body.to_string(), OVERLAY),
        ],
    );
}

/// What the list and the picker's preview show while the daemon is down: Enter restarts it.
pub(crate) fn daemon_down(f: &mut Frame, area: Rect, block: Block, sidebar: &Sidebar, error: &str) {
    let heading = Style::default().fg(colour(State::Blocked)).bold();
    let lines = if sidebar.restarting {
        vec![Line::styled("restarting daemon…", heading)]
    } else {
        let (title, action, detail) = match &sidebar.restart_failure {
            Some(failure) => (
                "daemon restart failed",
                "enter tries again",
                failure.as_str(),
            ),
            None => ("daemon not running", "enter restarts", error),
        };
        vec![
            Line::styled(title, heading),
            Line::default(),
            Line::styled(action, Style::default().fg(TEXT).bold()),
            Line::default(),
            Line::styled(detail.to_string(), OVERLAY),
        ]
    };
    centred(f, area, block, lines);
}

fn centred(f: &mut Frame, area: Rect, block: Block, lines: Vec<Line>) {
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let paragraph = Paragraph::new(lines).centered().wrap(Wrap { trim: true });
    // A third of the way down reads as centred, and leaves room for a long message to wrap.
    let [_, middle] =
        Layout::vertical([Constraint::Length(inner.height / 3), Constraint::Fill(1)]).areas(inner);
    f.render_widget(paragraph, middle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_matches_ignore_case_and_do_not_overlap() {
        assert_eq!(search_matches("Cedar cEDAR", "ceda"), [0..4, 6..10]);
        assert_eq!(search_matches("aaaa", "aa"), [0..2, 2..4]);
        assert!(search_matches("cedar", "").is_empty());
        assert!(search_matches("ced", "cedar").is_empty());
    }

    #[test]
    fn marked_highlights_every_match_and_a_cut_one_on_its_ellipsis() {
        let text = |spans: &[Span]| -> Vec<(String, bool)> {
            spans
                .iter()
                .map(|s| (s.content.to_string(), s.style == SEARCH_MATCH))
                .collect()
        };
        let plain = Style::default();
        let lit = |s: &str, on| (s.to_string(), on);
        assert_eq!(
            text(&marked("a Cedar b cedar", 99, "cedar", plain)),
            [
                lit("a ", false),
                lit("Cedar", true),
                lit(" b ", false),
                lit("cedar", true)
            ]
        );
        assert_eq!(text(&marked("abc", 99, "zz", plain)), [lit("abc", false)]);
        // Cut through the match, and wholly behind the cut.
        assert_eq!(
            text(&marked("abcdefgh", 4, "cde", plain)),
            [lit("ab", false), lit("c…", true)]
        );
        assert_eq!(
            text(&marked("abcdefgh", 4, "gh", plain)),
            [lit("abc", false), lit("…", true)]
        );
        // Several behind the cut still light it once.
        assert_eq!(
            text(&marked("abcdxxyx", 4, "x", plain)),
            [lit("abc", false), lit("…", true)]
        );
    }
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn agent(name: &str, pane: &str, title: &str) -> Agent {
        Agent {
            name: name.into(),
            pane: pane.into(),
            title: title.into(),
            kind: index::Kind::Pi,
            state: State::Working,
            detail: None,
            location: "work:1.1".into(),
            session: "work".into(),
            workspace: "main".into(),
            window: "@1".into(),
            cwd: "/tmp/project".into(),
            parent: None,
            parent_ended: false,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    fn app() -> App {
        let mut app = App::new("client");
        app.sidebar.all_agents = vec![
            agent("cedar-review", "%10", "Audit payments"),
            agent("quartz-build", "%11", "Ship compiler"),
        ];
        app.sidebar.all_agents[1].detail = Some("Approve cache refresh".into());
        app.sidebar.navigation.order = vec!["%10".into(), "%11".into()];
        app.sidebar.rebuild();
        app
    }

    #[test]
    fn the_screen_behind_the_panel_is_dimmed_and_stays_out_of_it() {
        let mut screen = "\x1b[31mred\x1b[0m plain".into_text().unwrap();
        dim(&mut screen);
        let mut app = App::new("client");
        app.backdrop = vec![BackdropPane {
            area: Rect::new(0, 0, 200, 1),
            screen,
        }];
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 0)].symbol(), "r");
        assert!(buffer[(0, 0)].modifier.contains(Modifier::DIM));
        assert!(buffer[(5, 0)].modifier.contains(Modifier::DIM));

        app.backdrop[0].area = Rect::new(0, 5, 200, 1);
        let mut terminal = Terminal::new(TestBackend::new(40, 20)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(0, 5)].symbol(), "r");
        assert_ne!(buffer[(3, 5)].symbol(), "d", "the panel covers the screen");
    }

    #[test]
    fn a_blocked_agents_reason_is_red_and_any_other_task_is_quiet() {
        let mut blocked = agent("a", "%1", "the task");
        blocked.state = State::Blocked;
        assert_eq!(task(&blocked), ("the task", Style::default().fg(SUBTEXT)));
        blocked.detail = Some("the reason".into());
        assert_eq!(
            task(&blocked),
            ("the reason", Style::default().fg(colour(State::Blocked)))
        );
        blocked.state = State::Idle;
        assert_eq!(task(&blocked), ("the task", Style::default().fg(SUBTEXT)));
    }

    #[test]
    fn agent_in_view_is_the_clients_pane_or_the_agent_beside_its_sidebar() {
        let mut other_window = agent("elsewhere", "%3", "");
        other_window.window = "@2".into();
        let index = Index {
            updated_at: 0,
            agents: vec![
                other_window,
                agent("first", "%1", ""),
                agent("second", "%2", ""),
            ],
            recent_event: None,
            windows: vec![index::WindowView {
                id: "@1".into(),
                active_pane: "%9".into(),
                viewed: true,
                sidebar: Some("%9".into()),
                workspace: "main".into(),
            }],
            order: Vec::new(),
            last_agent: Some("%3".into()),
            ..Index::default()
        };
        let client = |pane: &str| Client {
            name: "client".into(),
            pid: "1".into(),
            session: "$1".into(),
            window: "@1".into(),
            pane: pane.into(),
        };
        let view = |index: &Index, pane| agent_in_view(index, &client(pane));
        assert_eq!(view(&index, "%2").as_deref(), Some("%2"));
        // A shell beside an agent is not looking at it.
        assert_eq!(view(&index, "%8"), None);
        // The last agent used is elsewhere, so the sidebar's window picks its first.
        assert_eq!(view(&index, "%9").as_deref(), Some("%1"));
        let index = Index {
            last_agent: Some("%2".into()),
            tasks: Vec::new(),
            ..index
        };
        assert_eq!(view(&index, "%9").as_deref(), Some("%2"));
    }

    #[test]
    fn opens_on_the_top_row_ahead_of_agents_wanting_attention() {
        let mut app = app();
        app.sidebar.all_agents[1].state = State::Blocked;
        app.sidebar.navigation.pane = None;
        app.sidebar.rebuild();
        assert_eq!(selected(&app), "cedar-review");
    }

    fn press(app: &mut App, code: KeyCode) -> ControlFlow<Option<Agent>> {
        app.key(KeyEvent::from(code))
    }

    fn ctrl(app: &mut App, c: char) -> ControlFlow<Option<Agent>> {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// Replace the search with `query`, staying in search mode.
    fn type_query(app: &mut App, query: &str) {
        if app.sidebar.navigation.search.is_none() {
            assert!(press(app, KeyCode::Char('f')).is_continue());
        }
        let _ = ctrl(app, 'u');
        for c in query.chars() {
            assert!(press(app, KeyCode::Char(c)).is_continue());
        }
    }

    fn selected(app: &App) -> &str {
        &app.sidebar.selected().unwrap().name
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn fuzzy_queries_span_names_projects_titles_and_details_in_any_order() {
        let mut app = app();
        app.sidebar.all_agents[1].session = "zeta".into();
        for (query, expected) in [
            ("CDRRV", "cedar-review"),
            ("audpay", "cedar-review"),
            ("qrtzbldshpcmp", "quartz-build"),
            ("approvecache", "quartz-build"),
            ("zeta", "quartz-build"),
            ("compiler zeta quartz", "quartz-build"),
        ] {
            type_query(&mut app, query);
            assert_eq!(selected(&app), expected);
        }
        type_query(&mut app, "does not exist");
        assert!(app.sidebar.selected().is_none());
        assert!(press(&mut app, KeyCode::Enter).is_continue());
        assert!(press(&mut app, KeyCode::Enter).is_continue());
    }

    #[test]
    fn best_match_wins_each_edit_and_refresh_preserves_the_browsed_pane() {
        let mut app = app();
        app.sidebar
            .all_agents
            .push(agent("cedar-worker", "%12", "Review database"));
        app.sidebar.navigation.order.push("%12".into());
        type_query(&mut app, "cedar");
        assert_eq!(app.sidebar.selected().unwrap().pane, "%10");
        let _ = ctrl(&mut app, 'j');
        assert_eq!(app.sidebar.selected().unwrap().pane, "%12");
        app.sidebar.all_agents[2].title = "Review database again".into();
        app.sidebar.rebuild();
        assert_eq!(app.sidebar.selected().unwrap().pane, "%12");
        type_query(&mut app, "cedarr");
        assert_eq!(app.sidebar.selected().unwrap().pane, "%10");
        let ControlFlow::Break(Some(chosen)) = press(&mut app, KeyCode::Enter) else {
            panic!("Enter opens the selection from the search");
        };
        assert_eq!(chosen.pane, "%10");
    }

    #[test]
    fn the_list_opens_in_command_mode_and_plain_keys_browse() {
        let mut app = app();
        assert_eq!(selected(&app), "cedar-review");
        let _ = press(&mut app, KeyCode::Char('j'));
        assert_eq!(selected(&app), "quartz-build");
        let _ = press(&mut app, KeyCode::Char('k'));
        assert_eq!(selected(&app), "cedar-review");
        let _ = ctrl(&mut app, 'j');
        assert_eq!(selected(&app), "quartz-build");
        let _ = ctrl(&mut app, 'k');
        assert_eq!(selected(&app), "cedar-review");
        assert!(app.sidebar.navigation.query.is_empty());
        assert!(app.sidebar.navigation.search.is_none());
    }

    #[test]
    fn slash_starts_search_where_letters_are_text_and_ctrl_j_k_still_browse() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('f'));
        for c in "qjx東京".chars() {
            let _ = press(&mut app, KeyCode::Char(c));
        }
        let _ = press(&mut app, KeyCode::Backspace);
        assert_eq!(app.sidebar.navigation.query, "qjx東");
        type_query(&mut app, "cedar review  ");
        let _ = ctrl(&mut app, 'w');
        assert_eq!(app.sidebar.navigation.query, "cedar ");
        let _ = ctrl(&mut app, 'u');
        assert!(app.sidebar.navigation.query.is_empty());
        let first = app.sidebar.list.selected().unwrap();
        let _ = ctrl(&mut app, 'j');
        assert_eq!(app.sidebar.list.selected().unwrap(), first + 1);
        let _ = ctrl(&mut app, 'k');
        assert_eq!(app.sidebar.list.selected().unwrap(), first);
        assert!(app.sidebar.navigation.search.is_some());
    }

    #[test]
    fn slash_freezes_a_search_so_keys_act_on_its_matches_and_slash_resumes_it() {
        let mut app = app();
        app.sidebar
            .all_agents
            .push(agent("cedar-worker", "%12", "Review database"));
        app.sidebar.navigation.order.push("%12".into());
        type_query(&mut app, "cedar");
        let _ = ctrl(&mut app, 'j');
        assert_eq!(selected(&app), "cedar-worker");
        assert!(press(&mut app, KeyCode::Char('/')).is_continue());
        assert!(app.sidebar.navigation.search.is_none());
        assert_eq!(app.sidebar.navigation.query, "cedar");
        assert_eq!(selected(&app), "cedar-worker");
        assert_eq!(app.sidebar.agents.len(), 2);
        let _ = press(&mut app, KeyCode::Char('k'));
        assert_eq!(selected(&app), "cedar-review");
        assert_eq!(app.sidebar.navigation.query, "cedar");
        let _ = press(&mut app, KeyCode::Char('c'));
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("new agent in work"));
        let _ = press(&mut app, KeyCode::Esc);
        assert_eq!(app.sidebar.navigation.query, "cedar");
        let _ = press(&mut app, KeyCode::Char('f'));
        let _ = press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.sidebar.navigation.query, "cedarw");
        assert_eq!(selected(&app), "cedar-worker");
    }

    #[test]
    fn a_list_that_fits_again_scrolls_back_to_the_top() {
        let mut app = app();
        app.sidebar.all_agents = (0..20)
            .map(|i| agent(&format!("row-{i:02}"), &format!("%{i}"), "Task"))
            .collect();
        app.sidebar.navigation.order = (0..20).map(|i| format!("%{i}")).collect();
        app.sidebar.rebuild();
        let _ = press(&mut app, KeyCode::Char('G'));
        let mut short = Terminal::new(TestBackend::new(120, 20)).unwrap();
        short.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&short).contains("row-00"));

        let mut tall = Terminal::new(TestBackend::new(120, 80)).unwrap();
        tall.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&tall).contains("row-00"));
    }

    #[test]
    fn tab_sends_every_key_to_the_pinned_agent_until_tab_again() {
        let mut app = app();
        assert!(press(&mut app, KeyCode::Tab).is_continue());
        assert_eq!(app.talking.as_deref(), Some("%10"));
        // Nothing here may act on the list or close the popup, including Esc, q and Ctrl-C.
        for code in [
            KeyCode::Char('x'),
            KeyCode::Char('q'),
            KeyCode::Char('j'),
            KeyCode::Char('f'),
            KeyCode::Esc,
            KeyCode::Enter,
        ] {
            assert!(press(&mut app, code).is_continue());
        }
        assert!(ctrl(&mut app, 'c').is_continue());
        assert_eq!(app.outgoing.len(), 7);
        assert_eq!(selected(&app), "cedar-review");
        assert!(app.sidebar.navigation.search.is_none());
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("typing here"));
        assert!(screen(&terminal).contains("back to the menu"));
        // Never send: these panes belong to whatever tmux server runs the tests.
        app.outgoing.clear();
        assert!(press(&mut app, KeyCode::Tab).is_continue());
        assert!(app.talking.is_none());
        let _ = press(&mut app, KeyCode::Char('j'));
        assert_eq!(selected(&app), "quartz-build");
    }

    #[test]
    fn tab_returns_to_a_search_and_needs_an_agent() {
        let mut app = app();
        type_query(&mut app, "quartz");
        assert!(press(&mut app, KeyCode::Tab).is_continue());
        assert_eq!(app.talking.as_deref(), Some("%11"));
        let _ = press(&mut app, KeyCode::Tab);
        let _ = press(&mut app, KeyCode::Char('z'));
        assert_eq!(app.sidebar.navigation.query, "quartzz");
        type_query(&mut app, "does not exist");
        let _ = press(&mut app, KeyCode::Tab);
        assert!(app.talking.is_none());
    }

    #[test]
    fn tab_is_not_taken_during_a_rename_or_a_prompt() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('r'));
        let _ = press(&mut app, KeyCode::Tab);
        assert!(app.talking.is_none());
        let _ = press(&mut app, KeyCode::Esc);
        let _ = press(&mut app, KeyCode::Char('c'));
        let _ = press(&mut app, KeyCode::Tab);
        assert!(app.talking.is_none());
    }

    #[test]
    fn talking_stops_when_a_refresh_moves_the_selection() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Tab);
        let _ = press(&mut app, KeyCode::Char('x'));
        app.sidebar.all_agents.remove(0);
        app.sidebar.rebuild();
        app.keep_talking_to_selection();
        assert!(app.talking.is_none());
        assert!(app.outgoing.is_empty());
        assert!(
            app.sidebar
                .notice
                .as_deref()
                .unwrap()
                .contains("left the list")
        );
    }

    #[test]
    fn keys_become_exact_tmux_arguments() {
        let name = |code, modifiers| tmux_key(KeyEvent::new(code, modifiers)).unwrap();
        let none = KeyModifiers::NONE;
        assert_eq!(name(KeyCode::Char('a'), none), ["-H", "61"]);
        assert_eq!(name(KeyCode::Char('A'), KeyModifiers::SHIFT), ["-H", "41"]);
        assert_eq!(name(KeyCode::Char(';'), none), ["-H", "3b"]);
        assert_eq!(name(KeyCode::Char('é'), none), ["-H", "c3", "a9"]);
        assert_eq!(name(KeyCode::Char('c'), KeyModifiers::CONTROL), ["C-c"]);
        assert_eq!(name(KeyCode::Char('f'), KeyModifiers::ALT), ["M-f"]);
        assert_eq!(name(KeyCode::Char(';'), KeyModifiers::ALT), ["M-\\;"]);
        assert_eq!(name(KeyCode::Char(' '), KeyModifiers::CONTROL), ["C-Space"]);
        assert_eq!(name(KeyCode::Enter, none), ["Enter"]);
        assert_eq!(name(KeyCode::Enter, KeyModifiers::SHIFT), ["S-Enter"]);
        assert_eq!(name(KeyCode::Esc, none), ["Escape"]);
        assert_eq!(name(KeyCode::Backspace, none), ["BSpace"]);
        assert_eq!(name(KeyCode::BackTab, KeyModifiers::SHIFT), ["BTab"]);
        assert_eq!(name(KeyCode::Up, none), ["Up"]);
        assert_eq!(name(KeyCode::F(5), none), ["F5"]);
        assert!(tmux_key(KeyEvent::from(KeyCode::CapsLock)).is_none());
        assert_eq!(
            send_keys_command(
                "%1",
                &[
                    KeyEvent::from(KeyCode::Char('a')),
                    KeyEvent::from(KeyCode::Enter)
                ]
            ),
            [
                "send-keys",
                "-t",
                "%1",
                "-H",
                "61",
                ";",
                "send-keys",
                "-t",
                "%1",
                "Enter"
            ]
        );
    }

    #[test]
    fn esc_cancels_search_clears_a_kept_query_and_then_closes() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('j'));
        type_query(&mut app, "cedar");
        assert_eq!(selected(&app), "cedar-review");
        assert!(press(&mut app, KeyCode::Esc).is_continue());
        assert!(app.sidebar.navigation.query.is_empty());
        assert_eq!(selected(&app), "quartz-build");
        type_query(&mut app, "cedar");
        assert!(press(&mut app, KeyCode::Char('/')).is_continue());
        assert_eq!(app.sidebar.navigation.query, "cedar");
        assert!(press(&mut app, KeyCode::Esc).is_continue());
        assert!(app.sidebar.navigation.query.is_empty());
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            ControlFlow::Break(None)
        ));
    }

    #[test]
    fn slash_searches_without_hiding_and_enter_keeps_it_then_opens() {
        let mut app = app();
        let rows = app.sidebar.agents.len();
        let _ = press(&mut app, KeyCode::Char('/'));
        for c in "CEDAR".chars() {
            assert!(press(&mut app, KeyCode::Char(c)).is_continue());
        }
        assert_eq!(selected(&app), "cedar-review");
        assert_eq!(app.sidebar.agents.len(), rows);
        assert!(press(&mut app, KeyCode::Enter).is_continue());
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("/ CEDAR"));
        // Esc clears the search first, then closes.
        assert!(press(&mut app, KeyCode::Esc).is_continue());
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            ControlFlow::Break(None)
        ));
    }

    #[test]
    fn project_navigation_and_search_select_project_tops_in_the_picker() {
        let mut app = app();
        app.sidebar
            .all_agents
            .push(agent("helper", "%12", "Review output"));
        app.sidebar.navigation.order.push("%12".into());
        app.sidebar.all_agents[0].session = "agent-signals".into();
        for agent in &mut app.sidebar.all_agents[1..] {
            agent.session = "tools".into();
        }
        app.sidebar.rebuild();
        let first = app.sidebar.agents[0].pane.clone();
        let second = app.sidebar.agents[1].pane.clone();
        let _ = press(&mut app, KeyCode::Char('}'));
        assert_eq!(app.sidebar.selected().unwrap().pane, second);
        let _ = press(&mut app, KeyCode::Char('{'));
        assert_eq!(app.sidebar.selected().unwrap().pane, first);
        let _ = press(&mut app, KeyCode::Char('j'));
        let _ = press(&mut app, KeyCode::Char('g'));
        assert_ne!(app.sidebar.selected().unwrap().pane, first);
        let _ = press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.sidebar.selected().unwrap().pane, first);
        let _ = press(&mut app, KeyCode::Char('/'));
        for c in "tools".chars() {
            let _ = press(&mut app, KeyCode::Char(c));
        }
        assert_eq!(app.sidebar.selected().unwrap().pane, second);
        assert!(press(&mut app, KeyCode::Enter).is_continue());
        let _ = press(&mut app, KeyCode::Char('n'));
        assert_eq!(app.sidebar.selected().unwrap().pane, second);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let highlighted = buffer
            .content
            .iter()
            .filter(|cell| cell.bg == SEARCH_MATCH.bg.unwrap())
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert_eq!(highlighted, "toolstools");
    }

    #[test]
    fn every_search_match_lights_up_as_it_is_typed() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('/'));
        for c in "CEDAR".chars() {
            let _ = press(&mut app, KeyCode::Char(c));
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let lit = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .filter(|(x, y)| buffer[(*x, *y)].bg == SEARCH_MATCH.bg.unwrap())
            .map(|(x, y)| buffer[(x, y)].symbol())
            .collect::<String>();
        let matching = app
            .sidebar
            .agents
            .iter()
            .filter(|a| a.name.contains("cedar"))
            .count();
        assert!(matching > 0);
        assert_eq!(lit, "cedar".repeat(matching));
    }

    #[test]
    fn ctrl_c_cancels_a_search_before_it_closes_the_picker() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('/'));
        let _ = press(&mut app, KeyCode::Char('c'));
        assert!(ctrl(&mut app, 'c').is_continue());
        assert!(app.sidebar.navigation.jump.is_none());
        let _ = press(&mut app, KeyCode::Char('/'));
        let _ = press(&mut app, KeyCode::Char('c'));
        assert!(press(&mut app, KeyCode::Enter).is_continue());
        assert!(ctrl(&mut app, 'c').is_continue());
        assert!(app.sidebar.navigation.pattern.is_empty());
        assert!(matches!(ctrl(&mut app, 'c'), ControlFlow::Break(None)));
    }

    #[test]
    fn q_and_ctrl_c_close_without_a_target() {
        let mut app = app();
        assert!(matches!(
            press(&mut app, KeyCode::Char('q')),
            ControlFlow::Break(None)
        ));
        assert!(matches!(ctrl(&mut app, 'c'), ControlFlow::Break(None)));
    }

    #[test]
    fn a_state_filter_is_cleared_before_the_picker_closes() {
        for ctrl_c in [false, true] {
            let close = |app: &mut App| {
                if ctrl_c {
                    ctrl(app, 'c')
                } else {
                    press(app, KeyCode::Esc)
                }
            };
            let mut app = app();
            let _ = press(&mut app, KeyCode::Char('d'));
            assert!(app.sidebar.navigation.filter.is_some());
            assert!(close(&mut app).is_continue());
            assert!(app.sidebar.navigation.filter.is_none());
            assert!(matches!(close(&mut app), ControlFlow::Break(None)));
        }
    }

    #[test]
    fn rename_edits_in_the_row_and_its_letters_are_not_commands() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('r'));
        for c in "xq".chars() {
            assert!(press(&mut app, KeyCode::Char(c)).is_continue());
        }
        assert_eq!(app.sidebar.editing.as_ref().unwrap().name, "cedar-reviewxq");
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("cedar-reviewxq▏"));
        assert!(press(&mut app, KeyCode::Esc).is_continue());
        assert!(app.sidebar.editing.is_none());
    }

    #[test]
    fn filter_and_help_work_from_the_list() {
        let mut app = app();
        app.sidebar.all_agents[1].state = State::Blocked;
        app.sidebar.rebuild();
        let _ = press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.sidebar.navigation.filter, Some(State::Blocked));
        assert_eq!(app.sidebar.agents.len(), 1);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let text = screen(&terminal);
        assert!(text.contains(" d blocked "), "{text}");
        // "agents" once, in the frame that names the workspace.
        assert_eq!(text.matches("agents").count(), 1, "{text}");
        let _ = press(&mut app, KeyCode::Char('?'));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("Keyboard shortcuts"));
        assert!(press(&mut app, KeyCode::Char('q')).is_continue());
        assert!(!app.sidebar.help.open);
    }

    #[test]
    fn new_agent_prompt_uses_the_footer_and_consumes_keys() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('c'));
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("new agent in work"));
        assert!(press(&mut app, KeyCode::Char('q')).is_continue());
        let _ = press(&mut app, KeyCode::Esc);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("new agent in work"));
    }

    #[test]
    fn p_shows_the_selected_agents_family_in_place_of_the_preview() {
        let mut app = App::new("client");
        app.sidebar.all_agents = crate::ui::family::tests::family_agents();
        app.sidebar.navigation.order = app
            .sidebar
            .all_agents
            .iter()
            .map(|a| a.pane.clone())
            .collect();
        app.sidebar.rebuild();
        while app.sidebar.selected().unwrap().pane != "%4" {
            assert!(press(&mut app, KeyCode::Char('j')).is_continue());
        }
        assert!(press(&mut app, KeyCode::Char('p')).is_continue());
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let rows: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(100)
            .map(|row| row.iter().map(|c| c.symbol()).collect())
            .collect();
        let shown = rows.join("\n");
        assert!(shown.contains(" graph "), "{shown}");
        // api-tests is a child: its parent api-review on the left, site-lead above, ages shown.
        assert!(shown.contains("↑ site-lead"), "{shown}");
        let row = rows.iter().find(|r| r.contains("│───│")).expect(&shown);
        assert!(
            row.contains("✓ api-review 0s │───│ ◆ api-tests 0s │"),
            "{shown}"
        );
        assert!(press(&mut app, KeyCode::Char('p')).is_continue());
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let hidden: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(!hidden.contains(" graph "), "{hidden}");
    }

    #[test]
    fn group_column_names_each_session_in_a_colour_that_filtering_cannot_change() {
        let mut app = app();
        app.sidebar.all_agents[1].session = "zeta".into();
        app.sidebar.all_agents[1].location = "zeta:2.1".into();
        app.sidebar.rebuild();
        let work = group_colour(&app.sidebar.all_agents, "work");
        let zeta = group_colour(&app.sidebar.all_agents, "zeta");
        assert_ne!(work, zeta);
        for state in [State::Blocked, State::Done, State::Working, State::Idle] {
            assert!(!GROUP_COLOURS.contains(&colour(state)));
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        let shown = screen(&terminal);
        // The row names its session and leaves out the window and pane numbers.
        assert!(shown.contains("zeta") && !shown.contains("2.1"), "{shown}");
        let buffer = terminal.backend().buffer().clone();
        let cell = (0..buffer.area.height)
            .flat_map(|y| (0..buffer.area.width).map(move |x| (x, y)))
            .find(|&(x, y)| {
                (0..4).all(|i| buffer[(x + i, y)].symbol() == &"zeta"[i as usize..=i as usize])
            })
            .unwrap();
        assert_eq!(buffer[cell].fg, zeta);
        type_query(&mut app, "quartz");
        assert_eq!(group_colour(&app.sidebar.all_agents, "zeta"), zeta);
    }

    #[test]
    fn opening_in_search_mode_takes_letters_as_text_and_enter_opens_the_match() {
        let mut app = app();
        app.sidebar.start_search();
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("filter names / projects / descriptions"));
        for c in "qrtz".chars() {
            assert!(press(&mut app, KeyCode::Char(c)).is_continue());
        }
        assert_eq!(selected(&app), "quartz-build");
        let ControlFlow::Break(Some(chosen)) = press(&mut app, KeyCode::Enter) else {
            panic!("Enter opens the match");
        };
        assert_eq!(chosen.pane, "%11");
    }

    #[test]
    fn l_and_right_open_like_enter_but_type_into_a_search() {
        for code in [KeyCode::Char('l'), KeyCode::Right] {
            let mut app = app();
            let expected = app.sidebar.selected().unwrap().pane.clone();
            let ControlFlow::Break(Some(chosen)) = press(&mut app, code) else {
                panic!("{code:?} opens the selected agent");
            };
            assert_eq!(chosen.pane, expected);
        }
        let mut app = app();
        app.sidebar.start_search();
        assert!(press(&mut app, KeyCode::Char('l')).is_continue());
        assert!(press(&mut app, KeyCode::Right).is_continue());
    }

    #[test]
    fn search_box_shows_only_during_a_search() {
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        let mut app = app();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("filter names / projects / descriptions"));
        let _ = press(&mut app, KeyCode::Char('f'));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("filter names / projects / descriptions"));
        let _ = press(&mut app, KeyCode::Esc);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("filter names / projects / descriptions"));
    }

    #[test]
    fn popup_renders_empty_filtered_error_long_query_and_small_sizes() {
        for (width, height) in [(120, 35), (50, 16), (20, 8), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut app = app();
            for query in ["", "cedar", "missing", &"東京".repeat(100)] {
                type_query(&mut app, query);
                terminal.draw(|f| draw(f, &mut app)).unwrap();
                if width == 120 {
                    let shown = screen(&terminal);
                    assert!(shown.contains("filter names / projects / descriptions"));
                    if query == "missing" {
                        assert!(shown.contains("no matching agents"));
                    }
                }
            }
            app.sidebar.all_agents.clear();
            app.sidebar.rebuild();
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.sidebar.error = Some("stale agent index".into());
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
