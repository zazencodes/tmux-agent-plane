//! The sidebar's background tasks: `t` shows them below the agents, where `Enter` opens one, `x`
//! asks to stop it and `R` restarts it. Sidebar only: the picker never shows tasks.

use super::{Origin, Sidebar, kill_accepts, switch_to};
use crate::index::State;
use crate::tasks::{self, Status, Task};
use crate::tmux;
use crate::ui::{OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, colour, truncate};
use anyhow::Result;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::ListItem;
use std::collections::HashSet;
use std::path::Path;

#[derive(Default)]
pub(crate) struct Section {
    /// `t`: this sidebar lists its tasks. Each sidebar keeps its own.
    pub(crate) shown: bool,
    /// Every task in the index, less those just stopped here.
    all: Vec<Task>,
    /// Stopped from this sidebar: hidden until the daemon drops them from the index.
    stopped: HashSet<String>,
    /// `x` pinned this task and waits for Enter or `y`.
    pub(crate) stopping: Option<Task>,
}

impl Section {
    pub(crate) fn apply(&mut self, tasks: Vec<Task>) {
        self.stopped
            .retain(|pane| tasks.iter().any(|t| &t.pane == pane));
        self.all = tasks
            .into_iter()
            .filter(|t| !self.stopped.contains(&t.pane))
            .collect();
    }

    pub(crate) fn clear(&mut self) {
        self.all.clear();
    }

    /// Whether any task exists, shown or not.
    pub(crate) fn exist(&self) -> bool {
        !self.all.is_empty()
    }

    /// The task rows, below the agent rows.
    pub(crate) fn rows(&self) -> &[Task] {
        if self.shown { &self.all } else { &[] }
    }
}

impl Sidebar {
    /// Agent rows, then task rows.
    pub(crate) fn row_count(&self) -> usize {
        self.agents.len() + self.tasks.rows().len()
    }

    pub(crate) fn selected_task(&self) -> Option<&Task> {
        let i = self.list.selected()?.checked_sub(self.agents.len())?;
        self.tasks.rows().get(i)
    }

    /// The list row of a task pane.
    pub(crate) fn task_row(&self, pane: &str) -> Option<usize> {
        self.tasks
            .rows()
            .iter()
            .position(|t| t.pane == pane)
            .map(|i| i + self.agents.len())
    }

    /// Session, window and pane of any row.
    fn row_target(&self, row: usize) -> Option<(&str, &str, &str)> {
        match self.agents.get(row) {
            Some(a) => Some((&a.session, &a.window, &a.pane)),
            None => self
                .tasks
                .rows()
                .get(row - self.agents.len())
                .map(|t| (t.session.as_str(), t.window.as_str(), t.pane.as_str())),
        }
    }

