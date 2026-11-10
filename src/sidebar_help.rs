//! The shortcut list: a few short sections, one line per key, paged on short terminals. Keys
//! that only matter mid-action (confirming, editing text) show in the footer at that moment.
//! Every closing key is consumed.
use crate::quota::render::YELLOW;
use crate::ui::{LAVENDER, OVERLAY, SUBTEXT, TEXT};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Key column width; actions take the rest of a 38-column sidebar.
const KEYS: usize = 12;

/// Keep labels as individual whitespace-separated keys, so coverage can check them.
const SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Move",
        &[
            ("j k ↑ ↓", "Select and preview"),
            ("gg G", "First / last row"),
            ("enter l →", "Go to the selected row"),
        ],
    ),
    (
        "Find",
        &[
            ("/ n N", "Search; next / previous"),
            ("f", "Filter by text"),
            ("d", "Filter by state"),
            ("esc", "Clear or cancel"),
        ],
    ),
    (
        "Act",
        &[
            ("c", "New agent in this project"),
            ("r", "Rename agent"),
            ("x", "Kill agent or stop task"),
            ("R", "Restart task"),
            ("tab", "Picker: type to agent"),
        ],
    ),
    (
        "Show",
        &[
            ("p", "Graph: who spawned whom"),
            ("t", "Background tasks"),
            ("u", "Quota left"),
            ("b", "Agents to restore"),
            ("o", "Configure"),
            ("q", "Close"),
        ],
    ),
];

/// The plugin's global bindings, as tmux has them now: (option, what it does).
const GLOBAL: [(&str, &str); 4] = [
    ("previous", "Previous agent"),
    ("next", "Next agent"),
    ("sidebar-in", "Into the sidebar"),
    ("sidebar-out", "Out to the selected agent"),
];

#[derive(Default)]
pub struct Help {
    pub open: bool,
    lines: Vec<Line<'static>>,
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
            self.lines = lines(global_keys());
            self.start = self.next;
            return true;
        }
        false
    }

    fn page(&mut self, height: u16) -> std::ops::Range<usize> {
        let (title, footer) = chrome(height);
        let count = usize::from(height.saturating_sub(title + footer).max(1));
        let start = self.start.min(self.lines.len());
        let end = (start + count).min(self.lines.len());
        self.next = if end == self.lines.len() { 0 } else { end };
        start..end
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let height = area.height;
        let page = self.page(height);
        let paged = page.start > 0 || self.next != 0;
        let (title_height, footer_height) = chrome(height);
        let [title, body, footer] = Layout::vertical([
            Constraint::Length(title_height),
            Constraint::Min(0),
            Constraint::Length(footer_height),
        ])
        .areas(area);
        let mut heading = vec![Span::styled(
            "Keyboard shortcuts",
            Style::default().fg(TEXT).bold(),
        )];
        if paged {
            heading.push(Span::styled(
                format!("  {}–{}/{}", page.start + 1, page.end, self.lines.len()),
                OVERLAY,
            ));
        }
        frame.render_widget(Paragraph::new(Line::from(heading)), title);
        frame.render_widget(Paragraph::new(self.lines[page].to_vec()), body);
        let close = if self.next == 0 {
            "Any key closes"
        } else {
            "Any key closes; ? for more"
        };
        let mut lines = vec![Line::styled(close, SUBTEXT)];
        if footer.height > 1 {
            lines.insert(0, Line::default());
        }
        frame.render_widget(Paragraph::new(lines), footer);
    }
}

/// Rows for the title and the footer, each with a blank line to the list when there is room.
/// On tiny terminals at least one line of the list stays, even when no title fits.
fn chrome(height: u16) -> (u16, u16) {
    match height {
        6.. => (2, 2),
        4..=5 => (1, 2),
        2..=3 => (0, 1),
        _ => (0, 0),
    }
}

