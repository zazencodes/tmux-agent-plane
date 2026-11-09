//! Claude Code's published interactive session files.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub fn launch(task: Option<&str>, agent_args: &[String]) -> Result<Launch> {
    let mut args = agent_args.to_vec();
    args.extend(task.map(str::to_owned));
    Ok(Launch {
        command: "claude",
        args,
    })
}

/// Claude Code resumes a conversation in place, keeping its session id.
pub fn resume(session_id: &str) -> Launch {
    Launch {
        command: "claude",
        args: vec!["--resume".into(), session_id.into()],
    }
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let sessions = claude_sessions(context)?;
    Ok(context
        .panes
        .iter()
        .filter_map(|pane| {
            let session = sessions.get(&pane.id)?;
            Some(Observation {
                pane_id: pane.id.clone(),
                state: session.state,
                detail: session.detail.clone(),
                title: clean_title(&pane.title),
                session_id: Some(session.session_id.clone()),
                preserve_title: false,
                completed_turns: None,
                completion_epoch: None,
            })
        })
        .collect())
}

fn clean_title(title: &str) -> String {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string()
}

struct ClaudeSession {
    started_at: u64,
    session_id: String,
    state: RawState,
    detail: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionFile {
    pid: i32,
    kind: String,
    session_id: String,
    started_at: u64,
    tmux: Option<String>,
    status: Option<String>,
    waiting_for: Option<String>,
}

fn claude_dir() -> Result<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => {
            Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".claude"))
        }
    }
}

/// Live interactive Claude Code sessions, keyed by tmux pane id.
fn claude_sessions(context: &ScanContext<'_>) -> Result<HashMap<String, ClaudeSession>> {
    claude_sessions_in(&claude_dir()?.join("sessions"), context)
}

fn claude_sessions_in(
    dir: &Path,
    context: &ScanContext<'_>,
) -> Result<HashMap<String, ClaudeSession>> {
    let mut sessions: HashMap<String, ClaudeSession> = HashMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(sessions),
        Err(e) => return Err(e).with_context(|| format!("reading {}", dir.display())),
    };
    for entry in entries {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        // Claude rewrites and deletes these files as sessions change. A file that vanished or
        // is mid-write is read again on the next tick.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(file) = serde_json::from_slice::<ClaudeSessionFile>(&bytes) else {
            continue;
        };
        if file.kind != "interactive" || !process_alive(file.pid) {
            continue;
        }
        // "pro:@104.%154" -> "%154"
        let Some(pane) = file.tmux.as_deref().and_then(|t| t.rsplit('.').next()) else {
            continue;
        };
        if !context
            .panes
            .iter()
            .any(|p| p.id == pane && context.processes.is_descendant(file.pid, p.pid))
        {
            continue;
        }
        let (state, detail) = match file.status.as_deref() {
            Some("busy") => (RawState::Working, None),
            Some("waiting") => (RawState::Blocked, file.waiting_for.clone()),
            // "shell": the turn is over but a background shell (a dev server, say) runs on.
            Some("idle") | Some("shell") | None => (RawState::Idle, None),
            Some(other) => bail!(
                "unknown Claude Code session status {other:?} in {}",
                path.display()
            ),
        };
        let session = ClaudeSession {
            started_at: file.started_at,
            session_id: file.session_id,
            state,
            detail,
        };
        // Two live sessions in one pane: the newer one is in front.
        if sessions
            .get(pane)
            .is_none_or(|existing| existing.started_at < session.started_at)
        {
            sessions.insert(pane.to_string(), session);
        }
    }
    Ok(sessions)
}

fn process_alive(pid: i32) -> bool {
    // Signal 0 checks existence; EPERM means it exists under another user.
    let exists = unsafe { libc::kill(pid, 0) } == 0;
    exists || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Process, ProcessTable};
    use crate::tmux::Pane;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn published_files_preserve_newest_session_and_transient_rewrites() {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "plane-claude-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let pid = std::process::id() as i32;
        let panes = [Pane {
            id: "%1".into(),
            pid,
            session: "s".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 1,
            cwd: "/tmp".into(),
            title: "Task".into(),
            active: false,
            viewed: false,
            sidebar: false,
            name: String::new(),
            parent: String::new(),
            placed: false,
            task: false,
        }];
        let processes = ProcessTable {
            processes: HashMap::from([(
                pid,
                Process {
                    pid,
                    parent: 0,
                    executable: "claude".into(),
                    argv: "claude".into(),
                },
            )]),
        };
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(claude_sessions_in(&dir, &context).unwrap().is_empty());
        std::fs::create_dir(&dir).unwrap();
        let write = |name: &str, status: &str, started: u64| {
            std::fs::write(
                dir.join(name),
                json!({"pid":pid,"kind":"interactive","sessionId":name,
                "startedAt":started,"tmux":"s:@1.%1","status":status,"waitingFor":"input needed"})
                .to_string(),
            )
            .unwrap();
        };
        write("older.json", "busy", 1);
        write("newer.json", "waiting", 2);
        std::fs::write(dir.join("rewriting.json"), "{").unwrap();
        let found = claude_sessions_in(&dir, &context).unwrap();
        assert_eq!(found["%1"].session_id, "newer.json");
        assert_eq!(found["%1"].state, RawState::Blocked);
        assert_eq!(found["%1"].detail.as_deref(), Some("input needed"));
        write("newer.json", "shell", 2);
        assert_eq!(
            claude_sessions_in(&dir, &context).unwrap()["%1"].state,
            RawState::Idle
        );
        write("newer.json", "unknown", 2);
        assert!(claude_sessions_in(&dir, &context).is_err());
        // A pane on a different server cannot claim this PID, even with the same ID.
        let mut unrelated = panes[0].clone();
        unrelated.pid = pid + 1;
        let other_panes = [unrelated];
        let other = ScanContext {
            panes: &other_panes,
            processes: &processes,
        };
        assert!(claude_sessions_in(&dir, &other).unwrap().is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn launch_puts_agent_args_before_task() {
        let args = ["-m".to_string(), "gpt".to_string()];
        let launch_with = launch(Some("task"), &args).unwrap();
        assert_eq!(launch_with.command, "claude");
        assert_eq!(launch_with.args, ["-m", "gpt", "task"]);
        assert_eq!(launch(None, &args).unwrap().args, ["-m", "gpt"]);
        assert!(launch(None, &[]).unwrap().args.is_empty());
    }
}