    /// `t`, and `x` / `R` on a task row. A stop waiting for its answer consumes every key: Enter
    /// or `y` stops, any other key only cancels.
    pub(crate) fn task_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        let Origin::Sidebar(own) = origin else {
            return false;
        };
        if let Some(task) = self.tasks.stopping.take() {
            self.notice = None;
            if kill_accepts(key) {
                match self.confirm_stop(&task, own) {
                    Ok(next) => {
                        self.tasks.stopped.insert(task.pane.clone());
                        self.tasks.all.retain(|t| t.pane != task.pane);
                        self.navigation.pane = next;
                        self.rebuild();
                    }
                    Err(error) => self.notice = Some(format!("{error:#}")),
                }
            }
            return true;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL)
            || self.editing.is_some()
            || self.typing_query()
            || self.killing.is_some()
            || self.new_agent.is_some()
        {
            return false;
        }
        match key.code {
            KeyCode::Char('t') => {
                self.notice = None;
                self.tasks.shown = !self.tasks.shown;
                if self.tasks.shown && !self.tasks.exist() {
                    self.notice = Some("no background tasks".into());
                }
                self.rebuild();
            }
            KeyCode::Char('x') => {
                let Some(task) = self.selected_task().cloned() else {
                    return false;
                };
                self.notice = None;
                self.tasks.stopping = Some(task);
            }
            KeyCode::Char('R') => {
                let Some(task) = self.selected_task().cloned() else {
                    return false;
                };
                // The row shows it running again on the next read; only a failure needs words.
                self.notice = tasks::restart(&task.definition.name, Some(&task.pane))
                    .err()
                    .map(|error| format!("{error:#}"));
            }
            _ => return false,
        }
        true
    }

    /// Move off to the next row first, as a kill does, then stop the pinned task. Returns the
    /// row to select.
    fn confirm_stop(&self, task: &Task, own: &str) -> Result<Option<String>> {
        let row = self
            .task_row(&task.pane)
            .ok_or_else(|| anyhow::anyhow!("task left the list; stop cancelled"))?;
        let next = self
            .row_target(row + 1)
            .or_else(|| row.checked_sub(1).and_then(|row| self.row_target(row)));
        if let Some((session, window, pane)) = next
            && let Some(command) = switch_to(own, session, window, pane, false)?
        {
            let command: Vec<_> = command.iter().map(String::as_str).collect();
            tmux::run(&command)?;
        }
        tasks::stop(&task.definition.name, Some(&task.pane))?;
        Ok(next.map(|(_, _, pane)| pane.to_owned()))
    }
}

fn project(task: &Task) -> String {
    Path::new(&task.definition.cwd)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&task.definition.cwd)
        .to_owned()
}

/// What a row says about a task's state, in its colour.
fn state(task: &Task) -> (String, Style) {
    match task.status {
        Status::Running => (
            tasks::ports_label(&task.ports, " ").unwrap_or_default(),
            Style::default().fg(TEXT),
        ),
        Status::Exited { exit_code, .. } => (format!("exit {exit_code}"), exit_style(exit_code)),
    }
}

fn exit_style(exit_code: i32) -> Style {
    Style::default().fg(if exit_code == 0 {
        OVERLAY
    } else {
        colour(State::Blocked)
    })
}

fn icon(task: &Task) -> (&'static str, Style) {
    match task.status {
        Status::Running => ("▸", Style::default().fg(colour(State::Done))),
        Status::Exited { exit_code, .. } => ("✕", exit_style(exit_code)),
    }
}

/// Since it started, or since it ended.
fn since(task: &Task) -> u64 {
    match task.status {
        Status::Running => task.started_at,
        Status::Exited { exited_at, .. } => exited_at,
    }
}

/// One line per task under a `── tasks ──` rule: `▸ web-dev  :5173  pro  3h`, or
/// `✕ worker  exit 1  pro  2m`. `selected` counts from the first task. No counts: they live in
/// the status bar.
pub(crate) fn items(
    tasks: &[Task],
    selected: Option<usize>,
    in_use: Option<&str>,
    width: usize,
) -> Vec<ListItem<'static>> {
    tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let mut lines = Vec::new();
            if i == 0 {
                lines.push(Line::default());
                lines.push(Line::from(vec![
                    Span::styled("── ", SURFACE2),
                    Span::styled("tasks ", Style::default().fg(SUBTEXT).bold()),
                    Span::styled("─".repeat(width.saturating_sub(10)), SURFACE2),
                ]));
            }
            let is_selected = selected == Some(i);
            let (icon, icon_style) = icon(task);
            let (state, state_style) = state(task);
            let age = crate::ui::age(since(task));
            let state = truncate(&state, 13);
            let project = truncate(&project(task), 10);
            // Narrow rows drop the project, then the state, before the name gets too short.
            let end = [(true, true), (true, false), (false, false)]
                .into_iter()
                .map(|(with_state, with_project)| {
                    let mut end = Vec::new();
                    if with_state && !state.is_empty() {
                        end.push(Span::styled(state.clone(), state_style));
                        end.push(Span::raw("  "));
                    }
                    if with_project {
                        end.push(Span::styled(project.clone(), Style::default().fg(SUBTEXT)));
                        end.push(Span::raw("  "));
                    }
                    end.push(Span::styled(format!("{age} "), OVERLAY));
                    end
                })
                .find(|end| {
                    let used: usize = end.iter().map(Span::width).sum();
                    used + 4 + task.definition.name.chars().count().min(6) <= width
                })
                .unwrap_or_default();
            let end_width: usize = end.iter().map(Span::width).sum();
            let name_width = width.saturating_sub(end_width + 4).max(1);
            let mut name = Style::default().fg(TEXT);
            if in_use == Some(task.pane.as_str()) {
                name = name.bold();
            }
            let mut spans = vec![
                Span::styled(if is_selected { "▌" } else { " " }, icon_style),
                Span::styled(format!("{icon} "), icon_style),
                Span::styled(truncate(&task.definition.name, name_width), name),
            ];
            let used: usize = spans.iter().map(Span::width).sum();
            spans.push(Span::raw(
                " ".repeat(width.saturating_sub(used + end_width)),
            ));
            spans.extend(end);
            let line = Line::from(spans);
            lines.push(if is_selected {
                line.style(Style::default().bg(SURFACE0))
            } else {
                line
            });
            ListItem::new(lines)
        })
        .collect()
}