/// The plugin's global keys as tmux has them bound now.
#[derive(Default)]
struct Global {
    /// (keys, what they do), for each action with keys bound.
    keys: Vec<(String, &'static str)>,
    off: bool,
    toggle: Option<String>,
}

/// `@agent-plane-bound-<name>-key` for each global action, `@agent-plane-keys` and the toggle,
/// or why tmux could not say.
#[cfg(not(test))]
fn global_keys() -> Result<Global, String> {
    let options = crate::tmux::run(&["show-options", "-g"]).map_err(|e| format!("{e:#}"))?;
    let value = |name: &str| {
        let prefix = format!("{name} ");
        options.lines().find_map(|line| {
            let value = line.strip_prefix(&prefix)?.trim_matches('"');
            Some(value.to_string())
        })
    };
    Ok(Global {
        keys: GLOBAL
            .iter()
            .filter_map(|(name, action)| {
                Some((value(&format!("@agent-plane-bound-{name}-key"))?, *action))
            })
            .filter(|(keys, _)| !keys.is_empty())
            .collect(),
        off: value("@agent-plane-keys").as_deref() == Some("off"),
        toggle: value("@agent-plane-bound-keys-toggle-key"),
    })
}

/// Tests never ask a tmux server.
#[cfg(test)]
fn global_keys() -> Result<Global, String> {
    Ok(Global::default())
}

/// The plugin's tmux keys first, then the sections, a blank line apart.
fn lines(global: Result<Global, String>) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    match global {
        Ok(global) if global.keys.is_empty() => {}
        Ok(global) => {
            let mut title = vec![Span::styled(
                "Anywhere in tmux",
                Style::default().fg(LAVENDER).bold(),
            )];
            if global.off {
                title.push(Span::styled("  off", Style::default().fg(YELLOW).bold()));
            }
            lines.push(Line::from(title));
            lines.extend(global.keys.iter().map(|(key, action)| entry(key, action)));
            if let Some(toggle) = &global.toggle {
                let action = if global.off {
                    "Turn these on"
                } else {
                    "Turn these off"
                };
                lines.push(entry(&format!("prefix {toggle}"), action));
            }
            lines.push(Line::default());
        }
        Err(error) => {
            lines.push(heading("Anywhere in tmux"));
            lines.push(Line::styled(error, OVERLAY));
            lines.push(Line::default());
        }
    }
    for (i, (title, keys)) in SECTIONS.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(heading(title));
        lines.extend(keys.iter().map(|(key, action)| entry(key, action)));
    }
    lines
}

fn heading(title: &str) -> Line<'static> {
    Line::styled(title.to_string(), Style::default().fg(LAVENDER).bold())
}

fn entry(key: &str, action: &str) -> Line<'static> {
    let pad = KEYS.saturating_sub(key.chars().count()).max(1);
    Line::from(vec![
        Span::styled(format!("{key}{}", " ".repeat(pad)), SUBTEXT),
        Span::styled(action.to_string(), TEXT),
    ])
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn labels() -> Vec<&'static str> {
        SECTIONS
            .iter()
            .flat_map(|(_, keys)| keys.iter())
            .flat_map(|(keys, _)| keys.split_whitespace())
            .flat_map(|label| {
                if label == "gg" {
                    vec!["g", label]
                } else {
                    vec![label]
                }
            })
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
        let labels = labels();
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
        for label in ["c", "o", "x", "enter", "t", "u", "b", "R"] {
            assert!(labels.contains(&label), "{label}");
        }
    }

    #[test]
    fn global_keys_lead_with_their_toggle_and_a_tmux_failure_shows_in_their_place() {
        let mut global = Global {
            keys: vec![("C-M-k S-Up".into(), "Previous agent")],
            off: false,
            toggle: Some("g".into()),
        };
        let text = lines_text(Ok(Global {
            keys: global.keys.clone(),
            toggle: global.toggle.clone(),
            off: false,
        }));
        assert_eq!(text[0], "Anywhere in tmux");
        assert_eq!(
            text[1],
            format!("C-M-k S-Up{}Previous agent", " ".repeat(2))
        );
        assert_eq!(text[2], format!("prefix g{}Turn these off", " ".repeat(4)));
        global.off = true;
        let text = lines_text(Ok(global));
        assert_eq!(text[0], "Anywhere in tmux  off");
        assert!(text[2].ends_with("Turn these on"));
        let failed = lines_text(Err("no server running".into()));
        assert_eq!(failed[1], "no server running");
        assert_eq!(lines_text(Ok(Global::default()))[0], "Move");
    }

    fn lines_text(global: Result<Global, String>) -> Vec<String> {
        lines(global).iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn every_line_is_reachable_at_short_heights_and_each_close_is_consumed() {
        for height in [1, 2, 3, 4, 8, 12, 28, 60] {
            let mut help = Help::default();
            let mut visited = Vec::new();
            loop {
                assert!(help.key(KeyEvent::from(KeyCode::Char('?')), true));
                visited.extend(help.page(height));
                assert!(help.key(KeyEvent::from(KeyCode::Char('x')), true));
                assert!(!help.open);
                if help.next == 0 {
                    break;
                }
            }
            assert_eq!(visited, (0..help.lines.len()).collect::<Vec<_>>());
        }
    }

    #[test]
    fn every_line_fits_38_columns() {
        for (_, keys) in SECTIONS {
            for (key, action) in *keys {
                assert!(KEYS + action.chars().count() <= 38, "{key}: {action}");
                assert!(key.chars().count() < KEYS);
            }
        }
        for (_, action) in GLOBAL {
            assert!(KEYS + action.chars().count() <= 38, "{action}");
        }
    }
}
