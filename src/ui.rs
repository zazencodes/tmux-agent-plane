//! The agent list: every agent with its state, and a live preview of the selected one.

use crate::index::{self, Agent, State};
use crate::sidebar::{Origin, Sidebar, confirmation_footer, status_footer};
use crate::tmux;
use ansi_to_tui::IntoText;
use anyhow::{Context, Result};
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, List, ListItem, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::collections::BTreeSet;
use std::ops::ControlFlow;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(500);

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

/// Search matches for `query`, best first. Stable ties preserve the given order.
pub(crate) fn rank(matcher: &mut Matcher, query: &str, agents: &[Agent]) -> Vec<Agent> {
    let pattern = Atom::new(
        query,
        CaseMatching::Ignore,
        Normalization::Never,
        AtomKind::Fuzzy,
        false,
    );
    let mut buffer = Vec::new();
    let mut matches: Vec<_> = agents
        .iter()
        .filter_map(|agent| {
            let text = format!(
                "{} {} {}",
                agent.name,
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

/// The sidebar's list and actions, in a popup with a large preview.
struct App {
    sidebar: Sidebar,
    client: String,
    preview: Text<'static>,
}

impl App {
    fn new(client: &str) -> Self {
        let mut sidebar = Sidebar::default();
        sidebar.ranker = Some(Matcher::new(Config::DEFAULT));
        Self {
            sidebar,
            client: client.to_string(),
            preview: Text::default(),
        }
    }

    fn refresh(&mut self) {
        self.sidebar.refresh(Origin::Popup(&self.client));
        self.update_preview();
    }

    fn update_preview(&mut self) {
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
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return ControlFlow::Break(None);
        }
        self.sidebar.pressed(index::now_ms());
        if self.sidebar.action_key(key, Origin::Popup(&self.client)) {
            return if self.sidebar.left {
                ControlFlow::Break(None)
            } else {
                ControlFlow::Continue(())
            };
        }
        // `/` again freezes the search: its matches stay, and the keys are commands again.
        if self.sidebar.navigation.search.is_some()
            && key.code == KeyCode::Char('/')
            && key.modifiers.is_empty()
        {
            self.sidebar.key(KeyEvent::from(KeyCode::Enter));
            return ControlFlow::Continue(());
        }
        if self.sidebar.editing.is_none() && self.sidebar.navigation.search.is_none() {
            match key.code {
                KeyCode::Enter => {
                    if let Some(agent) = self.sidebar.selected() {
                        return ControlFlow::Break(Some(agent.clone()));
                    }
                    return ControlFlow::Continue(());
                }
                KeyCode::Char('q') => return ControlFlow::Break(None),
                KeyCode::Esc if self.sidebar.navigation.query.is_empty() => {
                    return ControlFlow::Break(None);
                }
                // The popup's preview is always on.
                KeyCode::Char('v') => return ControlFlow::Continue(()),
                _ => {}
            }
        }
        self.sidebar.key(key);
        ControlFlow::Continue(())
    }
}

pub fn run(client: &str) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, client);
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

fn event_loop(terminal: &mut DefaultTerminal, client: &str) -> Result<Option<Agent>> {
    let mut app = App::new(client);
    app.refresh();
    let mut refreshed = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if let ControlFlow::Break(jump) = app.key(key) {
                return Ok(jump);
            }
            if let Some(order) = app.sidebar.unsent_order.take() {
                // On screen first: the daemon's control thread answers within its poll interval.
                terminal.draw(|f| draw(f, &mut app))?;
                app.sidebar.send_order(&order);
            }
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
fn group_colour(agents: &[Agent], session: &str) -> Color {
    let sessions: BTreeSet<&str> = agents.iter().map(|a| a.session.as_str()).collect();
    let position = sessions.iter().position(|s| *s == session).unwrap_or(0);
    GROUP_COLOURS[position % GROUP_COLOURS.len()]
}

/// The window and pane: the group column already names the session.
fn place(agent: &Agent) -> &str {
    agent
        .location
        .strip_prefix(&format!("{}:", agent.session))
        .unwrap_or(&agent.location)
}

fn draw(f: &mut Frame, app: &mut App) {
    if app.sidebar.help.open {
        app.sidebar.help.draw(f);
        return;
    }
    if app.sidebar.auto_name.open {
        app.sidebar.auto_name.draw(f);
        return;
    }
    let sidebar = &mut app.sidebar;
    let width = f.area().width as usize;
    let confirmation = confirmation_footer(sidebar, width);
    let footer_height = confirmation.as_ref().map_or(1, |lines| lines.len() as u16);
    let searching = sidebar.navigation.search.is_some();
    // The search box exists only while there is a search.
    let search_height = if searching || !sidebar.navigation.query.is_empty() {
        3
    } else {
        0
    };
    let list_height = (sidebar.agents.len() as u16 + 2).clamp(3, (f.area().height / 3).max(3));
    let [search_area, list_area, preview_area, help_area] = Layout::vertical([
        Constraint::Length(search_height),
        Constraint::Length(list_height),
        Constraint::Min(3),
        Constraint::Length(footer_height),
    ])
    .areas(f.area());

    if search_height > 0 {
        let query = format!("› {}", sidebar.navigation.query);
        let cursor = Line::raw(query.as_str()).width().min(u16::MAX as usize) as u16;
        let scroll = cursor.saturating_sub(search_area.width.saturating_sub(3));
        let search = Paragraph::new(query)
            .scroll((0, scroll))
            .style(Style::default().fg(if searching { TEXT } else { SUBTEXT }))
            .block(
                panel(Line::from(" search names / descriptions ")).title_top(
                    Line::styled(
                        format!(" {} / {} ", sidebar.agents.len(), sidebar.all_agents.len()),
                        SUBTEXT,
                    )
                    .right_aligned(),
                ),
            );
        f.render_widget(search, search_area);
        if searching && search_area.width >= 3 && search_area.height >= 3 {
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
    let place_width = column(place);
    let row_width = list_area.width.saturating_sub(2) as usize;
    let items: Vec<ListItem> = sidebar
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_selected = selected == Some(i);
            let name = match renaming.as_ref().filter(|_| is_selected) {
                Some(typed) => Span::styled(
                    format!("{typed:<name_width$}  "),
                    Style::default()
                        .fg(colour(State::Done))
                        .add_modifier(Modifier::UNDERLINED),
                ),
                None => Span::styled(format!("{:<name_width$}  ", a.name), name_style(a)),
            };
            let mut line = Line::from(vec![
                Span::styled(if is_selected { "▌" } else { " " }, colour(a.state)),
                Span::styled(format!("{} ", icon(a.state)), colour(a.state)),
                name,
                Span::styled(
                    format!("{:<7} {:>4}  ", a.state.as_str(), age(a.state_since)),
                    state_style(a.state),
                ),
                Span::styled(
                    format!("{:<group_width$} ", a.session),
                    group_colour(&sidebar.all_agents, &a.session),
                ),
                Span::styled(format!("{:<place_width$}  ", place(a)), OVERLAY),
                Span::styled(
                    if a.state == State::Blocked {
                        a.detail.as_deref().unwrap_or(&a.title).to_string()
                    } else {
                        a.title.clone()
                    },
                    SUBTEXT,
                ),
            ]);
            if is_selected {
                line = fill(line, row_width).style(Style::default().bg(Color::DarkGray));
            }
            ListItem::new(line)
        })
        .collect();
    let title = match sidebar.navigation.filter {
        Some(state) => format!(" agents · {} ", state.as_str()),
        None => " agents ".to_string(),
    };
    let list = List::new(items).block(panel(Line::from(title)).title_top(counts(&sidebar.agents)));
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
            title.push(Span::raw(" "));
            panel(Line::from(title))
                .title_top(Line::styled(format!(" {} ", a.cwd), OVERLAY).right_aligned())
        }
        None => panel(Line::from(" preview ")),
    };
    match (&app.sidebar.error, app.sidebar.selected()) {
        (Some(e), _) => message(
            f,
            preview_area,
            block,
            colour(State::Blocked),
            &["daemon not running", e],
        ),
        (None, None) => message(
            f,
            preview_area,
            block,
            TEXT,
            &if app.sidebar.all_agents.is_empty() {
                ["no agents running", "start an agent in any tmux pane"]
            } else {
                ["no matching agents", "f changes state · / edits search"]
            },
        ),
        (None, Some(_)) => {
            // Show the bottom of the screen, where the agent's prompt and status live.
            let mut lines = app.preview.lines.clone();
            while lines.last().is_some_and(|l| l.width() == 0) {
                lines.pop();
            }
            let inner_height = preview_area.height.saturating_sub(2) as usize;
            let start = lines.len().saturating_sub(inner_height);
            let preview = Paragraph::new(Text::from(lines[start..].to_vec())).block(block);
            f.render_widget(preview, preview_area);
        }
    }

    let footer = confirmation.map_or_else(
        || {
            let sidebar = &app.sidebar;
            Text::from(if sidebar.notice.is_none() && searching {
                hints(&[
                    ("type", "search"),
                    ("^j/^k", "browse"),
                    ("/ ⏎", "freeze"),
                    ("esc", "cancel"),
                    ("^w ^u", "clear"),
                ])
            } else {
                status_footer(sidebar, width, index::now_ms()).unwrap_or_else(|| {
                    hints(&[
                        ("j/k", "browse"),
                        ("/", "search"),
                        ("⏎", "focus"),
                        ("?", "keys"),
                        ("n", "new"),
                        ("x", "kill"),
                        ("r", "rename"),
                        ("o", "organize"),
                        ("f", "filter"),
                        ("s", "sort"),
                        ("c", "configure"),
                        ("q", "close"),
                    ])
                })
            })
        },
        Text::from,
    );
    f.render_widget(Paragraph::new(footer), help_area);
}

// Catppuccin mocha.
pub(crate) const SUBTEXT: Color = Color::Rgb(0xa6, 0xad, 0xc8);
pub(crate) const TEXT: Color = Color::Rgb(0xcd, 0xd6, 0xf4);
pub(crate) const OVERLAY: Color = Color::Rgb(0x6c, 0x70, 0x86);
pub(crate) const SURFACE2: Color = Color::Rgb(0x58, 0x5b, 0x70);
pub(crate) const SURFACE0: Color = Color::Rgb(0x31, 0x32, 0x44);
pub(crate) const LAVENDER: Color = Color::Rgb(0xb4, 0xbe, 0xfe);

/// A rounded, quiet frame with a bold title.
pub(crate) fn panel(title: Line<'_>) -> Block<'_> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(SURFACE2)
        .title(title.style(Style::default().fg(TEXT).bold()))
}

/// The state style for an agent's status: blocked and done are bold, they want you.
pub(crate) fn state_style(state: State) -> Style {
    let style = Style::default().fg(colour(state));
    match state {
        State::Blocked | State::Done => style.bold(),
        State::Working | State::Idle => style,
    }
}

/// An agent's name: bold while it is on screen, faded when idle.
pub(crate) fn name_style(agent: &Agent) -> Style {
    let style = Style::default().fg(if agent.state == State::Idle {
        SUBTEXT
    } else {
        TEXT
    });
    if agent.visible { style.bold() } else { style }
}

/// Agent counts by state, right-aligned for a title bar; states with none are left out.
pub(crate) fn counts(agents: &[Agent]) -> Line<'static> {
    let mut spans = Vec::new();
    for state in [State::Blocked, State::Done, State::Working, State::Idle] {
        let n = agents.iter().filter(|a| a.state == state).count();
        if n > 0 {
            spans.push(Span::styled(format!(" {} {n}", icon(state)), colour(state)));
        }
    }
    if !spans.is_empty() {
        spans.push(Span::raw(" "));
    }
    Line::from(spans).right_aligned()
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
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let [heading, body] = text;
    let paragraph = Paragraph::new(vec![
        Line::styled(
            heading.to_string(),
            Style::default().fg(heading_colour).bold(),
        ),
        Line::default(),
        Line::styled(body.to_string(), OVERLAY),
    ])
    .centered()
    .wrap(Wrap { trim: true });
    // A third of the way down reads as centred, and leaves room for a long message to wrap.
    let [_, middle] =
        Layout::vertical([Constraint::Length(inner.height / 3), Constraint::Fill(1)]).areas(inner);
    f.render_widget(paragraph, middle);
}

