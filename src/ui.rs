//! The agent list: every agent with its state, and a live preview of the selected one.

use crate::index::{self, Agent, State};
use crate::tmux;
use ansi_to_tui::IntoText;
use anyhow::{Context, Result};
use nucleo_matcher::pattern::{Atom, AtomKind, CaseMatching, Normalization};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
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

struct App {
    all_agents: Vec<Agent>,
    agents: Vec<Agent>,
    query: String,
    matcher: Matcher,
    list: ListState,
    preview: Text<'static>,
    error: Option<String>,
}

impl App {
    fn new() -> Self {
        Self {
            all_agents: Vec::new(),
            agents: Vec::new(),
            query: String::new(),
            matcher: Matcher::new(Config::DEFAULT),
            list: ListState::default(),
            preview: Text::default(),
            error: None,
        }
    }

    fn selected(&self) -> Option<&Agent> {
        self.list.selected().and_then(|i| self.agents.get(i))
    }

    fn refresh(&mut self) {
        match index::read() {
            Ok(index) => {
                self.all_agents = index.agents;
                self.error = None;
            }
            Err(e) => {
                self.all_agents.clear();
                self.error = Some(e.to_string());
            }
        }
        self.filter(true);
        self.update_preview();
    }

    fn filter(&mut self, preserve_selection: bool) {
        let keep = preserve_selection
            .then(|| self.selected().map(|a| a.pane.clone()))
            .flatten();
        let pattern = Atom::new(
            &self.query,
            CaseMatching::Ignore,
            Normalization::Never,
            AtomKind::Fuzzy,
            false,
        );
        let mut buffer = Vec::new();
        let mut matches: Vec<_> = self
            .all_agents
            .iter()
            .filter_map(|agent| {
                let text = format!(
                    "{} {} {}",
                    agent.name,
                    agent.title,
                    agent.detail.as_deref().unwrap_or("")
                );
                pattern
                    .score(Utf32Str::new(&text, &mut buffer), &mut self.matcher)
                    .map(|score| (score, agent.clone()))
            })
            .collect();
        if !self.query.is_empty() {
            // Stable ties preserve the index order; every edit selects the best match.
            matches.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        self.agents = matches.into_iter().map(|(_, agent)| agent).collect();
        let selected = match keep {
            Some(pane) => self.agents.iter().position(|a| a.pane == pane),
            // First open: start on whatever needs attention.
            None if self.query.is_empty() => [State::Blocked, State::Done]
                .iter()
                .find_map(|s| self.agents.iter().position(|a| a.state == *s)),
            None => None,
        };
        self.list.select(selected.or(if self.agents.is_empty() {
            None
        } else {
            Some(0)
        }));
    }

    fn update_preview(&mut self) {
        self.preview = match self.selected() {
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
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return ControlFlow::Break(None),
            KeyCode::Char('c') if ctrl => return ControlFlow::Break(None),
            KeyCode::Enter => {
                if let Some(agent) = self.selected() {
                    return ControlFlow::Break(Some(agent.clone()));
                }
            }
            KeyCode::Down => self.step(1),
            KeyCode::Up => self.step(-1),
            KeyCode::Char('u') if ctrl => {
                self.query.clear();
                self.filter(false);
            }
            KeyCode::Char('w') if ctrl => {
                self.query = self.query.trim_end().to_string();
                while self
                    .query
                    .chars()
                    .last()
                    .is_some_and(|c| !c.is_whitespace())
                {
                    self.query.pop();
                }
                self.filter(false);
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.filter(false);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.query.push(c);
                self.filter(false);
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    fn step(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let len = self.agents.len() as isize;
        let i = self.list.selected().unwrap_or(0) as isize;
        self.list.select(Some((i + delta).rem_euclid(len) as usize));
    }
}

pub fn run(client: &str) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
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

fn event_loop(terminal: &mut DefaultTerminal) -> Result<Option<Agent>> {
    let mut app = App::new();
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
            app.update_preview();
        }
        if refreshed.elapsed() >= REFRESH {
            app.refresh();
            refreshed = Instant::now();
        }
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let list_height = (app.agents.len() as u16 + 2).clamp(3, (f.area().height / 3).max(3));
    let [search_area, list_area, preview_area, help_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(list_height),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());

    let query = format!("› {}", app.query);
    let cursor = Line::raw(query.as_str()).width().min(u16::MAX as usize) as u16;
    let scroll = cursor.saturating_sub(search_area.width.saturating_sub(3));
    let search = Paragraph::new(query)
        .scroll((0, scroll))
        .style(Style::default().fg(TEXT))
        .block(
            panel(Line::from(" search names / descriptions ")).title_top(
                Line::styled(
                    format!(" {} / {} ", app.agents.len(), app.all_agents.len()),
                    SUBTEXT,
                )
                .right_aligned(),
            ),
        );
    f.render_widget(search, search_area);
    if search_area.width >= 3 && search_area.height >= 3 {
        f.set_cursor_position((search_area.x + 1 + cursor - scroll, search_area.y + 1));
    }

    let width = |f: fn(&Agent) -> &str| {
        app.agents
            .iter()
            .map(|a| f(a).chars().count())
            .max()
            .unwrap_or(0)
    };
    let name_width = width(|a| &a.name);
    let location_width = width(|a| &a.location);
    let row_width = list_area.width.saturating_sub(2) as usize;
    let selected = app.list.selected();
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let is_selected = selected == Some(i);
            let mut line = Line::from(vec![
                Span::styled(if is_selected { "▌" } else { " " }, colour(a.state)),
                Span::styled(format!("{} ", icon(a.state)), colour(a.state)),
                Span::styled(format!("{:<name_width$}  ", a.name), name_style(a)),
                Span::styled(
                    format!("{:<7} {:>4}  ", a.state.as_str(), age(a.state_since)),
                    state_style(a.state),
                ),
                Span::styled(format!("{:<location_width$}  ", a.location), OVERLAY),
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
    let list = List::new(items).block(panel(Line::from(" agents ")).title_top(counts(&app.agents)));
    f.render_stateful_widget(list, list_area, &mut app.list);

    let block = match app.selected() {
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
    match (&app.error, app.selected()) {
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
            &if app.all_agents.is_empty() {
                ["no agents running", "start an agent in any tmux pane"]
            } else {
                [
                    "no matching agents",
                    "edit your search or press Ctrl-U to clear",
                ]
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

    f.render_widget(
        Paragraph::new(hints(&[
            ("type", "search"),
            ("↑/↓", "browse"),
            ("⏎", "focus"),
            ("esc", "cancel"),
        ])),
        help_area,
    );
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
        let mut app = App::new();
        app.all_agents = vec![
            agent("cedar-review", "%10", "Audit payments"),
            agent("quartz-build", "%11", "Ship compiler"),
        ];
        app.all_agents[1].detail = Some("Approve cache refresh".into());
        app.filter(false);
        app
    }

    fn type_query(app: &mut App, query: &str) {
        let _ = app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for c in query.chars() {
            assert!(app.key(KeyEvent::from(KeyCode::Char(c))).is_continue());
        }
    }

    #[test]
    fn fuzzy_queries_span_names_titles_and_details_and_ignore_case() {
        let mut app = app();
        for (query, pane) in [
            ("CDRRV", "%10"),
            ("adpym", "%10"),
            ("qbuildshpcmp", "%11"),
            ("apcchrfr", "%11"),
        ] {
            type_query(&mut app, query);
            assert_eq!(app.selected().unwrap().pane, pane, "{query}");
        }
        app.all_agents[0].title = "Révision 東京".into();
        type_query(&mut app, "rév東京");
        assert_eq!(app.selected().unwrap().pane, "%10");
        type_query(&mut app, "zzzz");
        assert!(app.selected().is_none());
        assert!(app.key(KeyEvent::from(KeyCode::Enter)).is_continue());
    }

    #[test]
    fn best_match_wins_each_edit_and_refresh_preserves_the_browsed_pane() {
        let mut app = app();
        app.all_agents = vec![
            agent("sparse", "%10", "p-a-y"),
            agent("payments", "%11", "Ship"),
        ];
        type_query(&mut app, "pay");
        assert_eq!(app.selected().unwrap().pane, "%11");
        let _ = app.key(KeyEvent::from(KeyCode::Down));
        assert_eq!(app.selected().unwrap().pane, "%10");
        app.all_agents.reverse();
        app.all_agents[1].name = "renamed".into();
        app.filter(true);
        assert_eq!(app.selected().unwrap().pane, "%10");
        let ControlFlow::Break(Some(chosen)) = app.key(KeyEvent::from(KeyCode::Enter)) else {
            panic!("expected focus");
        };
        assert_eq!(chosen.pane, "%10");
        app.all_agents.retain(|a| a.pane != "%10");
        app.filter(true);
        assert_eq!(app.selected().unwrap().pane, "%11");
    }

    #[test]
    fn letters_are_text_editing_is_unicode_safe_and_cancel_has_no_target() {
        let mut app = app();
        type_query(&mut app, "qjk/東京");
        let _ = app.key(KeyEvent::from(KeyCode::Backspace));
        assert_eq!(app.query, "qjk/東");
        type_query(&mut app, "cedar review  ");
        let _ = app.key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        assert_eq!(app.query, "cedar ");
        let _ = app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        assert!(app.query.is_empty());
        assert!(matches!(
            app.key(KeyEvent::from(KeyCode::Esc)),
            ControlFlow::Break(None)
        ));
        assert!(matches!(
            app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            ControlFlow::Break(None)
        ));
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
                    let screen = terminal
                        .backend()
                        .buffer()
                        .content
                        .iter()
                        .map(|c| c.symbol())
                        .collect::<String>();
                    assert!(screen.contains("search names / descriptions"));
                    if query == "missing" {
                        assert!(screen.contains("no matching agents"));
                    }
                }
            }
            app.all_agents.clear();
            app.filter(false);
            terminal.draw(|f| draw(f, &mut app)).unwrap();
            app.error = Some("stale agent index".into());
            terminal.draw(|f| draw(f, &mut app)).unwrap();
        }
    }
}
