//! Codex publishes activity and approvals in its terminal title.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::Result;

pub fn launch(task: Option<&str>) -> Result<Launch> {
    Ok(Launch {
        command: "codex",
        args: task.into_iter().map(str::to_owned).collect(),
    })
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let mut found = Vec::new();
    for pane in context.panes {
        // Codex starts helper `codex` children; only the outermost process is the agent.
        // A `codex` run by a Claude Code session in the same pane is that session's tool.
        let all: Vec<_> = context
            .processes
            .descendants_named(pane.pid, "codex")
            .into_iter()
            .filter(|p| {
                !context
                    .processes
                    .has_ancestor_named(p.pid, pane.pid, "claude")
            })
            .collect();
        let processes: Vec<_> = all
            .iter()
            .filter(|p| {
                !all.iter()
                    .any(|a| a.pid != p.pid && context.processes.is_descendant(p.pid, a.pid))
            })
            .collect();
        // Several top-level `codex` processes (parallel one-off runs) do not identify an agent.
        if processes.len() != 1 {
            continue;
        }
        let (state, detail) = codex_state(&pane.title);
        found.push(Observation {
            pane_id: pane.id.clone(),
            state,
            detail,
            title: clean_title(&pane.title),
            session_id: None,
            preserve_title: state == RawState::Blocked,
            completed_turns: None,
            completion_epoch: None,
        });
    }
    Ok(found)
}

fn clean_title(title: &str) -> String {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string()
}

fn codex_state(title: &str) -> (RawState, Option<String>) {
    if title.contains("Action Required") {
        return (RawState::Blocked, Some("action required".to_string()));
    }
    let spinning = title
        .chars()
        .next()
        .is_some_and(|c| ('\u{2800}'..='\u{28FF}').contains(&c));
    if spinning {
        (RawState::Working, None)
    } else {
        (RawState::Idle, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Process, ProcessTable};
    use crate::tmux::Pane;
    use std::collections::HashMap;

    fn process(pid: i32, parent: i32, executable: &str) -> (i32, Process) {
        let process = Process {
            pid,
            parent,
            executable: executable.into(),
            argv: executable.into(),
        };
        (pid, process)
    }

    fn pane(id: &str, pid: i32) -> Pane {
        Pane {
            id: id.into(),
            pid,
            session: "s".into(),
            window: "@1".into(),
            window_index: 0,
            pane_index: 0,
            cwd: "/".into(),
            title: "Fix tests | repo".into(),
            visible: false,
            name: String::new(),
            parent: String::new(),
        }
    }

    #[test]
    fn nested_codex_helpers_belong_to_one_agent() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/codex"),
                process(12, 11, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert_eq!(scan(&context).unwrap().len(), 1);
    }

    #[test]
    fn codex_run_by_claude_in_the_pane_is_not_an_agent() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/claude"),
                process(12, 11, "/bin/codex"),
                process(13, 11, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(scan(&context).unwrap().is_empty());
    }

    #[test]
    fn sibling_codex_processes_are_not_tracked_and_raise_no_fault() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/codex"),
                process(12, 10, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(scan(&context).unwrap().is_empty());
    }

    #[test]
    fn codex_titles() {
        assert_eq!(codex_state("⠼ Fix tests | repo").0, RawState::Working);
        assert_eq!(codex_state("[ ! ] Action Required").0, RawState::Blocked);
        assert_eq!(codex_state("[ . ] Action Required").0, RawState::Blocked);
        assert_eq!(codex_state("Fix tests | repo").0, RawState::Idle);
    }

    #[test]
    fn titles_lose_status_glyphs() {
        assert_eq!(
            clean_title("✳ Tmux agent monitoring"),
            "Tmux agent monitoring"
        );
        assert_eq!(clean_title("⠼ Fix tests | repo"), "Fix tests | repo");
        assert_eq!(clean_title("[ ! ] Action Required"), "Action Required");
    }
}
