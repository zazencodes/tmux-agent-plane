//! The sidebar's restore: a top line while agents from before tmux restarted wait, and `b` to see
//! them and choose: `Enter` brings back agents and tasks, `a` agents only, `d` forgets them, Esc
//! cancels. It asks every time. Sidebar only: the picker never offers it.

use super::{Origin, Sidebar, cancels};
use crate::index::State;
use crate::restore::{self, Plan, Report};
use crate::ui::{OVERLAY, SUBTEXT, TEXT, colour, hints, truncate};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

/// The status bar's `↺` colour.
const BLUE: Color = Color::Rgb(0x89, 0xb4, 0xfa);

#[derive(Default)]
pub(crate) struct Restore {
    /// From the index: agents a restore would bring back.
    pub(crate) pending: usize,
    view: Option<View>,
}

enum View {
    /// `b`: what would come back, waiting for a choice.
    Confirm(Plan),
    /// A choice was made; it runs once the screen says so. True with tasks.
    Running(bool),
    /// What the restore or discard did, until any key.
    Done(Vec<(String, Style)>),
}

impl Restore {
    pub(crate) fn is_open(&self) -> bool {
        self.view.is_some()
    }

    /// The restore chosen and not yet run, with or without tasks.
    pub(crate) fn chosen(&self) -> Option<bool> {
        match self.view {
            Some(View::Running(with_tasks)) => Some(with_tasks),
            _ => None,
        }
    }

    /// Run the chosen restore and show what it did.
    pub(crate) fn run(&mut self) {
        let Some(with_tasks) = self.chosen() else {
            return;
        };
        self.pending = 0;
        self.view = Some(View::Done(match restore::perform(with_tasks) {
            Ok(report) => report_lines(&report),
            Err(error) => vec![(format!("restore failed: {error:#}"), failed())],
        }));
    }

    /// Whole-pane, like the help.
    pub(crate) fn draw(&self, f: &mut Frame, area: Rect) {
        let width = area.width as usize;
        let (title, mut body, footer): (_, Vec<Line>, Vec<Line>) = match &self.view {
            None => return,
            Some(View::Confirm(plan)) => (
                format!(" bring back {} agents?", plan.agents.len()),
                confirm_lines(plan, width),
                vec![
                    hints(&[("⏎", "agents + tasks"), ("a", "agents only")]),
                    hints(&[("d", "discard"), ("esc", "cancel")]),
                ],
            ),
            Some(View::Running(_)) => (
                " restoring…".into(),
                Vec::new(),
                vec![Line::styled(" bringing agents back", SUBTEXT)],
            ),
            Some(View::Done(lines)) => (
                " restore".into(),
                lines
                    .iter()
                    .flat_map(|(text, style)| {
                        wrap(text, width.saturating_sub(1))
                            .into_iter()
                            .map(|line| Line::styled(format!(" {line}"), *style))
                    })
                    .collect(),
                vec![Line::styled(" any key closes", SUBTEXT)],
            ),
        };
        let [title_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(footer.len() as u16),
        ])
        .areas(area);
        f.render_widget(
            Paragraph::new(Line::styled(
                truncate(&title, width),
                Style::default().fg(TEXT).bold(),
            )),
            title_area,
        );
        let room = body_area.height as usize;
        if body.len() > room && room > 0 {
            let hidden = body.len() - room + 1;
            body.truncate(room - 1);
            body.push(Line::styled(
                truncate(
                    &format!(" … {hidden} more: tmux-agent-plane restore"),
                    width,
                ),
                OVERLAY,
            ));
        }
        f.render_widget(Paragraph::new(body), body_area);
        f.render_widget(Paragraph::new(footer), footer_area);
    }
}

fn failed() -> Style {
    Style::default().fg(colour(State::Blocked))
}

fn heading(text: &str, width: usize) -> Line<'static> {
    Line::styled(truncate(&format!(" {text}"), width), SUBTEXT)
}