#[cfg(test)]
mod tests {
    use super::*;
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
            window: "@1".into(),
            cwd: "/tmp/project".into(),
            parent: None,
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

    fn press(app: &mut App, code: KeyCode) -> ControlFlow<Option<Agent>> {
        app.key(KeyEvent::from(code))
    }

    fn ctrl(app: &mut App, c: char) -> ControlFlow<Option<Agent>> {
        app.key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    /// Replace the search with `query`, staying in search mode.
    fn type_query(app: &mut App, query: &str) {
        if app.sidebar.navigation.search.is_none() {
            assert!(press(app, KeyCode::Char('/')).is_continue());
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
    fn fuzzy_queries_span_names_titles_and_details_and_ignore_case() {
        let mut app = app();
        for (query, expected) in [
            ("CDRRV", "cedar-review"),
            ("audpay", "cedar-review"),
            ("qrtzbldshpcmp", "quartz-build"),
            ("approvecache", "quartz-build"),
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
        let _ = press(&mut app, KeyCode::Enter);
        let ControlFlow::Break(Some(chosen)) = press(&mut app, KeyCode::Enter) else {
            panic!("second Enter focuses the selection");
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
        let _ = press(&mut app, KeyCode::Char('/'));
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
        let _ = press(&mut app, KeyCode::Char('n'));
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("new agent in work"));
        let _ = press(&mut app, KeyCode::Esc);
        assert_eq!(app.sidebar.navigation.query, "cedar");
        let _ = press(&mut app, KeyCode::Char('/'));
        let _ = press(&mut app, KeyCode::Char('w'));
        assert_eq!(app.sidebar.navigation.query, "cedarw");
        assert_eq!(selected(&app), "cedar-worker");
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
        assert!(press(&mut app, KeyCode::Enter).is_continue());
        assert_eq!(app.sidebar.navigation.query, "cedar");
        assert!(press(&mut app, KeyCode::Esc).is_continue());
        assert!(app.sidebar.navigation.query.is_empty());
        assert!(matches!(
            press(&mut app, KeyCode::Esc),
            ControlFlow::Break(None)
        ));
    }

    #[test]
    fn q_and_ctrl_c_close_without_a_target_and_v_is_inert() {
        let mut app = app();
        assert!(press(&mut app, KeyCode::Char('v')).is_continue());
        assert!(!app.sidebar.navigation.view);
        assert!(matches!(
            press(&mut app, KeyCode::Char('q')),
            ControlFlow::Break(None)
        ));
        let _ = press(&mut app, KeyCode::Char('/'));
        assert!(matches!(ctrl(&mut app, 'c'), ControlFlow::Break(None)));
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
        let _ = press(&mut app, KeyCode::Char('f'));
        assert_eq!(app.sidebar.navigation.filter, Some(State::Blocked));
        assert_eq!(app.sidebar.agents.len(), 1);
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("agents · blocked"));
        let _ = press(&mut app, KeyCode::Char('?'));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("Keyboard shortcuts"));
        assert!(press(&mut app, KeyCode::Char('q')).is_continue());
        assert!(!app.sidebar.help.open);
    }

    #[test]
    fn new_agent_prompt_uses_the_footer_and_consumes_keys() {
        let mut app = app();
        let _ = press(&mut app, KeyCode::Char('n'));
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("new agent in work"));
        assert!(press(&mut app, KeyCode::Char('q')).is_continue());
        let _ = press(&mut app, KeyCode::Esc);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("new agent in work"));
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
        assert!(shown.contains("zeta 2.1") && !shown.contains("zeta:2.1"));
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
    fn search_box_shows_only_during_a_search() {
        let mut terminal = Terminal::new(TestBackend::new(120, 35)).unwrap();
        let mut app = app();
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("search names / descriptions"));
        let _ = press(&mut app, KeyCode::Char('/'));
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(screen(&terminal).contains("search names / descriptions"));
        let _ = press(&mut app, KeyCode::Esc);
        terminal.draw(|f| draw(f, &mut app)).unwrap();
        assert!(!screen(&terminal).contains("search names / descriptions"));
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
                    assert!(shown.contains("search names / descriptions"));
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
