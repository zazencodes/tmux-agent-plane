//! `v`: move the selected agent into another project of its workspace, without restarting it.
//! The footer lists the projects: `j`/`k` choose, `Enter` moves, `Esc` or Ctrl-C cancels. The
//! sidebar and the picker alike.

use super::{Sidebar, cancels};
use crate::index::{Agent, State};
use crate::ui::{SUBTEXT, TEXT, colour, hints, truncate};
use crate::{sessions, tasks};
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::style::Style;
use ratatui::text::Line;

pub(crate) struct ProjectMove {
    /// The agent as it was when `v` was pressed: the move refuses if it has changed since.
    agent: Agent,
    /// The workspace's other projects, by tmux session name.
    projects: Vec<String>,
    selected: usize,
    /// Why the last move failed, until the next key.
    error: Option<String>,
}

impl Sidebar {
    /// While a project is chosen, it takes every key.
    pub(crate) fn move_key(&mut self, key: KeyEvent) -> bool {
        if let Some(picker) = &mut self.project_move {
            picker.error = None;
            let count = picker.projects.len();
            match key.code {
                _ if cancels(key) => self.project_move = None,
                KeyCode::Char('j') | KeyCode::Down => {
                    picker.selected = (picker.selected + 1) % count
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    picker.selected = (picker.selected + count - 1) % count
                }
                KeyCode::Enter => {
                    let project = &picker.projects[picker.selected];
                    match sessions::move_agent(&picker.agent, project) {
                        Ok(_) => {
                            self.notice = Some(format!("moved {} to {project}", picker.agent.name));
                            self.project_move = None;
                        }
                        Err(error) => picker.error = Some(format!("{error:#}")),
                    }
                }
                _ => {}
            }
            return true;
        }
        if key.code != KeyCode::Char('v')
            || !key.modifiers.is_empty()
            || self.editing.is_some()
            || self.typing_query()
            || self.auto_name.open
            || self.help.open
            || self.killing.is_some()
            || self.new_agent.is_some()
            || self.project_rename.is_some()
            || self.tasks.stopping.is_some()
            || self.restore.is_open()
            || self.menu.is_open()
        {
            return false;
        }
        self.notice = None;
        let Some(agent) = self.selected() else {
            self.notice = Some("select an agent to move it".into());
            return true;
        };
        let projects: Vec<String> = self
            .workspaces
            .iter()
            .filter(|w| w.name == agent.workspace)
            .flat_map(|w| &w.sessions)
            .map(|s| s.name.clone())
            .filter(|name| *name != agent.session && name != tasks::SESSION)
            .collect();
        if projects.is_empty() {
            self.notice = Some(format!("no other project in {}", agent.workspace));
            return true;
        }
        self.project_move = Some(ProjectMove {
            agent: agent.clone(),
            projects,
            selected: 0,
            error: None,
        });
        true
    }
}

pub(crate) fn footer(picker: &ProjectMove, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(
        truncate(&format!(" move {} to", picker.agent.name), width),
        Style::default().fg(TEXT).bold(),
    )];
    lines.extend(picker.projects.iter().enumerate().map(|(row, project)| {
        if row == picker.selected {
            Line::styled(
                truncate(&format!(" ▌ {project}"), width),
                Style::default().fg(TEXT).bold(),
            )
        } else {
            Line::styled(truncate(&format!("   {project}"), width), SUBTEXT)
        }
    }));
    if let Some(error) = &picker.error {
        lines.push(Line::styled(
            truncate(&format!(" {error}"), width),
            colour(State::Blocked),
        ));
    }
    lines.push(hints(&[("⏎", "move"), ("esc", "cancel")]));
    lines
}

#[cfg(test)]
mod tests {
    use super::super::Origin;
    use super::super::tests::showing;
    use super::*;
    use crate::index::{Kind, Workspace, WorkspaceSession};
    use ratatui::crossterm::event::KeyModifiers;

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
            orchestrator: false,
            process_id: 0,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    fn workspace(name: &str, sessions: &[&str]) -> Workspace {
        Workspace {
            name: name.into(),
            sessions: sessions
                .iter()
                .enumerate()
                .map(|(i, s)| WorkspaceSession {
                    id: format!("${i}"),
                    name: (*s).into(),
                })
                .collect(),
        }
    }

    fn press(sidebar: &mut Sidebar, key: KeyEvent) -> bool {
        sidebar.action_key(key, Origin::Popup("client"))
    }

    fn footer_text(sidebar: &Sidebar) -> Vec<String> {
        footer(sidebar.project_move.as_ref().unwrap(), 38)
            .iter()
            .map(Line::to_string)
            .collect()
    }

    #[test]
    fn v_lists_the_workspaces_other_projects_and_j_k_choose() {
        let mut sidebar = showing(vec![agent()]);
        sidebar.workspaces = vec![
            workspace("main", &["csj", "site", tasks::SESSION, "api"]),
            workspace("other", &["far"]),
        ];
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('v'))));
        assert_eq!(
            footer_text(&sidebar),
            [" move lead to", " ▌ site", "   api", " ⏎ move  esc cancel "]
        );
        // Every key belongs to the picker: x never kills, k wraps to the last project.
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('x'))));
        assert!(sidebar.killing.is_none());
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('k'))));
        assert_eq!(footer_text(&sidebar)[2], " ▌ api");
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(press(&mut sidebar, ctrl_c));
        assert!(sidebar.project_move.is_none());
        assert!(!sidebar.left);
    }

    #[test]
    fn v_needs_an_agent_and_another_project() {
        let mut sidebar = showing(Vec::new());
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('v'))));
        assert_eq!(
            sidebar.notice.as_deref(),
            Some("select an agent to move it")
        );
        let mut sidebar = showing(vec![agent()]);
        sidebar.workspaces = vec![workspace("main", &["csj"])];
        assert!(press(&mut sidebar, KeyEvent::from(KeyCode::Char('v'))));
        assert!(sidebar.project_move.is_none());
        assert_eq!(sidebar.notice.as_deref(), Some("no other project in main"));
    }
}
