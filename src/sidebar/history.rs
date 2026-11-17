//! The same recent-conversation browser in the sidebar and picker.
use super::{Origin, Sidebar, cancels};
use crate::history::{self, Entry};
use crate::index::Agent;
use crate::ui::{OVERLAY, SUBTEXT, TEXT, age, hints, truncate};
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

    pub(crate) fn draw(&mut self, f: &mut Frame, area: Rect) {
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
            Paragraph::new(Line::styled(
                " Recently closed",
                Style::default().fg(TEXT).bold(),
            )),
            title,
        );
        let items: Vec<ListItem> = self
            .entries
            .iter()
            .map(|entry| {
                let agent = &entry.agent;
                let when = format!(" {:>4}  ", age(entry.closed_at.unwrap()));
                let title = if agent.title.trim().is_empty() {
                    &agent.name
                } else {
                    &agent.title
                };
                let available = width.saturating_sub(when.len() + 1);
                ListItem::new(Line::from(vec![
                    Span::styled(when, SUBTEXT),
                    Span::styled(truncate(title, available), TEXT),
                    Span::styled(format!("  · {}", agent.session), SUBTEXT),
                ]))
            })
            .collect();
        if items.is_empty() {
            f.render_widget(
                Paragraph::new(Line::styled(" No recently closed sessions here", SUBTEXT)),
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
        let selected = self.list.selected().and_then(|i| self.entries.get(i));
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
        f.render_widget(
            Paragraph::new(hints(&[
                ("j/k", "browse"),
                ("⏎", "revive"),
                ("esc", "back"),
            ])),
            footer,
        );
    }

    fn key(&mut self, key: KeyEvent) {
        if self.chosen.is_some() {
            return;
        }
        if cancels(key) || matches!(key.code, KeyCode::Char('h' | 'q')) {
            self.open = false;
            return;
        }
        self.message = None;
        let last = self.entries.len().saturating_sub(1);
        let row = self.list.selected().unwrap_or(0);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.list.select(Some((row + 1).min(last))),
            KeyCode::Char('k') | KeyCode::Up => self.list.select(Some(row.saturating_sub(1))),
            KeyCode::Home => self.list.select(Some(0)),
            KeyCode::End => self.list.select(Some(last)),
            KeyCode::Enter => {
                if let Some(entry) = self.entries.get(row) {
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
                .draw(|f| sidebar.history.draw(f, f.area()))
                .unwrap();
        }
    }
}
