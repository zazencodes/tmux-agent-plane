//! The agent list: every agent with its state, and a live preview of the selected one.

use crate::index::{self, Agent, State};
use crate::tmux;
use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, BorderType, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{DefaultTerminal, Frame};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(500);

pub(crate) fn colour(state: State) -> Color {
    match state {
        State::Blocked => Color::Rgb(0xf3, 0x8b, 0xa8),
        State::Done => Color::Rgb(0xa6, 0xe3, 0xa1),
        State::Working => Color::Rgb(0x89, 0xb4, 0xfa),
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
    agents: Vec<Agent>,
    list: ListState,
    preview: Text<'static>,
    error: Option<String>,
}

impl App {
    fn selected(&self) -> Option<&Agent> {
        self.list.selected().and_then(|i| self.agents.get(i))
    }

    fn refresh(&mut self) {
        let keep = self.selected().map(|a| a.pane.clone());
        match index::read() {
            Ok(index) => {
                self.agents = index.agents;
                self.error = None;
            }
            Err(e) => {
                self.agents.clear();
                self.error = Some(e.to_string());
            }
        }
        let selected = match keep {
            Some(pane) => self.agents.iter().position(|a| a.pane == pane),
            // First open: start on whatever needs attention.
            None => [State::Blocked, State::Done]
                .iter()
                .find_map(|s| self.agents.iter().position(|a| a.state == *s)),
        };
        self.list.select(selected.or(if self.agents.is_empty() {
            None
        } else {
            Some(0)
        }));
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

    fn step(&mut self, delta: isize) {
        if self.agents.is_empty() {
            return;
        }
        let len = self.agents.len() as isize;
        let i = self.list.selected().unwrap_or(0) as isize;
        self.list.select(Some((i + delta).rem_euclid(len) as usize));
    }
}

pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    ratatui::restore();
    let jump = result?;
    if let Some(agent) = jump {
        tmux::jump(&agent.session, &agent.window, &agent.pane)?;
    }
    Ok(())
}

fn event_loop(terminal: &mut DefaultTerminal) -> Result<Option<Agent>> {
    let mut app = App {
        agents: Vec::new(),
        list: ListState::default(),
        preview: Text::default(),
        error: None,
    };
    app.refresh();
    let mut refreshed = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &mut app))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(None),
                KeyCode::Enter => return Ok(app.selected().cloned()),
                KeyCode::Char('j') | KeyCode::Down => app.step(1),
                KeyCode::Char('k') | KeyCode::Up => app.step(-1),
                _ => continue,
            }
            app.refresh();
            refreshed = Instant::now();
        }
        if refreshed.elapsed() >= REFRESH {
            app.refresh();
            refreshed = Instant::now();
        }
    }
}

fn draw(f: &mut Frame, app: &mut App) {
    let list_height = (app.agents.len() as u16 + 2).clamp(3, f.area().height / 3);
    let [list_area, preview_area, help_area] = Layout::vertical([
        Constraint::Length(list_height),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(f.area());

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
                Span::styled(a.title.clone(), SUBTEXT),
            ]);
            if is_selected {
                line = fill(line, row_width).style(Style::default().bg(SURFACE0));
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
            &[
                "no agents running",
                "start claude or codex in any tmux pane",
            ],
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
        Paragraph::new(hints(&[("⏎", "jump"), ("j/k", "move"), ("q", "quit")])),
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
