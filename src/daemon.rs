//! The watcher: once a second, detect agents, track done/seen, write the index,
//! update the status bar and notify on transitions.

use crate::detect::{self, Detected};
use crate::index::{self, Agent, Index, Kind, State};
use crate::tmux;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::process::Command;
use std::time::Duration;

const TICK: Duration = Duration::from_secs(1);

/// tmux option holding the status bar segment. Put `#{@agent_plane_status}` in `status-right`.
const STATUS_OPTION: &str = "@agent_plane_status";

pub fn run() -> Result<()> {
    let dir = index::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    let lock = File::create(dir.join("daemon.lock"))?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        // Another daemon owns the index; sourcing tmux.conf again lands here.
        return Ok(());
    }

    let mut prev: HashMap<String, Agent> = index::read_any()?
        .map(|i| i.agents.into_iter().map(|a| (a.pane.clone(), a)).collect())
        .unwrap_or_default();
    let mut status = None;

    loop {
        let panes = match tmux::list_panes() {
            Ok(panes) => panes,
            Err(_) if !tmux::server_running() => return Ok(()),
            Err(e) => return Err(e),
        };
        let now = index::now_ms();
        let mut detected = detect::detect(&panes)?;
        detected.sort_by(|a, b| {
            (&a.pane.session, a.pane.window_index, a.pane.pane_index).cmp(&(
                &b.pane.session,
                b.pane.window_index,
                b.pane.pane_index,
            ))
        });

        let mut agents = Vec::with_capacity(detected.len());
        for d in detected {
            let before = prev.get(&d.pane.id).filter(|a| a.kind == d.kind);
            let agent = advance(before, d, now);
            if let Some(message) = transition_message(before, &agent) {
                notify(&agent.name, &message)?;
            }
            agents.push(agent);
        }

        let segment = status_segment(&agents);
        if status.as_ref() != Some(&segment) {
            tmux::run(&["set-option", "-g", STATUS_OPTION, &segment])?;
            for client in tmux::clients()? {
                tmux::run(&["refresh-client", "-S", "-t", &client])?;
            }
            status = Some(segment);
        }

        index::write(&Index {
            updated_at: now,
            agents: agents.clone(),
        })?;
        prev = agents.into_iter().map(|a| (a.pane.clone(), a)).collect();
        std::thread::sleep(TICK);
    }
}

/// Fold one tick of detection into an agent's record.
///
/// A turn that ends while nobody is looking makes the agent `Done`; it becomes `Idle` once its
/// pane is on screen.
fn advance(before: Option<&Agent>, d: Detected, now: u64) -> Agent {
    let was_working = before.is_some_and(|a| a.state == State::Working);
    let finished = was_working && d.state == State::Idle;
    let completions = before.map_or(0, |a| a.completions) + u64::from(finished);
    let unseen = before.is_some_and(|a| a.state == State::Done) || finished;
    let state = match d.state {
        State::Idle if unseen && !d.pane.visible => State::Done,
        other => other,
    };
    // Codex replaces its task title with "Action Required" while it waits; keep the task.
    let title = match before {
        Some(a) if d.kind == Kind::Codex && d.state == State::Blocked => a.title.clone(),
        _ => d.title,
    };
    let name = if d.pane.name.is_empty() {
        format!("{}-{}", d.kind.as_str(), d.pane.id.trim_start_matches('%'))
    } else {
        d.pane.name.clone()
    };
    Agent {
        name,
        location: d.pane.location(),
        pane: d.pane.id,
        kind: d.kind,
        state,
        detail: d.detail,
        title,
        session: d.pane.session,
        window: d.pane.window,
        cwd: d.pane.cwd,
        parent: Some(d.pane.parent).filter(|p| !p.is_empty()),
        session_id: d.session_id,
        state_since: match before {
            Some(a) if a.state == state => a.state_since,
            _ => now,
        },
        completions,
        visible: d.pane.visible,
    }
}

