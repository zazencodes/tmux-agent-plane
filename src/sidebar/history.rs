//! The same recent-conversation browser in the sidebar and picker.
use super::{Origin, Sidebar, cancels};
use crate::history::{self, Entry};
use crate::index::Agent;
use crate::ui::{OVERLAY, SUBTEXT, TEXT, age, hints, project_rgb, truncate};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

#[derive(Default)]
pub(crate) struct History {
    pub(crate) open: bool,
    entries: Vec<Entry>,
    list: ListState,
    pub(crate) chosen: Option<u64>,
    message: Option<String>,
    project: Option<String>,
    query: String,
    editing: Option<String>,
    removing: Option<u64>,
}

impl History {
    pub(crate) fn run(&mut self) -> Option<Agent> {
        let id = self.chosen.take()?;
        match history::revive(id) {
            Ok(agent) => {
                self.open = false;
                Some(agent)
            }
            Err(error) => {
                self.message = Some(format!("{error:#}"));
                None
            }
        }
    }

    fn visible(&self) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                history::matches(e, self.project.as_deref(), &self.query).then_some(i)
            })
            .collect()
    }

    fn reset_selection(&mut self) {
        self.list = ListState::default().with_selected(self.visible().first().map(|_| 0));
    }

    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect, agents: &[Agent]) {
        let [title, content, footer] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);
        let wide = content.width >= 80;
        let [body, detail] = if wide {
            Layout::horizontal([Constraint::Percentage(55), Constraint::Fill(1)])
                .spacing(2)
                .areas(content)
        } else {
            Layout::vertical([Constraint::Min(0), Constraint::Length(3)]).areas(content)
        };
        let width = body.width as usize;
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    format!(
                        " Recently closed{}",
                        self.project
                            .as_ref()
                            .map(|p| format!(" · {p}"))
                            .unwrap_or_default()
                    ),
                    Style::default().fg(TEXT).bold(),
                ),
                Line::styled(
                    if self.editing.is_some() || !self.query.is_empty() {
                        format!(" / {}", self.query)
                    } else {
                        String::new()
                    },
                    SUBTEXT,
                ),
            ]),
            title,
        );
        let visible = self.visible();
        let items: Vec<ListItem> = visible
            .iter()
            .map(|&i| {
                let entry = &self.entries[i];
                let agent = &entry.agent;
                let [r, g, b] = project_rgb(agents, &agent.workspace, &agent.session);
                let color = ratatui::style::Color::Rgb(r, g, b);
                let when = format!(" {:>4}  ", age(entry.closed_at.unwrap()));
                let title = if agent.title.trim().is_empty() {
                    &agent.name
                } else {
                    &agent.title
                };
                let available = width.saturating_sub(when.len() + 3);
                ListItem::new(Line::from(vec![
                    Span::styled(" ▏", color),
                    Span::styled(when, SUBTEXT),
                    Span::styled(truncate(title, available), color),
                    Span::styled(format!("  · {}", agent.session), color),
                ]))
            })
            .collect();
        if items.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(
                    if self.entries.is_empty() {
                        " No recently closed sessions here"
                    } else {
                        " No matching conversations"
                    },
                    SUBTEXT,
                )),
                body,
            );
        } else {
            f.render_stateful_widget(
                List::new(items)
                    .highlight_style(Style::default().bg(OVERLAY))
                    .highlight_symbol("▸"),
                body,
                &mut self.list,
            );
        }
        let selected = self
            .list
            .selected()
            .and_then(|i| visible.get(i).map(|&j| &self.entries[j]));
        let message = if self.chosen.is_some() {
            "Reviving conversation…".into()
        } else {
            self.message
                .clone()
                .or_else(|| selected.and_then(history::reason))
                .unwrap_or_else(|| selected.map(|e| e.agent.cwd.clone()).unwrap_or_default())
        };
        let mut lines = Vec::new();
        if let Some(entry) = selected {
            let a = &entry.agent;
            if wide {
                lines.push(Line::styled(
                    a.title.clone(),
                    Style::default().fg(TEXT).bold(),
                ));
                lines.push(Line::default());
            }
            let metadata = format!("{} · {} · {}", a.name, a.session, a.kind.as_str());
            lines.push(Line::styled(truncate(&metadata, detail.width.into()), TEXT));
        }
        lines.push(Line::from(message));
        f.render_widget(
            Paragraph::new(lines)
                .style(SUBTEXT)
                .wrap(ratatui::widgets::Wrap { trim: true }),
            detail,
        );
        let actions = if self.removing.is_some() {
            Line::styled(" Remove from history? y/n", TEXT)
        } else if self.editing.is_some() {
            hints(&[("⏎", "apply"), ("esc", "cancel")])
        } else {
            hints(&[
                ("/", "find"),
                (
                    "p",
                    if self.project.is_some() {
                        "show all"
                    } else {
                        "project"
                    },
                ),
                ("x", "remove"),
                ("⏎", "revive"),
            ])
        };
        f.render_widget(Paragraph::new(actions), footer);
    }

    fn key(&mut self, key: KeyEvent) {
        if self.chosen.is_some() {
            return;
        }
        if let Some(id) = self.removing.take() {
            if !cancels(key) && key.code == KeyCode::Char('y') {
                match history::forget(id) {
                    Ok(()) => {
                        self.entries.retain(|e| e.id != id);
                        self.reset_selection();
                    }
                    Err(error) => self.message = Some(format!("{error:#}")),
                }
            }
            return;
        }
        if self.editing.is_some() {
            if cancels(key) {
                self.query = self.editing.take().unwrap();
            } else {
                match key.code {
                    KeyCode::Enter => self.editing = None,
                    KeyCode::Backspace => {
                        self.query.pop();
                    }
                    KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                        self.query.push(c)
                    }
                    _ => return,
                }
            }
            self.reset_selection();
            return;
        }
        if cancels(key) {
            if !self.query.is_empty() {
                self.query.clear();
            } else if self.project.is_none() {
                self.open = false;
            } else {
                self.project = None;
            }
            self.reset_selection();
            return;
        }
        self.message = None;
        let visible = self.visible();
        let last = visible.len().saturating_sub(1);
        let row = self.list.selected().unwrap_or(0);
        let selected = visible.get(row).map(|&i| &self.entries[i]);
        match key.code {
            KeyCode::Char('h' | 'q') => self.open = false,
            KeyCode::Char('/') => self.editing = Some(self.query.clone()),
            KeyCode::Char('p') => {
                self.project = if self.project.is_some() {
                    None
                } else {
                    selected.map(|e| e.agent.session.clone())
                };
                self.reset_selection();
            }
            KeyCode::Char('x') => self.removing = selected.map(|e| e.id),
            KeyCode::Char('j') | KeyCode::Down => self.list.select(Some((row + 1).min(last))),
            KeyCode::Char('k') | KeyCode::Up => self.list.select(Some(row.saturating_sub(1))),
            KeyCode::Home => self.list.select(Some(0)),
            KeyCode::End => self.list.select(Some(last)),
            KeyCode::Enter => {
                if let Some(entry) = selected {
                    self.message = history::reason(entry);
                    if self.message.is_none() {
                        self.chosen = Some(entry.id);
                    }
                }
            }
            _ => {}
        }
    }
}

