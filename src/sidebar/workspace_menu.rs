//! `w`: the workspaces. `Enter` shows one, `c` starts a new one, `r` renames one and `m` moves the
//! selected agent's project into one. The sidebar and the picker alike; it takes the whole list
//! while open and consumes every key.

use super::{Origin, Sidebar, cancels, clients, delete_word, input_client};
use crate::index::State;
use crate::ui::{OVERLAY, SUBTEXT, SURFACE0, TEXT, WORKSPACE, colour, hints, icon, truncate};
use crate::{sessions, tmux, workspaces};
use anyhow::{Context, Result, bail};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

#[derive(Default)]
pub(crate) struct Menu {
    view: Option<View>,
}

struct View {
    /// The highlighted workspace, by name, so a refresh never moves it.
    selected: String,
    /// A name being typed.
    naming: Option<Naming>,
    /// Why the last action failed, until the next key.
    error: Option<String>,
}

struct Naming {
    /// A new name for the highlighted workspace, or the name of a new one.
    renaming: bool,
    name: String,
}

impl Menu {
    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }
}

impl Sidebar {
    pub(crate) fn workspace_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        if self.menu.view.is_none() {
            let opens = key.code == KeyCode::Char('w')
                && key.modifiers.is_empty()
                && self.editing.is_none()
                && !self.typing_query()
                && !self.auto_name.open
                && !self.help.open
                && self.killing.is_none()
                && self.new_agent.is_none()
                && self.tasks.stopping.is_none()
                && !self.restore.is_open();
            if opens {
                self.notice = None;
                self.menu.view = Some(View {
                    selected: self.workspace.clone().unwrap_or_default(),
                    naming: None,
                    error: None,
                });
            }
            return opens;
        }
        let view = self.menu.view.as_mut().expect("menu is open");
        view.error = None;
        if let Some(naming) = &mut view.naming {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Enter => {
                    let naming = view.naming.take().expect("naming");
                    let selected = view.selected.clone();
                    let result = if naming.renaming {
                        self.rename_workspace(&selected, &naming.name)
                    } else {
                        self.new_workspace(&naming.name, origin)
                    };
                    if let Err(error) = result {
                        let view = self.menu.view.as_mut().expect("menu is open");
                        view.error = Some(format!("{error:#}"));
                        view.naming = Some(naming);
                    }
                }
                _ if cancels(key) => view.naming = None,
                KeyCode::Backspace => {
                    naming.name.pop();
                }
                KeyCode::Char('w') if ctrl => delete_word(&mut naming.name),
                KeyCode::Char('u') if ctrl => naming.name.clear(),
                KeyCode::Char(c) if !ctrl => naming.name.push(c),
                _ => {}
            }
            return true;
        }
        let names: Vec<String> = self.workspaces.iter().map(|w| w.name.clone()).collect();
        let position = names.iter().position(|name| *name == view.selected);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(next) =
                    position.map_or(Some(0), |p| (p + 1 < names.len()).then_some(p + 1))
                {
                    view.selected = names.get(next).cloned().unwrap_or_default();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(p) = position.and_then(|p| p.checked_sub(1)) {
                    view.selected = names[p].clone();
                }
            }
            KeyCode::Char('c') => {
                view.naming = Some(Naming {
                    renaming: false,
                    name: String::new(),
                })
            }
            KeyCode::Char('r') if position.is_some() => {
                view.naming = Some(Naming {
                    renaming: true,
                    name: view.selected.clone(),
                })
            }
            KeyCode::Enter if position.is_some() => {
                let selected = view.selected.clone();
                if let Err(error) = self.show_workspace(&selected, origin) {
                    self.menu_error(error);
                }
            }
            KeyCode::Char('m') if position.is_some() => {
                let selected = view.selected.clone();
                if let Err(error) = self.move_project(&selected) {
                    self.menu_error(error);
                }
            }
            KeyCode::Char('w' | 'q') => self.menu.view = None,
            _ if cancels(key) => self.menu.view = None,
            _ => {}
        }
        true
    }

    fn menu_error(&mut self, error: anyhow::Error) {
        if let Some(view) = &mut self.menu.view {
            view.error = Some(format!("{error:#}"));
        }
    }

    /// Take the client to the session it used last in the workspace. The picker closes; a
    /// sidebar stays behind in the window the client left.
    fn show_workspace(&mut self, workspace: &str, origin: Origin<'_>) -> Result<()> {
        if self.workspace.as_deref() == Some(workspace) {
            self.menu.view = None;
            return Ok(());
        }
        let clients = clients()?;
        let client = input_client(&clients, origin)?;
        workspaces::switch(workspace, Some(&client.name))?;
        self.menu.view = None;
        self.left = matches!(origin, Origin::Popup(_));
        Ok(())
    }

    /// A new workspace starts with a session of its own name, in the folder this list runs in,
    /// and the client goes there.
    fn new_workspace(&mut self, name: &str, origin: Origin<'_>) -> Result<()> {
        workspaces::validate(name)?;
        if self.workspaces.iter().any(|w| w.name == name) {
            bail!("workspace {name} exists");
        }
        let clients = clients()?;
        let client = input_client(&clients, origin)?;
        let session = sessions::create_project(name, None, name)?;
        tmux::run(&["switch-client", "-c", &client.name, "-t", &session])?;
        self.menu.view = None;
        self.left = matches!(origin, Origin::Popup(_));
        Ok(())
    }

    fn rename_workspace(&mut self, workspace: &str, name: &str) -> Result<()> {
        workspaces::rename(workspace, name)?;
        if let Some(view) = &mut self.menu.view {
            view.selected = name.to_owned();
        }
        if self.workspace.as_deref() == Some(workspace) {
            self.workspace = Some(name.to_owned());
        }
        Ok(())
    }

    /// The selected agent's whole project joins the highlighted workspace.
    fn move_project(&mut self, workspace: &str) -> Result<()> {
        let agent = self
            .selected()
            .context("select an agent to choose its project")?;
        if agent.workspace == workspace {
            bail!("{} is already in {workspace}", agent.session);
        }
        let project = agent.session.clone();
        workspaces::move_sessions(std::slice::from_ref(&project), workspace)?;
        self.menu.view = None;
        self.notice = Some(format!("moved {project} to {workspace}"));
        Ok(())
    }

    /// Whole area, like the help.
    pub(crate) fn draw_menu(&self, f: &mut Frame, area: Rect) {
        let Some(view) = &self.menu.view else {
            return;
        };
        let width = area.width as usize;
        let mut body: Vec<Line> = self
            .workspaces
            .iter()
            .map(|workspace| {
                let here = self.workspace.as_ref() == Some(&workspace.name);
                // What wants you there, without counts: those live in the status bar.
                let marks: Vec<Span> = [State::Blocked, State::Done]
                    .into_iter()
                    .filter(|state| {
                        self.everyone
                            .iter()
                            .any(|a| a.workspace == workspace.name && a.state == *state)
                    })
                    .map(|state| Span::styled(format!(" {}", icon(state)), colour(state)))
                    .collect();
                let marks_width: usize = marks.iter().map(Span::width).sum();
                let label = if here { "  here" } else { "" };
                let name = truncate(
                    &workspace.name,
                    width.saturating_sub(4 + label.len() + marks_width),
                );
                let mut line = vec![
                    Span::raw(" "),
                    Span::styled(
                        if here { "• " } else { "  " },
                        Style::default().fg(WORKSPACE),
                    ),
                    Span::styled(name.clone(), Style::default().fg(TEXT).bold()),
                    Span::styled(label, OVERLAY),
                ];
                let used = 3 + name.chars().count() + label.len() + marks_width;
                line.push(Span::raw(" ".repeat(width.saturating_sub(used + 1))));
                line.extend(marks);
                let line = Line::from(line);
                if workspace.name == view.selected {
                    line.style(Style::default().bg(SURFACE0))
                } else {
                    line
                }
            })
            .collect();
        if let Some(naming) = &view.naming {
            body.push(Line::default());
            let prompt = if naming.renaming {
                format!(" rename {}: ", view.selected)
            } else {
                " new workspace: ".to_owned()
            };
            body.push(Line::from(vec![
                Span::styled(prompt, SUBTEXT),
                Span::styled(
                    format!("{}▏", naming.name),
                    Style::default().fg(colour(State::Done)),
                ),
            ]));
        }
        if let Some(error) = &view.error {
            body.push(Line::default());
            body.push(Line::styled(
                truncate(&format!(" {error}"), width),
                colour(State::Blocked),
            ));
        }
        let project = self.selected().map(|a| a.session.clone());
        let footer = if view.naming.is_some() {
            vec![hints(&[("⏎", "save"), ("esc", "cancel")])]
        } else {
            let mut second = Vec::new();
            if let Some(project) = &project {
                second.push(("m", format!("move {} here", truncate(project, 16))));
            }
            second.push(("esc", "close".to_owned()));
            let second: Vec<(&str, &str)> = second.iter().map(|(k, v)| (*k, v.as_str())).collect();
            vec![
                hints(&[("⏎", "switch"), ("c", "new"), ("r", "rename")]),
                hints(&second),
            ]
        };
        let [title_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(footer.len() as u16),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(Line::styled(
                " workspaces",
                Style::default().fg(TEXT).bold(),
            )),
            title_area,
        );
        f.render_widget(Paragraph::new(body), body_area);
        f.render_widget(Paragraph::new(footer), footer_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Agent, Index, Kind, Workspace, WorkspaceSession};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn agent(name: &str, session: &str, workspace: &str, state: State) -> Agent {
        Agent {
            name: name.into(),
            pane: format!("%{name}"),
            kind: Kind::Claude,
            state,
            detail: None,
            title: String::new(),
            location: format!("{session}:1.0"),
            session: session.into(),
            workspace: workspace.into(),
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

    fn workspace(name: &str, session: &str) -> Workspace {
        Workspace {
            name: name.into(),
            sessions: vec![WorkspaceSession {
                id: format!("${session}"),
                name: session.into(),
            }],
        }
    }

    const POPUP: Origin = Origin::Popup("client");

    fn sidebar() -> Sidebar {
        let mut sidebar = Sidebar {
            client_session: Some("$api".into()),
            ..Sidebar::default()
        };
        let index = Index {
            updated_at: crate::index::now_ms(),
            agents: vec![
                agent("lead", "api", "main", State::Idle),
                agent("helper", "web", "client-x", State::Blocked),
            ],
            order: vec!["%lead".into(), "%helper".into()],
            workspaces: vec![workspace("client-x", "web"), workspace("main", "api")],
            workspace: "main".into(),
            ..Index::default()
        };
        sidebar.apply(index, POPUP);
        sidebar
    }

    fn press(sidebar: &mut Sidebar, code: KeyCode) -> bool {
        sidebar.action_key(KeyEvent::from(code), POPUP)
    }

    fn screen(sidebar: &Sidebar) -> String {
        let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
        terminal.draw(|f| sidebar.draw_menu(f, f.area())).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .chunks(38)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_list_shows_only_its_workspace_and_names_it() {
        let sidebar = sidebar();
        assert_eq!(sidebar.workspace.as_deref(), Some("main"));
        assert_eq!(
            sidebar
                .all_agents
                .iter()
                .map(|a| a.name.as_str())
                .collect::<Vec<_>>(),
            ["lead"]
        );
        assert_eq!(
            crate::ui::list_title(&sidebar).to_string(),
            " agents · main "
        );
        assert_eq!(sidebar.everyone.len(), 2);
    }

    #[test]
    fn a_session_without_a_workspace_shows_the_one_you_are_in() {
        let mut sidebar = sidebar();
        sidebar.client_session = Some("$agent-plane-tasks".into());
        let index = Index {
            updated_at: crate::index::now_ms(),
            agents: sidebar.everyone.clone(),
            order: vec!["%lead".into(), "%helper".into()],
            workspaces: sidebar.workspaces.clone(),
            workspace: "client-x".into(),
            ..Index::default()
        };
        sidebar.apply(index, POPUP);
        assert_eq!(sidebar.workspace.as_deref(), Some("client-x"));
        assert_eq!(sidebar.all_agents[0].name, "helper");
    }

    #[test]
    fn w_opens_on_this_workspace_and_marks_what_wants_you_elsewhere() {
        let mut sidebar = sidebar();
        assert!(press(&mut sidebar, KeyCode::Char('w')));
        let text = screen(&sidebar);
        assert!(text.contains(" workspaces"), "{text}");
        let client = text.lines().find(|l| l.contains("client-x")).unwrap();
        assert!(client.trim_end().ends_with('◆'), "{client}");
        assert!(text.lines().any(|l| l.contains("• main  here")), "{text}");
        assert!(text.contains("m move api here"), "{text}");
        // Every key is the menu's: j moves its highlight, not the list's.
        assert!(press(&mut sidebar, KeyCode::Char('k')));
        assert_eq!(sidebar.menu.view.as_ref().unwrap().selected, "client-x");
        assert!(press(&mut sidebar, KeyCode::Char('k')));
        assert_eq!(sidebar.menu.view.as_ref().unwrap().selected, "client-x");
        assert!(press(&mut sidebar, KeyCode::Char('j')));
        assert_eq!(sidebar.menu.view.as_ref().unwrap().selected, "main");
        assert!(press(&mut sidebar, KeyCode::Esc));
        assert!(!sidebar.menu.is_open());
    }

    #[test]
    fn names_are_typed_text_and_enter_on_this_workspace_just_closes() {
        let mut sidebar = sidebar();
        press(&mut sidebar, KeyCode::Char('w'));
        press(&mut sidebar, KeyCode::Char('c'));
        for c in "qmw".chars() {
            assert!(press(&mut sidebar, KeyCode::Char(c)));
        }
        assert!(sidebar.menu.is_open());
        assert!(screen(&sidebar).contains(" new workspace: qmw▏"));
        press(&mut sidebar, KeyCode::Esc);
        assert!(sidebar.menu.is_open());
        press(&mut sidebar, KeyCode::Char('r'));
        assert!(screen(&sidebar).contains(" rename main: main▏"));
        press(&mut sidebar, KeyCode::Esc);
        press(&mut sidebar, KeyCode::Enter);
        assert!(!sidebar.menu.is_open() && !sidebar.left);
    }

    #[test]
    fn ctrl_c_leaves_workspace_menu_and_naming_one_level_at_a_time() {
        let mut sidebar = sidebar();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        press(&mut sidebar, KeyCode::Char('w'));
        assert!(sidebar.action_key(ctrl_c, POPUP));
        assert!(!sidebar.menu.is_open());
        press(&mut sidebar, KeyCode::Char('w'));
        press(&mut sidebar, KeyCode::Char('c'));
        sidebar.action_key(ctrl_c, POPUP);
        assert!(sidebar.menu.view.as_ref().unwrap().naming.is_none());
        assert!(sidebar.menu.is_open());
        sidebar.action_key(ctrl_c, POPUP);
        assert!(!sidebar.menu.is_open());
    }

    #[test]
    fn m_refuses_a_project_already_there_and_w_needs_no_edit_in_progress() {
        let mut sidebar = sidebar();
        press(&mut sidebar, KeyCode::Char('w'));
        press(&mut sidebar, KeyCode::Char('m'));
        assert!(screen(&sidebar).contains("api is already in main"));
        press(&mut sidebar, KeyCode::Char('w'));
        sidebar.start_search();
        assert!(!press(&mut sidebar, KeyCode::Char('w')));
        assert!(!sidebar.menu.is_open());
    }
}