/// The stop question, styled like a kill's: the name, then its state and `[Y/n]`.
pub(crate) fn stop_footer(task: &Task, width: usize) -> Vec<Line<'static>> {
    let name = &task.definition.name;
    let narrow = name.chars().count() + 6 > width;
    let label = task.status.label();
    let (first, second) = if narrow {
        (truncate(name, width), format!("stop {label}? [Y/n]"))
    } else {
        (format!(" stop {name}"), format!(" ({label})? [Y/n]"))
    };
    let style = match task.status {
        Status::Running => Style::default().fg(colour(State::Done)),
        Status::Exited { exit_code, .. } => exit_style(exit_code),
    };
    vec![
        Line::styled(first, Style::default().fg(TEXT).bold()),
        Line::styled(truncate(&second, width), style),
    ]
}

#[cfg(test)]
mod tests {
    use super::super::tests::{agent, rendered, showing};
    use super::*;
    use crate::tasks::Definition;

    fn task(name: &str, status: Status, ports: &[u16]) -> Task {
        Task {
            definition: Definition {
                name: name.into(),
                cwd: "/Users/a/pro/site".into(),
                command: vec!["npm".into(), "run".into(), "dev".into()],
                started_by: Some("agent-%1".into()),
            },
            pane: format!("%{}", 50 + name.len()),
            session: tasks::SESSION.into(),
            window: format!("@{}", 50 + name.len()),
            location: format!("{}:1.0", tasks::SESSION),
            started_at: crate::index::now_ms(),
            status,
            ports: ports.to_vec(),
        }
    }

    fn with_tasks() -> Sidebar {
        let mut sidebar = showing(vec![agent("%1", State::Idle), agent("%2", State::Working)]);
        sidebar.tasks.apply(vec![
            task("web-dev", Status::Running, &[5173]),
            task(
                "worker",
                Status::Exited {
                    exit_code: 1,
                    exited_at: crate::index::now_ms(),
                },
                &[],
            ),
        ]);
        sidebar.rebuild();
        sidebar
    }

    const OWN: Origin<'static> = Origin::Sidebar("%99");

    fn key(sidebar: &mut Sidebar, code: KeyCode) -> bool {
        let key = KeyEvent::new(code, KeyModifiers::NONE);
        sidebar.action_key(key, OWN) || sidebar.key(key)
    }

