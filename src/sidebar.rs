//! A narrow, always-on agent list in a pane down the side of a window.

use crate::index::{self, Agent, State};
use crate::tmux;
use crate::ui::{age, colour, icon};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame};
use std::time::{Duration, Instant};

const WIDTH: &str = "38";
const REFRESH: Duration = Duration::from_millis(500);
/// Pane option marking a sidebar pane, so the toggle finds it.
const MARK: &str = "@agent-plane-sidebar";

/// The sidebar key: open the window's sidebar and move into it, move into it when it is
/// already open, or close it when you are in it.
pub fn toggle(window: &str) -> Result<()> {
    let panes = tmux::run(&[
        "list-panes",
        "-t",
        window,
        "-F",
        &format!("#{{pane_id}} #{{pane_active}} #{{{MARK}}}"),
    ])?;
    let sidebar = panes.lines().find_map(|line| {
        let mut fields = line.split(' ');
        let (pane, active, mark) = (fields.next()?, fields.next()?, fields.next()?);
        (mark == "1").then(|| (pane.to_string(), active == "1"))
    });
    match sidebar {
        Some((pane, true)) => tmux::run(&["kill-pane", "-t", &pane]).map(drop),
        Some((pane, false)) => tmux::run(&["select-pane", "-t", &pane]).map(drop),
        None => {
            let pane = tmux::run(&[
                "split-window",
                "-h",
                "-b",
                "-f",
                "-l",
                WIDTH,
                "-t",
                window,
                "-P",
                "-F",
                "#{pane_id}",
                "tmux-agent-plane sidebar",
            ])?;
            tmux::run(&["set-option", "-p", "-t", pane.trim(), MARK, "1"]).map(drop)
        }
    }
}

pub fn run() -> Result<()> {
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal);
    ratatui::restore();
    result
}

struct Sidebar {
    agents: Vec<Agent>,
    list: ListState,
    error: Option<String>,
}

impl Sidebar {
    fn refresh(&mut self) {
        let keep = self
            .list
            .selected()
            .and_then(|i| self.agents.get(i))
            .map(|a| a.pane.clone());
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
        let selected = keep
            .and_then(|pane| self.agents.iter().position(|a| a.pane == pane))
            .or(if self.agents.is_empty() {
                None
            } else {
                Some(0)
            });
        self.list.select(selected);
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

fn event_loop(terminal: &mut DefaultTerminal) -> Result<()> {
    let mut sidebar = Sidebar {
        agents: Vec::new(),
        list: ListState::default(),
        error: None,
    };
    sidebar.refresh();
    let mut refreshed = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &mut sidebar))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('j') | KeyCode::Down => sidebar.step(1),
                KeyCode::Char('k') | KeyCode::Up => sidebar.step(-1),
                KeyCode::Enter => {
                    if let Some(a) = sidebar.list.selected().and_then(|i| sidebar.agents.get(i)) {
                        tmux::jump(&a.session, &a.window, &a.pane)?;
                    }
                }
                _ => {}
            }
        }
        if refreshed.elapsed() >= REFRESH {
            sidebar.refresh();
            refreshed = Instant::now();
        }
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
    let dim = Style::default().fg(colour(State::Idle));

    let items: Vec<ListItem> = sidebar
        .agents
        .iter()
        .map(|a| {
            let state = Style::default().fg(colour(a.state));
            let status = format!("{} {}", a.state.as_str(), age(a.state_since));
            let name_width = inner.saturating_sub(status.len() + 3);
            let name = truncate(&a.name, name_width);
            let pad = inner.saturating_sub(2 + name.chars().count() + status.len());
            let mut name_style = Style::default();
            if a.visible {
                name_style = name_style.add_modifier(Modifier::BOLD);
            }
            let second = match (&a.state, &a.detail) {
                (State::Blocked, Some(detail)) => {
                    Span::styled(truncate(&format!("  {detail}"), inner), state)
                }
                _ => Span::styled(truncate(&format!("  {}", a.title), inner), dim),
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(format!("{} ", icon(a.state)), state),
                    Span::styled(name, name_style),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(status, state),
                ]),
                Line::from(second),
                Line::from(Span::styled(
                    truncate(&format!("  {}", a.location), inner),
                    dim,
                )),
            ])
        })
        .collect();

    let count = |s: State| sidebar.agents.iter().filter(|a| a.state == s).count();
    let title = match &sidebar.error {
        Some(_) => " daemon not running ".to_string(),
        None => format!(
            " agents ◆{} ✓{} ●{} ○{} ",
            count(State::Blocked),
            count(State::Done),
            count(State::Working),
            count(State::Idle)
        ),
    };
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, list_area, &mut sidebar.list);
    f.render_widget(
        Paragraph::new(Span::styled(" ⏎ jump · j/k · q close", dim)),
        help_area,
    );
}
