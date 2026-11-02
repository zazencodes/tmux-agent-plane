//! A narrow, always-on agent list in a pane down the side of a window.

use crate::commands;
use crate::index::{self, Agent, State};
use crate::tmux;
use crate::ui::{
    LAVENDER, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, age, colour, counts, hints, icon,
    message, name_style, panel, state_style,
};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
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
    /// The name being typed for the selected agent, while renaming.
    editing: Option<String>,
    /// Why the last rename failed.
    notice: Option<String>,
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

    fn selected(&self) -> Option<&Agent> {
        self.list.selected().and_then(|i| self.agents.get(i))
    }

    /// A key while renaming: edit the name, save it, or give up.
    fn edit_key(&mut self, key: KeyEvent) {
        let Some(name) = self.editing.as_mut() else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.editing = None,
            KeyCode::Enter => {
                let name = name.clone();
                let Some(pane) = self.selected().map(|a| a.pane.clone()) else {
                    return;
                };
                match commands::set_name(&pane, &name) {
                    Ok(_) => {
                        // Show it now; the daemon picks it up on its next tick.
                        if let Some(agent) = self.agents.iter_mut().find(|a| a.pane == pane) {
                            agent.name = name;
                        }
                        self.editing = None;
                        self.notice = None;
                    }
                    Err(e) => self.notice = Some(e.to_string()),
                }
            }
            KeyCode::Char('w') if ctrl => delete_word(name),
            KeyCode::Char('u') if ctrl => name.clear(),
            KeyCode::Backspace => {
                name.pop();
            }
            KeyCode::Char(' ') => name.push('-'),
            KeyCode::Char(c) if !ctrl => name.push(c),
            _ => {}
        }
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
        editing: None,
        notice: None,
    };
    sidebar.refresh();
    let mut refreshed = Instant::now();
    loop {
        terminal.draw(|f| draw(f, &mut sidebar))?;
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            if sidebar.editing.is_some() {
                sidebar.edit_key(key);
                continue;
            }
            sidebar.notice = None;
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('r') => sidebar.editing = sidebar.selected().map(|a| a.name.clone()),
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
    let block = panel(Line::from(" agents ")).title_top(counts(&sidebar.agents));
    if let Some(e) = &sidebar.error {
        message(
            f,
            list_area,
            block,
            colour(State::Blocked),
            &["daemon not running", e],
        );
    } else if sidebar.agents.is_empty() {
        message(
            f,
            list_area,
            block,
            TEXT,
            &[
                "no agents running",
                "start claude or codex in any tmux pane",
            ],
        );
    } else {
        let selected = sidebar.list.selected();
        let items = items(&sidebar.agents, selected, sidebar.editing.as_ref(), inner);
        f.render_stateful_widget(List::new(items).block(block), list_area, &mut sidebar.list);
    }

    let help = match (&sidebar.notice, &sidebar.editing) {
        (Some(notice), _) => Line::styled(
            truncate(&format!(" {notice}"), inner + 2),
            colour(State::Blocked),
        ),
        (None, Some(_)) => hints(&[("⏎", "save"), ("esc", "cancel"), ("^w ^u", "clear")]),
        (None, None) => hints(&[("⏎", "jump"), ("r", "rename"), ("q", "close")]),
    };
    f.render_widget(Paragraph::new(help), help_area);
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
    use super::delete_word;

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