fn confirm_lines(plan: &Plan, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = plan
        .agents
        .iter()
        .map(|a| {
            Line::styled(
                truncate(
                    &format!(" ▸ {}  {}  {}", a.name, a.kind.as_str(), a.project),
                    width,
                ),
                TEXT,
            )
        })
        .collect();
    if !plan.skipped.is_empty() {
        lines.push(Line::default());
        lines.push(heading("cannot come back", width));
        for (a, why) in &plan.skipped {
            lines.push(Line::styled(
                truncate(
                    &format!(" ✕ {}  {}  {}", a.name, a.kind.as_str(), a.project),
                    width,
                ),
                failed(),
            ));
            lines.extend(
                wrap(why, width.saturating_sub(3))
                    .into_iter()
                    .map(|line| Line::styled(format!("   {line}"), OVERLAY)),
            );
        }
    }
    if !plan.tasks.is_empty() {
        lines.push(Line::default());
        lines.push(heading("tasks that were running", width));
        for task in &plan.tasks {
            let by = task.started_by.as_deref().unwrap_or("-");
            lines.push(Line::styled(
                truncate(&format!(" ▸ {}  by {by}", task.name), width),
                TEXT,
            ));
        }
    }
    lines
}

fn report_lines(report: &Report) -> Vec<(String, Style)> {
    let mut lines: Vec<(String, Style)> = report
        .done
        .iter()
        .map(|line| (line.clone(), Style::default().fg(TEXT)))
        .collect();
    lines.extend(
        report
            .notes
            .iter()
            .map(|line| (line.clone(), Style::default().fg(SUBTEXT))),
    );
    lines.extend(
        report
            .failed
            .iter()
            .map(|line| (format!("failed: {line}"), failed())),
    );
    if lines.is_empty() {
        lines.push(("nothing came back".into(), Style::default().fg(SUBTEXT)));
    }
    lines
}

/// Words onto lines of at most `width` columns; a longer word is cut.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let length = line.chars().count();
        if length > 0 && length + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
        if line.chars().count() > width {
            lines.push(truncate(&line, width));
            line.clear();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// The list's top line while agents wait, wrapped to the list's width.
pub(crate) fn top_lines(pending: usize, width: usize) -> Vec<Line<'static>> {
    if pending == 0 {
        return Vec::new();
    }
    wrap(
        &format!("↺ {pending} agents from before tmux restarted · b restore"),
        width.saturating_sub(1),
    )
    .into_iter()
    .map(|line| Line::styled(format!(" {line}"), BLUE))
    .collect()
}

