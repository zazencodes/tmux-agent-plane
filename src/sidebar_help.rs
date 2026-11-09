//! One shortcut list, paged on short terminals. Every closing key is consumed.
use crate::ui::{SUBTEXT, TEXT};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// Keep labels as individual whitespace-separated keys, so coverage can check them.
const SHORTCUTS: &[(&str, &str)] = &[
    ("Alt-↑/↓", "Prev/next agent (global)"),
    ("S-←", "Go into the sidebar (global)"),
    ("S-→", "Open selected, leave sidebar"),
    ("j k", "Move selection and preview it"),
    ("g G", "gg first agent, G last agent"),
    ("↑ ↓", "Move selection or in filter"),
    ("^j ^k", "Move selection or in filter"),
    ("enter", "Open, even from filter / save"),
    ("enter", "Daemon down: restart it"),
    ("/", "Search rows; enter keeps it"),
    ("n N", "Next/prev search match"),
    ("f", "Filter rows; / freezes it"),
    ("d", "Cycle state filter"),
    ("esc", "Clear / cancel / close"),
    ("^c", "In lists, same as esc"),
    ("r", "Rename selected agent"),
    ("x", "Confirm kill of agent"),
    ("t", "Show or hide background tasks"),
    ("x", "Task row: confirm its stop"),
    ("R", "Restart selected task"),
    ("b", "Restore agents after restart"),
    ("c", "New agent in this project"),
    ("o", "Configure"),
    ("u", "Quota left; u or esc back"),
    ("r", "Quota view: read again"),
    ("enter", "Quota: allow Keychain read"),
    ("tab", "Picker: type to the agent"),
    ("?", "Keyboard shortcuts"),
    ("q", "Close sidebars or picker"),
    ("ctrl-w", "Delete word in an edit"),
    ("ctrl-u", "Clear text in an edit"),
    ("bksp", "Delete character in an edit"),
    ("space", "Rename: insert hyphen"),
    ("enter y", "Kill or stop: confirm it"),
    ("1-7", "Configure: choose AI command"),
    ("d", "Configure: turn AI names off"),
];

#[derive(Default)]
pub struct Help {
    pub open: bool,
    start: usize,
    next: usize,
}

impl Help {
    pub fn key(&mut self, key: KeyEvent, can_open: bool) -> bool {
        if self.open {
            self.open = false;
            return true;
        }
        if can_open && key.code == KeyCode::Char('?') {
            self.open = true;
            self.start = self.next;
            return true;
        }
        false
    }

    fn page(&mut self, height: u16) -> std::ops::Range<usize> {
        // On tiny terminals retain at least one shortcut, even when no header fits.
        let chrome = if height >= 4 {
            3
        } else {
            u16::from(height >= 2)
        };
        let count = usize::from(height.saturating_sub(chrome).max(1));
        let end = (self.start + count).min(SHORTCUTS.len());
        self.next = if end == SHORTCUTS.len() { 0 } else { end };
        self.start..end
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let height = area.height;
        let page = self.page(height);
        let [title, body, footer] = Layout::vertical([
            Constraint::Length(u16::from(height >= 4)),
            Constraint::Min(0),
            Constraint::Length(if height >= 4 {
                2
            } else {
                u16::from(height >= 2)
            }),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(format!(
                "Keyboard shortcuts {}–{}/{}",
                page.start + 1,
                page.end,
                SHORTCUTS.len()
            )),
            title,
        );
        let lines: Vec<_> = SHORTCUTS[page]
            .iter()
            .map(|(keys, action)| {
                let pad = 9usize.saturating_sub(keys.chars().count());
                Line::styled(format!("{keys}{}{action}", " ".repeat(pad)), TEXT)
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), body);
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled(
                    if footer.height == 1 {
                        "Any key closes; reopen ? for next"
                    } else {
                        "Any key closes (consumed)"
                    },
                    SUBTEXT,
                ),
                Line::styled(
                    if self.next == 0 {
                        "? reopens this help"
                    } else {
                        "Reopen ? for next page"
                    },
                    SUBTEXT,
                ),
            ]),
            footer,
        );
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn labels() -> Vec<&'static str> {
        SHORTCUTS
            .iter()
            .flat_map(|(keys, _)| keys.split_whitespace())
            .collect()
    }
    #[test]
    fn every_key_handled_by_sidebar_key_has_a_help_entry() {
        // Read the method itself: adding a command without a help entry fails this test.
        let source = include_str!("sidebar.rs")
            .split("    pub(crate) fn key(&mut self, key: KeyEvent) -> bool {")
            .nth(1)
            .unwrap()
            .split("\n    }\n")
            .next()
            .unwrap();
        let labels = SHORTCUTS
            .iter()
            .flat_map(|(keys, _)| keys.split_whitespace())
            .collect::<Vec<_>>();
        for token in source.split("KeyCode::").skip(1) {
            let label = if let Some(value) = token.strip_prefix("Char('") {
                value.chars().next().unwrap().to_string()
            } else {
                let name: String = token
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                match name.as_str() {
                    "Up" => "↑".into(),
                    "Down" => "↓".into(),
                    other => other.to_lowercase(),
                }
            };
            assert!(
                labels.contains(&label.as_str()),
                "missing sidebar help for {label}"
            );
        }
        for label in ["?", "c", "o", "x", "enter", "ctrl-w", "ctrl-u"] {
            assert!(labels.contains(&label));
        }
    }

    #[test]
    fn every_shortcut_is_reachable_at_short_heights_and_each_close_is_consumed() {
        for height in [1, 2, 3, 4, 8, 12, 28] {
            let mut help = Help::default();
            let mut visited = Vec::new();
            for _ in 0..SHORTCUTS.len() {
                assert!(help.key(KeyEvent::from(KeyCode::Char('?')), true));
                visited.extend(help.page(height));
                assert!(help.key(KeyEvent::from(KeyCode::Char('x')), true));
                assert!(!help.open);
                if help.next == 0 {
                    break;
                }
            }
            assert_eq!(visited, (0..SHORTCUTS.len()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn shortcut_lines_fit_38_columns() {
        for (keys, action) in SHORTCUTS {
            assert!(9 + action.chars().count() <= 38, "{keys}: {action}");
            assert!(keys.chars().count() < 9);
        }
    }
}