    #[test]
    fn t_toggles_a_tasks_section_below_the_agents_and_the_hint_needs_tasks() {
        let mut sidebar = with_tasks();
        let screen = rendered(&mut sidebar);
        assert!(!screen.contains("── tasks"));
        assert!(screen.contains("t tasks"));
        assert!(key(&mut sidebar, KeyCode::Char('t')));
        let screen = rendered(&mut sidebar);
        assert!(screen.contains("── tasks ─"), "{screen}");
        let web = screen.lines().find(|l| l.contains("web-dev")).unwrap();
        assert!(
            web.contains("▸ web-dev") && web.contains(":5173") && web.contains("site"),
            "{web}"
        );
        let worker = screen.lines().find(|l| l.contains("worker")).unwrap();
        assert!(
            worker.contains("✕ worker") && worker.contains("exit 1"),
            "{worker}"
        );
        let rule = screen.lines().position(|l| l.contains("── tasks")).unwrap();
        let lines: Vec<&str> = screen.lines().collect();
        let last_agent = lines.iter().rposition(|l| l.contains("agent-%")).unwrap();
        assert!(rule > last_agent);
        // No counts in the list.
        assert!(!screen.contains("2 tasks"));
        assert!(key(&mut sidebar, KeyCode::Char('t')));
        assert!(!rendered(&mut sidebar).contains("── tasks"));
        let mut empty = showing(vec![agent("%1", State::Idle)]);
        assert!(!rendered(&mut empty).contains("t tasks"));
        assert!(key(&mut empty, KeyCode::Char('t')));
        assert_eq!(empty.notice.as_deref(), Some("no background tasks"));
    }

    #[test]
    fn j_and_k_go_through_agents_then_tasks_and_selection_survives_refresh() {
        let mut sidebar = with_tasks();
        sidebar.select(Some(0));
        key(&mut sidebar, KeyCode::Char('t'));
        let mut seen = Vec::new();
        for _ in 0..4 {
            seen.push(sidebar.navigation.pane.clone().unwrap());
            key(&mut sidebar, KeyCode::Char('j'));
        }
        assert_eq!(seen, ["%1", "%2", "%57", "%56"]);
        assert_eq!(sidebar.navigation.pane.as_deref(), Some("%1"));
        key(&mut sidebar, KeyCode::Char('G'));
        assert_eq!(sidebar.selected_task().unwrap().definition.name, "worker");
        sidebar.rebuild();
        assert_eq!(sidebar.selected_task().unwrap().definition.name, "worker");
        assert!(rendered(&mut sidebar).contains("x stop"));
        key(&mut sidebar, KeyCode::Char('k'));
        assert_eq!(sidebar.selected_task().unwrap().definition.name, "web-dev");
        assert!(sidebar.selected().is_none());
        // Hiding tasks moves the selection back to an agent.
        key(&mut sidebar, KeyCode::Char('t'));
        assert!(sidebar.selected().is_some());
        assert_eq!(sidebar.row_count(), 2);
    }

    #[test]
    fn x_asks_to_stop_a_task_and_any_other_key_cancels_without_replay() {
        let mut sidebar = with_tasks();
        key(&mut sidebar, KeyCode::Char('t'));
        sidebar.select(sidebar.task_row("%57"));
        assert!(key(&mut sidebar, KeyCode::Char('x')));
        assert_eq!(sidebar.tasks.stopping.as_ref().unwrap().pane, "%57");
        assert!(sidebar.killing.is_none());
        let screen = rendered(&mut sidebar);
        assert!(screen.contains("stop web-dev"), "{screen}");
        assert!(screen.contains("(running)? [Y/n]"), "{screen}");
        let before = sidebar.navigation.pane.clone();
        assert!(key(&mut sidebar, KeyCode::Char('j')));
        assert!(sidebar.tasks.stopping.is_none());
        assert_eq!(sidebar.navigation.pane, before);
        // On an agent row x is still a kill.
        sidebar.select(Some(0));
        assert!(!sidebar.task_key(KeyEvent::from(KeyCode::Char('x')), OWN));
        assert!(!sidebar.task_key(KeyEvent::from(KeyCode::Char('R')), OWN));
    }

