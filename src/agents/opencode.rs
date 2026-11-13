//! Opt-in official OpenCode TUI plugin snapshots, bound to a live process and pane.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

pub fn launch(task: Option<&str>, agent_args: &[String]) -> Result<Launch> {
    let mut args = agent_args.to_vec();
    args.extend(task.into_iter().flat_map(|s| ["--prompt".into(), s.into()]));
    Ok(Launch {
        command: "opencode",
        args,
    })
}

#[derive(Deserialize)]
struct Published {
    version: u32,
    pid: i32,
    pane_id: String,
    updated_at: u64,
    session_id: String,
    state: String,
    detail: Option<String>,
    title: String,
    completed_turns: u64,
    completion_epoch: String,
}

/// OpenCode continues a session by id.
pub fn resume(session_id: &str) -> Launch {
    Launch {
        command: "opencode",
        args: vec!["--session".into(), session_id.into()],
    }
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    scan_dir(
        context,
        &crate::index::state_dir()?.join("opencode"),
        crate::index::now_ms(),
    )
}

fn scan_dir(context: &ScanContext<'_>, directory: &Path, now: u64) -> Result<Vec<Observation>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("reading {}", directory.display()));
        }
    };
    let mut found = Vec::new();
    let mut occupied = HashSet::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let pid: i32 = path
            .file_stem()
            .and_then(|name| name.to_str())
            .context("OpenCode status filename is not UTF-8")?
            .parse()
            .with_context(|| {
                format!(
                    "OpenCode status filename must be <pid>.json: {}",
                    path.display()
                )
            })?;
        // Gate by filename before parsing: crashed sessions can leave arbitrary old files.
        let Some(process) = context.processes.processes.get(&pid) else {
            continue;
        };
        if process.executable.rsplit('/').next() != Some("opencode") {
            continue;
        }
        let panes: Vec<_> = context
            .panes
            .iter()
            .filter(|pane| context.processes.is_descendant(pid, pane.pid))
            .collect();
        if panes.is_empty() {
            continue;
        }
        if panes.len() != 1 {
            bail!("ambiguous OpenCode PID {pid} pane ancestry");
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let file: Published = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing live OpenCode status {}", path.display()))?;
        let pane = panes[0];
        if file.version != 1
            || file.pid != pid
            || file.pane_id != pane.id
            || file.session_id.is_empty()
            || file.completion_epoch.is_empty()
        {
            bail!(
                "invalid live OpenCode status identity in {}",
                path.display()
            );
        }
        // Age checks reject broken publication; age never determines an agent state.
        if now.saturating_sub(file.updated_at) > 5_000
            || file.updated_at > now.saturating_add(1_000)
        {
            bail!("stale or future live OpenCode status in {}", path.display());
        }
        let state = match file.state.as_str() {
            "working" => RawState::Working,
            "blocked" => RawState::Blocked,
            "idle" => RawState::Idle,
            other => bail!(
                "unknown live OpenCode state {other:?} in {}: {}",
                path.display(),
                file.detail.as_deref().unwrap_or("")
            ),
        };
        if !occupied.insert(pane.id.clone()) {
            bail!("multiple opted-in OpenCode sessions in pane {}", pane.id);
        }
        found.push(Observation {
            pane_id: pane.id.clone(),
            pid,
            state,
            detail: file.detail,
            title: file.title,
            session_id: Some(file.session_id),
            preserve_title: false,
            completed_turns: Some(file.completed_turns),
            completion_epoch: Some(file.completion_epoch),
        });
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Process, ProcessTable};
    use crate::tmux::Pane;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "opencode-test-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, pid: i32, data: &str) {
            std::fs::write(self.0.join(format!("{pid}.json")), data).unwrap();
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn pane(id: &str, pid: i32) -> Pane {
        Pane {
            id: id.into(),
            pid,
            session: "test".into(),
            session_id: "$1".into(),
            workspace: "main".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 1,
            cwd: "/same/cwd".into(),
            title: "irrelevant terminal title".into(),
            active: false,
            viewed: false,
            sidebar: false,
            name: String::new(),
            parent: String::new(),
            placed: false,
            task: false,
        }
    }
    fn processes() -> ProcessTable {
        ProcessTable {
            processes: HashMap::from([
                (
                    42,
                    Process {
                        pid: 42,
                        parent: 10,
                        executable: "/bin/opencode".into(),
                        argv: "opencode".into(),
                    },
                ),
                (
                    43,
                    Process {
                        pid: 43,
                        parent: 11,
                        executable: "/bin/opencode".into(),
                        argv: "opencode".into(),
                    },
                ),
            ]),
        }
    }
    const WORKING: &str = include_str!("../../tests/fixtures/opencode/working.json");

    #[test]
    fn fixtures_map_only_published_states_and_completion_counts() {
        let panes = [pane("%1", 10)];
        let processes = processes();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let directory = Directory::new();
        for (data, expected, detail, completed) in [
            (WORKING, RawState::Working, None, 0),
            (
                include_str!("../../tests/fixtures/opencode/blocked.json"),
                RawState::Blocked,
                Some("permission: bash"),
                0,
            ),
            (
                include_str!("../../tests/fixtures/opencode/question.json"),
                RawState::Blocked,
                Some("question: Which fixture?"),
                0,
            ),
            (
                include_str!("../../tests/fixtures/opencode/idle.json"),
                RawState::Idle,
                None,
                2,
            ),
        ] {
            directory.write(42, data);
            let found = scan_dir(&context, &directory.0, 10_000).unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].state, expected);
            assert_eq!(found[0].detail.as_deref(), detail);
            assert_eq!(found[0].completed_turns, Some(completed));
            assert_eq!(found[0].session_id.as_deref(), Some("ses_one"));
        }
    }

    #[test]
    fn absent_opt_in_does_not_detect_running_opencode() {
        let panes = [pane("%1", 10)];
        let processes = processes();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let directory = Directory::new();
        assert!(scan_dir(&context, &directory.0, 10_000).unwrap().is_empty());
        assert!(
            scan_dir(&context, &directory.0.join("missing"), 10_000)
                .unwrap()
                .is_empty()
        );
        directory.write(
            999,
            include_str!("../../tests/fixtures/opencode/malformed.json"),
        );
        assert!(scan_dir(&context, &directory.0, 10_000).unwrap().is_empty());
    }

    #[test]
    fn malformed_unknown_stale_future_and_wrong_identity_fail_for_live_sessions() {
        let panes = [pane("%1", 10)];
        let processes = processes();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let directory = Directory::new();
        for (data, now) in [
            (
                include_str!("../../tests/fixtures/opencode/malformed.json").to_string(),
                10_000,
            ),
            (
                include_str!("../../tests/fixtures/opencode/unknown.json").to_string(),
                10_000,
            ),
            (WORKING.to_string(), 15_001),
            (WORKING.to_string(), 8_999),
            (WORKING.replace("\"version\":1", "\"version\":2"), 10_000),
            (WORKING.replace("\"pid\":42", "\"pid\":43"), 10_000),
            (WORKING.replace("%1", "%2"), 10_000),
            (WORKING.replace("ses_one", ""), 10_000),
            (WORKING.replace("launch-a", ""), 10_000),
        ] {
            directory.write(42, &data);
            assert!(scan_dir(&context, &directory.0, now).is_err(), "{data}");
        }
    }

    #[test]
    fn same_cwd_panes_follow_pids_and_reject_duplicate_live_sessions() {
        let panes = [pane("%1", 10), pane("%2", 11)];
        let mut processes = processes();
        let directory = Directory::new();
        directory.write(42, WORKING);
        directory.write(
            43,
            &WORKING
                .replace("\"pid\":42", "\"pid\":43")
                .replace("%1", "%2")
                .replace("ses_one", "ses_two"),
        );
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let mut found = scan_dir(&context, &directory.0, 10_000).unwrap();
        found.sort_by(|a, b| a.pane_id.cmp(&b.pane_id));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].pane_id, "%1");
        assert_eq!(found[1].pane_id, "%2");
        processes.processes.get_mut(&43).unwrap().parent = 10;
        directory.write(43, &WORKING.replace("\"pid\":42", "\"pid\":43"));
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(
            scan_dir(&context, &directory.0, 10_000)
                .err()
                .unwrap()
                .to_string()
                .contains("multiple opted-in")
        );
    }

    #[test]
    fn launch_uses_interactive_prompt_flag() {
        assert_eq!(launch(None, &[]).unwrap().command, "opencode");
        assert!(launch(None, &[]).unwrap().args.is_empty());
        assert_eq!(
            launch(Some("task with 'quotes'"), &[]).unwrap().args,
            ["--prompt", "task with 'quotes'"]
        );
    }

    #[test]
    fn launch_puts_agent_args_before_task() {
        let args = ["-m".to_string(), "gpt".to_string()];
        let launch_with = launch(Some("task"), &args).unwrap();
        assert_eq!(launch_with.command, "opencode");
        assert_eq!(launch_with.args, ["-m", "gpt", "--prompt", "task"]);
        assert_eq!(launch(None, &args).unwrap().args, ["-m", "gpt"]);
        assert!(launch(None, &[]).unwrap().args.is_empty());
    }
}