fn transition_message(before: Option<&Agent>, agent: &Agent) -> Option<String> {
    let before = before?;
    if before.state == agent.state || agent.visible {
        return None;
    }
    match agent.state {
        State::Done => Some(format!("finished: {}", agent.title)),
        State::Blocked => Some(format!(
            "needs you: {}",
            agent.detail.as_deref().unwrap_or(&agent.title)
        )),
        State::Working | State::Idle => None,
    }
}

fn notify(name: &str, message: &str) -> Result<()> {
    for client in tmux::clients()? {
        tmux::run(&[
            "display-message",
            "-c",
            &client,
            "-d",
            "5000",
            &format!("{name} {message}"),
        ])?;
    }
    system_notification(name, message)
}

#[cfg(target_os = "macos")]
fn system_notification(name: &str, message: &str) -> Result<()> {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        quote(message),
        quote(name)
    );
    let status = Command::new("osascript")
        .args(["-e", &script])
        .status()
        .context("failed to execute osascript")?;
    if !status.success() {
        bail!("osascript notification failed");
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn system_notification(name: &str, message: &str) -> Result<()> {
    let status = Command::new("notify-send")
        .args([name, message])
        .status()
        .context("failed to execute notify-send")?;
    if !status.success() {
        bail!("notify-send failed");
    }
    Ok(())
}

pub fn status_segment(agents: &[Agent]) -> String {
    let count = |s: State| agents.iter().filter(|a| a.state == s).count();
    [
        (State::Blocked, "#f38ba8", "◆"),
        (State::Done, "#a6e3a1", "✓"),
        (State::Working, "#89b4fa", "●"),
        (State::Idle, "#6c7086", "○"),
    ]
    .iter()
    .filter_map(|&(state, colour, icon)| match count(state) {
        0 => None,
        n => Some(format!("#[fg={colour}]{icon} {n}")),
    })
    .collect::<Vec<_>>()
    .join(" ")
        + if agents.is_empty() { "" } else { "#[default] " }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::Pane;

    fn tick(before: Option<&Agent>, state: State, visible: bool) -> Agent {
        let pane = Pane {
            id: "%1".into(),
            pid: 1,
            session: "s".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 1,
            cwd: "/tmp".into(),
            title: "Task".into(),
            visible,
            name: String::new(),
            parent: String::new(),
        };
        let d = Detected {
            pane,
            kind: Kind::Claude,
            state,
            detail: None,
            title: "Task".into(),
            session_id: None,
        };
        advance(before, d, 0)
    }

    #[test]
    fn unseen_finish_is_done_until_viewed() {
        let a = tick(None, State::Working, false);
        let a = tick(Some(&a), State::Idle, false);
        assert_eq!((a.state, a.completions), (State::Done, 1));
        let a = tick(Some(&a), State::Idle, false);
        assert_eq!(a.state, State::Done);
        let a = tick(Some(&a), State::Idle, true);
        assert_eq!(a.state, State::Idle);
        let a = tick(Some(&a), State::Idle, false);
        assert_eq!((a.state, a.completions), (State::Idle, 1));
    }

    #[test]
    fn finish_while_watched_is_idle() {
        let a = tick(None, State::Working, true);
        let a = tick(Some(&a), State::Idle, true);
        assert_eq!((a.state, a.completions), (State::Idle, 1));
    }

    #[test]
    fn startup_idle_is_not_a_completion() {
        let a = tick(None, State::Idle, false);
        assert_eq!((a.state, a.completions), (State::Idle, 0));
    }

    #[test]
    fn notifies_on_done_and_blocked_only_when_unseen() {
        let working = tick(None, State::Working, false);
        let done = tick(Some(&working), State::Idle, false);
        assert!(transition_message(Some(&working), &done).is_some());
        let blocked = tick(Some(&working), State::Blocked, false);
        assert!(transition_message(Some(&working), &blocked).is_some());
        let watched = tick(Some(&working), State::Blocked, true);
        assert!(transition_message(Some(&working), &watched).is_none());
    }
}