impl Sidebar {
    /// `b`, and every key while the restore view is open: it consumes them all.
    pub(crate) fn restore_key(&mut self, key: KeyEvent, origin: Origin<'_>) -> bool {
        let Origin::Sidebar(_) = origin else {
            return false;
        };
        match self.restore.view.take() {
            Some(View::Confirm(plan)) => {
                self.restore.view = match key.code {
                    KeyCode::Enter => Some(View::Running(true)),
                    KeyCode::Char('a') => Some(View::Running(false)),
                    KeyCode::Char('d') => Some(View::Done(match restore::discard() {
                        Ok(count) => {
                            self.restore.pending = 0;
                            vec![(
                                format!("forgot {count} agents from before tmux restarted"),
                                Style::default().fg(TEXT),
                            )]
                        }
                        Err(error) => vec![(format!("discard failed: {error:#}"), failed())],
                    })),
                    _ if cancels(key) => None,
                    _ => Some(View::Confirm(plan)),
                };
                return true;
            }
            Some(View::Running(with_tasks)) => {
                self.restore.view = Some(View::Running(with_tasks));
                return true;
            }
            Some(View::Done(_)) => return true,
            None => {}
        }
        if key.code != KeyCode::Char('b')
            || key.modifiers.contains(KeyModifiers::CONTROL)
            || self.editing.is_some()
            || self.typing_query()
            || self.killing.is_some()
            || self.new_agent.is_some()
            || self.tasks.stopping.is_some()
            || self.auto_name.open
            || self.help.open
            || self.quota.open
        {
            return false;
        }
        self.notice = None;
        match restore::current() {
            Ok(Some(plan)) => self.restore.view = Some(View::Confirm(plan)),
            Ok(None) => self.notice = Some("nothing to restore".into()),
            Err(error) => self.notice = Some(format!("{error:#}")),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::Kind;
    use crate::restore::Listed;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    const OWN: Origin<'static> = Origin::Sidebar("%9");

    fn plan() -> Plan {
        let listed = |name: &str, kind| Listed {
            name: name.into(),
            kind,
            project: "pro".into(),
            cwd: "/work/pro".into(),
        };
        Plan {
            windows: Vec::new(),
            agents: vec![listed("reviewer", Kind::Claude)],
            skipped: vec![(
                listed("codex-2", Kind::Codex),
                "Codex publishes no session id to resume".into(),
            )],
            tasks: vec![crate::tasks::Definition {
                name: "web".into(),
                cwd: "/work/pro".into(),
                command: vec!["npm".into()],
                started_by: Some("reviewer".into()),
            }],
        }
    }

    fn confirming() -> Sidebar {
        let mut sidebar = Sidebar::default();
        sidebar.restore.pending = 1;
        sidebar.restore.view = Some(View::Confirm(plan()));
        sidebar
    }

    fn press(sidebar: &mut Sidebar, code: KeyCode) -> bool {
        sidebar.action_key(KeyEvent::from(code), OWN)
    }

    #[test]
    fn enter_brings_back_agents_and_tasks_and_a_agents_only() {
        let mut sidebar = confirming();
        assert!(press(&mut sidebar, KeyCode::Enter));
        assert_eq!(sidebar.restore.chosen(), Some(true));
        // Keys while it runs change nothing.
        assert!(press(&mut sidebar, KeyCode::Char('d')));
        assert_eq!(sidebar.restore.chosen(), Some(true));
        let mut sidebar = confirming();
        assert!(press(&mut sidebar, KeyCode::Char('a')));
        assert_eq!(sidebar.restore.chosen(), Some(false));
    }

    #[test]
    fn esc_cancels_other_keys_wait_and_nothing_is_remembered() {
        let mut sidebar = confirming();
        for code in [KeyCode::Char('j'), KeyCode::Char('x'), KeyCode::Char('q')] {
            assert!(press(&mut sidebar, code));
            assert!(matches!(sidebar.restore.view, Some(View::Confirm(_))));
        }
        assert!(press(&mut sidebar, KeyCode::Esc));
        assert!(!sidebar.restore.is_open());
        assert_eq!(sidebar.restore.chosen(), None);
        assert_eq!(sidebar.restore.pending, 1);
        // A result closes on any key, which does nothing else.
        sidebar.restore.view = Some(View::Done(vec![("done".into(), Style::default())]));
        assert!(press(&mut sidebar, KeyCode::Char('q')));
        assert!(!sidebar.restore.is_open());
    }

    #[test]
    fn b_is_the_sidebars_only_and_waits_for_other_prompts() {
        let mut sidebar = Sidebar::default();
        let b = KeyEvent::from(KeyCode::Char('b'));
        assert!(!sidebar.restore_key(b, Origin::Popup("client")));
        sidebar.editing = Some(super::super::Rename {
            pane: "%1".into(),
            name: "x".into(),
        });
        assert!(!sidebar.restore_key(b, OWN));
    }

    #[test]
    fn the_quota_view_never_opens_over_the_question() {
        let mut sidebar = confirming();
        assert!(!sidebar.quota_key(KeyEvent::from(KeyCode::Char('u'))));
        assert!(!sidebar.quota.open);
        assert!(!sidebar.quota.pinned);
        assert!(matches!(sidebar.restore.view, Some(View::Confirm(_))));
    }

    #[test]
    fn the_confirmation_lists_agents_reasons_and_tasks_with_their_keys() {
        let sidebar = confirming();
        let mut terminal = Terminal::new(TestBackend::new(38, 16)).unwrap();
        terminal
            .draw(|f| sidebar.restore.draw(f, f.area()))
            .unwrap();
        let screen: Vec<String> = terminal
            .backend()
            .buffer()
            .content
            .chunks(38)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect();
        let text = screen.join("\n");
        for expected in [
            " bring back 1 agents?",
            " ▸ reviewer  claude  pro",
            " cannot come back",
            " ✕ codex-2  codex  pro",
            "   Codex publishes no session id to",
            " tasks that were running",
            " ▸ web  by reviewer",
            " ⏎ agents + tasks  a agents only",
            " d discard  esc cancel",
        ] {
            assert!(text.contains(expected), "{expected:?} in\n{text}");
        }
    }

    #[test]
    fn the_top_line_wraps_to_the_list_and_is_absent_without_pending_agents() {
        assert!(top_lines(0, 36).is_empty());
        let lines: Vec<String> = top_lines(6, 36).iter().map(|l| l.to_string()).collect();
        assert_eq!(
            lines,
            [" ↺ 6 agents from before tmux", " restarted · b restore"]
        );
        let wide: Vec<String> = top_lines(6, 80).iter().map(|l| l.to_string()).collect();
        assert_eq!(wide, [" ↺ 6 agents from before tmux restarted · b restore"]);
    }
}