    #[test]
    fn the_picker_never_shows_or_acts_on_tasks() {
        let mut sidebar = with_tasks();
        let picker = Origin::Popup("client");
        assert!(!sidebar.task_key(KeyEvent::from(KeyCode::Char('t')), picker));
        assert!(!sidebar.tasks.shown);
        assert_eq!(sidebar.row_count(), 2);
    }

    #[test]
    fn tasks_alone_still_show_under_the_agent_message() {
        let mut sidebar = showing(Vec::new());
        sidebar
            .tasks
            .apply(vec![task("web-dev", Status::Running, &[])]);
        key(&mut sidebar, KeyCode::Char('t'));
        let screen = rendered(&mut sidebar);
        assert!(screen.contains("no agents running"), "{screen}");
        assert!(screen.contains("▸ web-dev"), "{screen}");
        assert!(sidebar.selected_task().is_some());
    }

    #[test]
    fn tasks_stay_below_family_when_agents_overflow() {
        let mut sidebar = with_tasks();
        sidebar.all_agents = crate::ui::family::tests::family_agents();
        for i in 10..40 {
            sidebar
                .all_agents
                .push(agent(&format!("%{i}"), State::Idle));
        }
        sidebar.rebuild();
        sidebar.select(sidebar.agents.iter().position(|a| a.pane == "%2"));
        key(&mut sidebar, KeyCode::Char('t'));
        key(&mut sidebar, KeyCode::Char('p'));
        let screen = rendered(&mut sidebar);
        let family = screen.find(" graph ").expect(&screen);
        let tasks = screen.find("── tasks").expect(&screen);
        assert!(family < tasks, "{screen}");
        assert!(
            screen.contains("web-dev") && screen.contains("worker"),
            "{screen}"
        );
        key(&mut sidebar, KeyCode::Char('t'));
        let screen = rendered(&mut sidebar);
        assert!(!screen.contains("── tasks"), "{screen}");
    }

    #[test]
    fn quota_stays_at_the_bottom_below_graph_and_overflowing_tasks() {
        let mut sidebar = with_tasks();
        sidebar.all_agents = crate::ui::family::tests::family_agents();
        for i in 10..40 {
            sidebar
                .all_agents
                .push(agent(&format!("%{i}"), State::Idle));
        }
        sidebar.rebuild();
        sidebar.select(sidebar.agents.iter().position(|a| a.pane == "%2"));
        key(&mut sidebar, KeyCode::Char('t'));
        key(&mut sidebar, KeyCode::Char('p'));
        let u = KeyEvent::from(KeyCode::Char('u'));
        assert!(sidebar.quota_key(u));
        let screen = rendered(&mut sidebar);
        let quota_row = |screen: &str| screen.lines().position(|line| line.contains(" quota "));
        assert!(
            screen.contains("agents") && screen.contains("u collapse"),
            "{screen}"
        );
        assert!(quota_row(&screen).unwrap() > 0);
        assert!(sidebar.quota_key(u));
        let screen = rendered(&mut sidebar);
        assert_eq!(quota_row(&screen), Some(15), "{screen}");
        assert!(screen.contains("u expand"));
    }

    #[test]
    fn rows_fit_narrow_widths() {
        let long = task(
            "a-very-long-task-name-indeed",
            Status::Running,
            &[3000, 3001, 3002, 3003],
        );
        for width in [12, 20, 36] {
            for item in items(std::slice::from_ref(&long), Some(0), None, width) {
                assert!(item.width() <= width, "{width}");
            }
        }
        let footer = stop_footer(&long, 20);
        assert!(footer.iter().all(|line| line.width() <= 20));
        assert!(footer[1].to_string().contains("[Y/n]"));
    }

    #[test]
    fn task_keys_have_help_entries() {
        let help = crate::sidebar_help::tests::labels();
        for label in ["t", "x", "R"] {
            assert!(help.contains(&label), "{label}");
        }
    }
}