impl Sidebar {
    pub(crate) fn history_key(&mut self, key: KeyEvent, _origin: Origin<'_>) -> bool {
        if self.history.open {
            self.history.key(key);
            return true;
        }
        if key.code != KeyCode::Char('h')
            || key.modifiers.contains(KeyModifiers::CONTROL)
            || self.editing.is_some()
            || self.typing_query()
            || self.killing.is_some()
            || self.cleaning.is_some()
            || self.new_agent.is_some()
            || self.tasks.stopping.is_some()
            || self.auto_name.open
            || self.help.open
            || self.quota.open
            || self.restore.is_open()
            || self.menu.is_open()
            || self.project_rename.is_some()
            || self.project_move.is_some()
        {
            return false;
        }
        match history::recent(self.workspace.as_deref()) {
            Ok(entries) => {
                self.history = History {
                    open: true,
                    entries,
                    ..History::default()
                };
                if !self.history.entries.is_empty() {
                    self.history.list.select(Some(0));
                }
            }
            Err(error) => self.notice = Some(format!("{error:#}")),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn browsing() -> Sidebar {
        let mut sidebar = Sidebar::default();
        sidebar.history.open = true;
        sidebar.history.entries = (1..=3)
            .map(|id| Entry {
                id,
                agent: crate::history::tests::agent(Some(&id.to_string())),
                closed_at: Some(1),
            })
            .collect();
        sidebar.history.list.select(Some(0));
        sidebar
    }

    #[test]
    fn both_views_browse_select_revive_and_consume_cancellation() {
        for origin in [Origin::Sidebar("%9"), Origin::Popup("client")] {
            let mut sidebar = browsing();
            for key in [
                KeyCode::Char('j'),
                KeyCode::Down,
                KeyCode::Down,
                KeyCode::Up,
            ] {
                assert!(sidebar.action_key(KeyEvent::from(key), origin));
            }
            assert_eq!(sidebar.history.list.selected(), Some(1));
            assert!(sidebar.action_key(KeyEvent::from(KeyCode::Enter), origin));
            assert_eq!(sidebar.history.chosen, Some(2));
            let mut sidebar = browsing();
            assert!(sidebar.action_key(
                KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
                origin
            ));
            assert!(!sidebar.history.open);
            assert!(!sidebar.left);
        }
    }

    #[test]
    fn search_and_project_filter_pin_actions_to_the_visible_conversation() {
        let mut sidebar = browsing();
        sidebar.history.entries[1].agent.session = "tools".into();
        sidebar.history.entries[1].agent.title = "Review the parser".into();
        let origin = Origin::Sidebar("%9");
        for c in "/review".chars() {
            assert!(sidebar.action_key(KeyEvent::from(KeyCode::Char(c)), origin));
        }
        sidebar.action_key(KeyEvent::from(KeyCode::Enter), origin);
        sidebar.action_key(KeyEvent::from(KeyCode::Char('p')), origin);
        sidebar.action_key(KeyEvent::from(KeyCode::Esc), origin);
        assert_eq!(sidebar.history.visible(), vec![1]);
        sidebar.action_key(KeyEvent::from(KeyCode::Char('x')), origin);
        assert_eq!(sidebar.history.removing, Some(2));
        sidebar.action_key(KeyEvent::from(KeyCode::Esc), origin);
        assert!(sidebar.history.removing.is_none());
        sidebar.action_key(KeyEvent::from(KeyCode::Enter), origin);
        assert_eq!(sidebar.history.chosen, Some(2));
    }

    #[test]
    fn missing_ids_do_not_launch_and_small_layouts_render() {
        let mut sidebar = browsing();
        sidebar.history.entries[0].agent.session_id = None;
        sidebar.history.key(KeyEvent::from(KeyCode::Enter));
        assert_eq!(sidebar.history.chosen, None);
        assert!(
            sidebar
                .history
                .message
                .as_deref()
                .unwrap()
                .contains("no session id")
        );
        for (width, height) in [(38, 18), (16, 5), (1, 1)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|f| sidebar.history.draw(f, f.area(), &[]))
                .unwrap();
        }
    }
}
