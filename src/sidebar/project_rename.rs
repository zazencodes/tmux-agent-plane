//! `e`: rename the selected agent's project, its tmux session, in the footer. While the name is
//! typed every key is text: `Enter` saves, `Esc` or Ctrl-C cancels. The sidebar and the picker
//! alike.

use super::{Sidebar, cancels, delete_word};
use crate::index::State;
use crate::sessions;
use crate::ui::{TEXT, colour, hints, truncate};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::Line;

pub(crate) struct ProjectRename {
    /// The project's name when `e` was pressed: the rename is pinned to it.
    project: String,
    name: String,
    /// Why the last save failed, until the next key.
    error: Option<String>,
}

impl Sidebar {
    /// While a project name is typed, it takes every key.
    pub(crate) fn project_key(&mut self, key: KeyEvent) -> bool {
        if let Some(edit) = &mut self.project_rename {
            edit.error = None;
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                _ if cancels(key) => self.project_rename = None,
                KeyCode::Enter => match sessions::rename_project(&edit.project, &edit.name) {
                    Ok(old) => {
                        self.notice = Some(format!("project {old} is now {}", edit.name));
                        self.project_rename = None;
                    }
                    Err(error) => edit.error = Some(format!("{error:#}")),
                },
                KeyCode::Backspace => {
                    edit.name.pop();
                }
                KeyCode::Char('w') if ctrl => delete_word(&mut edit.name),
                KeyCode::Char('u') if ctrl => edit.name.clear(),
                KeyCode::Char(c) if !ctrl => edit.name.push(c),
                _ => {}
            }
            return true;
        }
        if key.code != KeyCode::Char('e')
            || !key.modifiers.is_empty()
            || self.editing.is_some()
            || self.typing_query()
            || self.auto_name.open
            || self.help.open
            || self.killing.is_some()
            || self.new_agent.is_some()
            || self.tasks.stopping.is_some()
            || self.restore.is_open()
            || self.menu.is_open()
        {
            return false;
        }
        self.notice = None;
        match self.selected() {
            Some(agent) => {
                self.project_rename = Some(ProjectRename {
                    project: agent.session.clone(),
                    name: agent.session.clone(),
                    error: None,
                })
            }
            None => self.notice = Some("select an agent to choose its project".into()),
        }
        true
    }
}

pub(crate) fn footer(edit: &ProjectRename, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::styled(
            truncate(&format!(" rename project {}", edit.project), width),
            Style::default().fg(TEXT).bold(),
        ),
        Line::styled(
            // Keep the end in view, where the cursor is.
            format!(
                " › {}▏",
                edit.name
                    .chars()
                    .rev()
                    .take(width.saturating_sub(5))
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<String>()
            ),
            Style::default().fg(colour(State::Done)),
        ),
    ];
    if let Some(error) = &edit.error {
        lines.push(Line::styled(
            truncate(&format!(" {error}"), width),
            colour(State::Blocked),
        ));
    }
    lines.push(hints(&[("⏎", "save"), ("esc", "cancel")]));
    lines
}

#[cfg(test)]
mod tests {
    use super::super::Origin;
    use super::super::tests::showing;
    use super::*;
    use crate::index::{Agent, Kind};

    fn agent() -> Agent {
        Agent {
            name: "lead".into(),
            pane: "%1".into(),
            kind: Kind::Claude,
            state: State::Idle,
            detail: None,
            title: String::new(),
            location: "csj:1.0".into(),
            session: "csj".into(),
            workspace: "main".into(),
            window: "@1".into(),
            cwd: "/tmp".into(),
            parent: None,
            parent_ended: false,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    fn press(sidebar: &mut Sidebar, key: KeyEvent) -> bool {
        sidebar.action_key(key, Origin::Popup("client"))
    }

    fn typed(sidebar: &Sidebar) -> Option<&str> {
        sidebar.project_rename.as_ref().map(|e| e.name.as_str())
    }

    #[test]
    fn e_starts_from_the_projects_name_and_every_key_is_text_until_esc() {
        let mut sidebar = showing(vec![agent()]);
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('e'))));
        assert_eq!(typed(&sidebar), Some("csj"));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        press(&mut sidebar, ctrl('u'));
        // Shortcut letters are text: c, q, w, x and ? never act on the list.
        for c in "cqwx?".chars() {
            assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char(c))));
        }
        assert_eq!(typed(&sidebar), Some("cqwx?"));
        assert!(sidebar.new_agent.is_none() && !sidebar.menu.is_open() && !sidebar.help.open);
        press(&mut sidebar, KeyEvent::from(KeyCode::Backspace));
        assert_eq!(typed(&sidebar), Some("cqwx"));
        let text: Vec<String> = footer(sidebar.project_rename.as_ref().unwrap(), 38)
            .iter()
            .map(Line::to_string)
            .collect();
        assert_eq!(text[0], " rename project csj");
        assert_eq!(text[1], " › cqwx▏");
        // Ctrl-C cancels like Esc, and nothing else closes.
        assert!(press(&mut sidebar, ctrl('c')));
        assert!(sidebar.project_rename.is_none());
        assert!(!sidebar.left);
    }

    #[test]
    fn e_needs_a_selected_agent_and_waits_for_other_edits() {
        let mut sidebar = showing(Vec::new());
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('e'))));
        assert!(sidebar.project_rename.is_none());
        assert_eq!(
            sidebar.notice.as_deref(),
            Some("select an agent to choose its project")
        );
        let mut sidebar = showing(vec![agent()]);
        sidebar.start_search();
        assert!(!press(&mut sidebar, KeyEvent::from(KeyCode::Char('e'))));
        assert!(sidebar.project_rename.is_none());
    }
}
