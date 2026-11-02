//! Codex publishes activity and approvals in its terminal title.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Result, bail};

pub fn launch(task: Option<&str>) -> Launch {
    Launch {
        command: "codex",
        args: task.into_iter().map(str::to_owned).collect(),
    }
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let mut found = Vec::new();
    for pane in context.panes {
        let processes = context.processes.descendants_named(pane.pid, "codex");
        if processes.len() > 1 {
            bail!("multiple interactive Codex processes in pane {}", pane.id);
        }
        if processes.is_empty() {
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
