//! Pi's opt-in extension publishes atomic live status snapshots, never session history.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

/// The publisher refreshes each second. Expiry is an error, never an idle signal.
const STALE_AFTER_MS: u64 = 5_000;

pub fn launch(task: Option<&str>, agent_args: &[String]) -> Result<Launch> {
    let mut args = agent_args.to_vec();
    // End option parsing so tasks beginning with '-' remain interactive messages.
    args.extend(task.into_iter().flat_map(|s| ["--".into(), s.into()]));
    Ok(Launch {
        command: "pi",
        args,
    })
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    scan_directory(
        context,
        &crate::index::state_dir()?.join("pi"),
        crate::index::now_ms,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Status {
    version: u32,
    pid: i32,
    pane_id: String,
    session_id: String,
    completion_epoch: String,
    state: String,
    detail: Option<String>,
    title: String,
    completed_turns: u64,
    updated_at: u64,
}

fn scan_directory(
    context: &ScanContext<'_>,
    directory: &Path,
    now: impl Fn() -> u64,
) -> Result<Vec<Observation>> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", directory.display())),
    };
    let mut observations = Vec::new();
    let mut occupied = HashSet::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let pid: i32 = path
            .file_stem()
            .and_then(|s| s.to_str())
            .context("Pi status filename is not UTF-8")?
            .parse()
            .with_context(|| format!("invalid Pi status filename {}", path.display()))?;
        // The filename lets us ignore even malformed files left by dead processes.
        if !context.processes.processes.contains_key(&pid) {
            continue;
        }
        let panes: Vec<_> = context
            .panes
            .iter()
            .filter(|p| context.processes.is_descendant(pid, p.pid))
            .collect();
        // A different tmux server can have the same pane ID. PID ancestry is authoritative.
        if panes.is_empty() {
            continue;
        }
        let process = &context.processes.processes[&pid];
        // An exiting process loses its command line before it leaves the process table.
        if process.argv.is_empty() {
            continue;
        }
        if process.argv.trim() != "pi" {
            bail!("live Pi status PID {pid} is not an interactive Pi process");
        }
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let status: Status = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid live Pi status {}", path.display()))?;
        if status.version != 1
            || status.pid != pid
            || status.session_id.is_empty()
            || status.completion_epoch.is_empty()
        {
            bail!("invalid live Pi identity/version in {}", path.display());
        }
        if panes.len() != 1 || panes[0].id != status.pane_id {
            bail!(
                "Pi PID {pid} does not uniquely match published pane {}",
                status.pane_id
            );
        }
        if !occupied.insert(status.pane_id.clone()) {
            bail!("multiple live Pi sessions in pane {}", status.pane_id);
        }
        let now = now();
        if status.updated_at > now || now - status.updated_at > STALE_AFTER_MS {
            bail!("stale or future live Pi status in {}", path.display());
        }
        let state = match status.state.as_str() {
            "working" => RawState::Working,
            "blocked" => RawState::Blocked,
            "idle" => RawState::Idle,
            other => bail!("unknown live Pi state {other:?} in {}", path.display()),
        };
        observations.push(Observation {
            pane_id: status.pane_id,
            state,
            detail: status.detail,
            title: status.title,
            session_id: Some(status.session_id),
            completed_turns: Some(status.completed_turns),
            completion_epoch: Some(status.completion_epoch),
            preserve_title: false,
        });
    }
    Ok(observations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Process, ProcessTable};
    use crate::tmux::Pane;
    use serde_json::{Value, json};
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const WORKING: &str = include_str!("../../tests/fixtures/pi/working.json");
    const BLOCKED: &str = include_str!("../../tests/fixtures/pi/blocked.json");
    const IDLE: &str = include_str!("../../tests/fixtures/pi/idle.json");

    struct Fixture {
        directory: PathBuf,
        panes: Vec<Pane>,
        processes: ProcessTable,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target")
                .join(format!(
                    "pi-fixture-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&directory).unwrap();
            let pane = Pane {
                id: "%1".into(),
                pid: 100,
                session: "test".into(),
                window: "@1".into(),
                window_index: 1,
                pane_index: 0,
                cwd: "/same/cwd".into(),
                title: "Ignored title".into(),
                active: false,
                viewed: false,
                sidebar: false,
                name: String::new(),
                parent: String::new(),
                placed: false,
                task: false,
            };
            let process = Process {
                pid: 101,
                parent: 100,
                executable: "/usr/bin/node".into(),
                argv: "pi".into(),
            };
            Self {
                directory,
                panes: vec![pane],
                processes: ProcessTable {
                    processes: HashMap::from([(101, process)]),
                },
            }
        }
        fn write(&self, pid: i32, data: &str) {
            std::fs::write(self.directory.join(format!("{pid}.json")), data).unwrap();
        }
        fn scan(&self, now: u64) -> Result<Vec<Observation>> {
            scan_directory(
                &ScanContext {
                    panes: &self.panes,
                    processes: &self.processes,
                },
                &self.directory,
                || now,
            )
        }
        fn change(&self, field: &str, value: Value) {
            let mut status: Value = serde_json::from_str(WORKING).unwrap();
            status[field] = value;
            self.write(101, &status.to_string());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.directory).unwrap();
        }
    }

    #[test]
    fn published_states_and_durable_completion_count() {
        let f = Fixture::new();
        for (data, state, count) in [
            (WORKING, RawState::Working, 0),
            (BLOCKED, RawState::Blocked, 0),
            (IDLE, RawState::Idle, 2),
        ] {
            f.write(101, data);
            let observations = f.scan(10000).unwrap();
            assert_eq!(observations.len(), 1);
            let o = &observations[0];
            assert_eq!(o.state, state);
            assert_eq!(o.completed_turns, Some(count));
            assert_eq!(o.completion_epoch.as_deref(), Some("101-instance"));
            assert_eq!(o.session_id.as_deref(), Some("native-session"));
            assert_eq!(o.title, "Review parser");
            assert_eq!(
                o.detail.as_deref(),
                (state == RawState::Blocked).then_some("Allow command?")
            );
        }
    }

    #[test]
    fn opt_in_only_and_dead_files_are_ignored_before_parsing() {
        let f = Fixture::new();
        assert!(f.scan(10000).unwrap().is_empty());
        f.write(999, "malformed dead process file");
        assert!(f.scan(10000).unwrap().is_empty());
        std::fs::remove_file(f.directory.join("999.json")).unwrap();
        std::fs::remove_dir(&f.directory).unwrap();
        assert!(f.scan(10000).unwrap().is_empty());
        std::fs::create_dir(&f.directory).unwrap();
    }

    #[test]
    fn live_malformed_unknown_identity_and_freshness_fail_loudly() {
        let f = Fixture::new();
        f.write(101, "malformed live data");
        assert!(
            f.scan(10000)
                .err()
                .unwrap()
                .to_string()
                .contains("invalid live Pi status")
        );
        for (field, value) in [
            ("state", json!("mystery")),
            ("state", json!("done")),
            ("version", json!(2)),
            ("pid", json!(102)),
            ("pane_id", json!("%2")),
            ("session_id", json!("")),
            ("completion_epoch", json!("")),
            ("completed_turns", json!(-1)),
        ] {
            f.change(field, value);
            assert!(f.scan(10000).is_err(), "accepted invalid {field}");
        }
        f.change("unexpected", json!(true));
        assert!(f.scan(10000).is_err());
        let mut missing: Value = serde_json::from_str(WORKING).unwrap();
        missing.as_object_mut().unwrap().remove("completed_turns");
        f.write(101, &missing.to_string());
        assert!(f.scan(10000).is_err());
        f.write(101, WORKING);
        assert!(f.scan(15000).is_ok());
        assert!(f.scan(15001).is_err());
        assert!(f.scan(9999).is_err());
    }

    #[test]
    fn pid_ancestry_disambiguates_servers_and_same_cwd() {
        let mut f = Fixture::new();
        f.write(101, WORKING);
        f.panes[0].pid = 200;
        assert!(f.scan(10000).unwrap().is_empty());
        f.panes[0].pid = 100;
        let mut second = f.panes[0].clone();
        second.id = "%2".into();
        second.pid = 200;
        f.panes.push(second);
        f.processes.processes.insert(
            201,
            Process {
                pid: 201,
                parent: 200,
                executable: "node".into(),
                argv: "pi".into(),
            },
        );
        let mut other: Value = serde_json::from_str(IDLE).unwrap();
        other["pid"] = json!(201);
        other["pane_id"] = json!("%2");
        other["session_id"] = json!("second-session");
        f.write(201, &other.to_string());
        let observations = f.scan(10000).unwrap();
        assert_eq!(observations.len(), 2);
        assert!(
            observations
                .iter()
                .any(|o| o.pane_id == "%1" && o.state == RawState::Working)
        );
        assert!(
            observations
                .iter()
                .any(|o| o.pane_id == "%2" && o.state == RawState::Idle)
        );
    }

    #[test]
    fn exiting_process_is_gone_but_a_different_command_is_an_error() {
        let mut f = Fixture::new();
        f.write(101, WORKING);
        f.processes.processes.get_mut(&101).unwrap().argv = String::new();
        assert!(f.scan(10000).unwrap().is_empty());
        f.processes.processes.get_mut(&101).unwrap().argv = "vim".into();
        assert!(f.scan(10000).is_err());
    }

    #[test]
    fn multiple_live_sessions_in_one_pane_are_ambiguous() {
        let mut f = Fixture::new();
        f.write(101, WORKING);
        f.processes.processes.insert(
            102,
            Process {
                pid: 102,
                parent: 100,
                executable: "node".into(),
                argv: "pi".into(),
            },
        );
        let mut other: Value = serde_json::from_str(IDLE).unwrap();
        other["pid"] = json!(102);
        f.write(102, &other.to_string());
        assert!(
            f.scan(10000)
                .err()
                .unwrap()
                .to_string()
                .contains("multiple live Pi")
        );
    }

    #[test]
    fn interactive_launch_protects_option_like_tasks() {
        assert_eq!(launch(None, &[]).unwrap().command, "pi");
        assert!(launch(None, &[]).unwrap().args.is_empty());
        assert_eq!(launch(Some("-task"), &[]).unwrap().args, ["--", "-task"]);
    }

    #[test]
    fn launch_puts_agent_args_before_task() {
        let args = ["-m".to_string(), "gpt".to_string()];
        let launch_with = launch(Some("task"), &args).unwrap();
        assert_eq!(launch_with.command, "pi");
        assert_eq!(launch_with.args, ["-m", "gpt", "--", "task"]);
        assert_eq!(launch(None, &args).unwrap().args, ["-m", "gpt"]);
        assert!(launch(None, &[]).unwrap().args.is_empty());
    }
}
