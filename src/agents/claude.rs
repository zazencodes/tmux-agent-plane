//! Claude Code's published interactive session files.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

pub fn launch(task: Option<&str>) -> Launch {
    Launch {
        command: "claude",
        args: task.into_iter().map(str::to_owned).collect(),
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
    let dir = claude_dir()?.join("sessions");
    let mut sessions: HashMap<String, ClaudeSession> = HashMap::new();
    let entries = match std::fs::read_dir(&dir) {
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
