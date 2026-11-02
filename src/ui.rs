//! The agent list: every agent with its state, and a live preview of the selected one.

use crate::index::{self, Agent, State};
use crate::tmux;
use ansi_to_tui::IntoText;
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_millis(500);

fn colour(state: State) -> Color {
    match state {
        State::Blocked => Color::Rgb(0xf3, 0x8b, 0xa8),
        State::Done => Color::Rgb(0xa6, 0xe3, 0xa1),
        State::Working => Color::Rgb(0x89, 0xb4, 0xfa),
        State::Idle => Color::Rgb(0x6c, 0x70, 0x86),
    }
}

fn icon(state: State) -> &'static str {
    match state {
        State::Blocked => "◆",
        State::Done => "✓",
        State::Working => "●",
        State::Idle => "○",
    }
}

fn age(since_ms: u64) -> String {
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

    let dim = Style::default().fg(colour(State::Idle));
    let width = |f: fn(&Agent) -> &str| {
        app.agents
            .iter()
            .map(|a| f(a).chars().count())
            .max()
            .unwrap_or(0)
    };
    let name_width = width(|a| &a.name);
    let location_width = width(|a| &a.location);
    let items: Vec<ListItem> = app
        .agents
        .iter()
        .map(|a| {
            let state = Style::default().fg(colour(a.state));
            ListItem::new(Line::from(vec![
                Span::styled(format!("{} ", icon(a.state)), state),
                Span::raw(format!("{:<name_width$}  ", a.name)),
                Span::styled(
                    format!("{:<7} {:>4}  ", a.state.as_str(), age(a.state_since)),
                    state,
                ),
                Span::styled(format!("{:<location_width$}  ", a.location), dim),
                Span::raw(a.title.clone()),
            ]))
        })
        .collect();
    let title = match &app.error {
        Some(e) => format!(" {e} "),
        None if app.agents.is_empty() => " no agents running ".to_string(),
        None => format!(" agents · {} ", app.agents.len()),
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, list_area, &mut app.list);

    let preview_title = match app.selected() {
        Some(a) => match &a.detail {
            Some(detail) if a.state == State::Blocked => {
                format!(" {} · {} · {} ", a.name, detail, a.cwd)
            }
            _ => format!(" {} · {} ", a.name, a.cwd),
        },
        None => String::new(),
    };
    // Show the bottom of the screen, where the agent's prompt and status live.
    let mut lines = app.preview.lines.clone();
    while lines.last().is_some_and(|l| l.width() == 0) {
        lines.pop();
    }
    let inner_height = preview_area.height.saturating_sub(2) as usize;
    let start = lines.len().saturating_sub(inner_height);
    let preview = Paragraph::new(Text::from(lines[start..].to_vec()))
        .block(Block::default().borders(Borders::ALL).title(preview_title));
    f.render_widget(preview, preview_area);

    f.render_widget(
        Paragraph::new(Span::styled("enter jump · j/k move · q quit", dim)),
        help_area,
    );
}
