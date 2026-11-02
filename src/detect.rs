//! Find the agents running in tmux panes and read their live state, without hooks.
//!
//! - Claude Code keeps `~/.claude/sessions/<pid>.json` for each running session, with its tmux
//!   pane and a `status` of `busy`, `waiting` (plus `waitingFor`), `idle` or `shell`.
//! - Codex sets the pane title: a braille spinner in front while a turn runs, and
//!   `[ ! ] Action Required` while an approval or question waits.

use crate::index::{Kind, State};
use crate::tmux::Pane;
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

pub struct Detected {
    pub pane: Pane,
    pub kind: Kind,
    /// `Working`, `Blocked` or `Idle`; the daemon decides `Done`.
    pub state: State,
    pub detail: Option<String>,
    pub title: String,
    pub session_id: Option<String>,
}

pub fn detect(panes: &[Pane]) -> Result<Vec<Detected>> {
    let claude = claude_sessions()?;
    let procs = ProcessTable::load()?;
    let mut found = Vec::new();
    for pane in panes {
        if let Some(session) = claude.get(&pane.id) {
            found.push(Detected {
                pane: pane.clone(),
                kind: Kind::Claude,
                state: session.state,
                detail: session.detail.clone(),
                title: clean_title(&pane.title),
                session_id: Some(session.session_id.clone()),
            });
        } else if procs.has_descendant_named(pane.pid, "codex") {
            let (state, detail) = codex_state(&pane.title);
            found.push(Detected {
                pane: pane.clone(),
                kind: Kind::Codex,
                state,
                detail,
                title: clean_title(&pane.title),
                session_id: None,
            });
        }
    }
    Ok(found)
}

/// Drop the status glyphs agents put in front of the title (spinners, `✳`, `[ ! ]`).
fn clean_title(title: &str) -> String {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string()
}

fn codex_state(title: &str) -> (State, Option<String>) {
    if title.contains("Action Required") {
        return (State::Blocked, Some("action required".to_string()));
    }
    let spinning = title
        .chars()
        .next()
        .is_some_and(|c| ('\u{2800}'..='\u{28FF}').contains(&c));
    if spinning {
        (State::Working, None)
    } else {
        (State::Idle, None)
    }
}

struct ClaudeSession {
    started_at: u64,
    session_id: String,
    state: State,
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
fn claude_sessions() -> Result<HashMap<String, ClaudeSession>> {
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
        let (state, detail) = match file.status.as_deref() {
            Some("busy") => (State::Working, None),
            Some("waiting") => (State::Blocked, file.waiting_for.clone()),
            // "shell": the turn is over but a background shell (a dev server, say) runs on.
            Some("idle") | Some("shell") | None => (State::Idle, None),
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

struct ProcessTable {
    children: HashMap<i32, Vec<i32>>,
    names: HashMap<i32, String>,
}

impl ProcessTable {
    fn load() -> Result<Self> {
        let out = Command::new("ps")
            .args(["-A", "-o", "pid=,ppid=,comm="])
            .output()
            .context("failed to execute ps")?;
        if !out.status.success() {
            bail!("ps failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let mut children: HashMap<i32, Vec<i32>> = HashMap::new();
        let mut names = HashMap::new();
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let mut parts = line.split_whitespace();
            let (Some(pid), Some(ppid)) = (parts.next(), parts.next()) else {
                continue;
            };
            let (Ok(pid), Ok(ppid)) = (pid.parse::<i32>(), ppid.parse::<i32>()) else {
                continue;
            };
            let comm = parts.collect::<Vec<_>>().join(" ");
            let name = comm.rsplit('/').next().unwrap_or(&comm).to_string();
            children.entry(ppid).or_default().push(pid);
            names.insert(pid, name);
        }
        Ok(Self { children, names })
    }

    fn has_descendant_named(&self, root: i32, name: &str) -> bool {
        let mut stack = vec![root];
        while let Some(pid) = stack.pop() {
            if self.names.get(&pid).is_some_and(|n| n == name) {
                return true;
            }
            if let Some(kids) = self.children.get(&pid) {
                stack.extend(kids);
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_titles() {
        assert_eq!(codex_state("⠼ Fix tests | repo").0, State::Working);
        assert_eq!(codex_state("[ ! ] Action Required").0, State::Blocked);
        assert_eq!(codex_state("[ . ] Action Required").0, State::Blocked);
        assert_eq!(codex_state("Fix tests | repo").0, State::Idle);
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
