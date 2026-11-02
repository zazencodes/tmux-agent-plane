//! The watcher: once a second, detect agents, track done/seen, write the index,
//! update the status bar and notify on transitions.

use crate::agents::RawState;
use crate::detect::{self, Detected};
use crate::diagnostics::{self, Active, Batch, Issue};
use crate::index::{self, Agent, Index, State};
use crate::tmux;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const TICK: Duration = Duration::from_secs(1);

/// tmux option holding the status bar segment. Put `#{@agent_plane_status}` in `status-right`.
const STATUS_OPTION: &str = "@agent_plane_status";

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// Called by the lifecycle worker's signal/control handlers.
pub fn request_shutdown() {
    SHUTDOWN.store(true, Ordering::Relaxed);
}

/// Persist and surface a fatal daemon/lifecycle error without desktop notification recursion.
pub fn report_fatal(error: &anyhow::Error) -> Result<()> {
    let path = diagnostics::log_path()?;
    let reason = format!("{error:#}");
    let persistence = diagnostics::record("fatal", &reason);
    let _ = tmux::run(&[
        "set-option",
        "-g",
        STATUS_OPTION,
        "#[fg=#f38ba8]⚠ daemon stopped; see daemon.log#[default] ",
    ]);
    let _ = tmux_message(&format!(
        "tmux-agent-plane stopped: {reason}; log: {}",
        path.display()
    ));
    persistence
}

pub fn run() -> Result<()> {
    let path = diagnostics::log_path()?;
    if let Err(error) = run_inner() {
        return match report_fatal(&error) {
            Ok(()) => Err(error.context(format!("daemon stopped; see {}", path.display()))),
            Err(log_error) => Err(error.context(format!(
                "daemon stopped; could not persist reason to {}: {log_error:#}",
                path.display()
            ))),
        };
    }
    Ok(())
}

fn run_inner() -> Result<()> {
    let dir = index::state_dir()?;
    std::fs::create_dir_all(&dir).context("creating daemon state directory")?;
    let lock = File::create(dir.join("daemon.lock")).context("opening daemon lock")?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            return Ok(());
        }
        return Err(error).context("locking daemon index");
    }
    diagnostics::record("daemon", "active")?;
    let mut prev: HashMap<String, Agent> = index::read_any()?
        .map(|i| i.agents.into_iter().map(|a| (a.pane.clone(), a)).collect())
        .unwrap_or_default();
    let mut presentation = Presentation::default();
    let mut active = Active::default();
    let mut notifications = Notifications::default();
    // Diagnostic announcements use only tmux and are never sent through notification retry.
    let mut announcement: Option<String> = None;

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return Ok(());
        }
        let panes = match tmux::list_panes() {
            Ok(panes) => panes,
            Err(_) if !tmux::server_running() => return Ok(()),
            Err(e) => return Err(e),
        };
        let now = index::now_ms();
        let before = prev.clone();
        let batch = fold(&mut prev, detect::detect(&panes)?, &panes, now);
        let agents = batch.values;
        let mut issues = batch.issues;
        for agent in &agents {
            if let Some(message) = transition_message(
                before.get(&agent.pane).filter(|a| a.kind == agent.kind),
                agent,
            ) {
                notifications.push(&agent.name, &message);
            }
        }
        issues.extend(notifications.deliver());
        if let Some(message) = &announcement {
            match tmux_message(message) {
                Ok(()) => announcement = None,
                Err(error) => {
                    // Retain the announcement for retry. Its failure never announces itself.
                    issues.push(Issue::new("diagnostics:tmux", error));
                }
            }
        }
        // Index persistence remains fatal and independent of status presentation.
        index::write(&Index {
            updated_at: now,
            agents: agents.clone(),
        })
        .context("writing agent index")?;
        // Use the last diagnostic snapshot for presentation, then fold this attempt's issues
        // exactly once. A new fault/recovery reaches the status on the next tick.
        let segment = active.status() + &status_segment(&agents);
        issues.extend(presentation.update(&segment));
        let changes = active.update(&issues)?;
        let announcements: Vec<_> = changes
            .into_iter()
            .filter(|message| !message.starts_with("diagnostics:tmux:"))
            .collect();
        if !announcements.is_empty() {
            announcement = Some(format!(
                "tmux-agent-plane: {}; log: {}",
                announcements.join("; "),
                diagnostics::log_path()?.display()
            ));
        }
        std::thread::sleep(TICK);
    }
}

/// Status writes and client refreshes are presentation; none can invalidate tracking.
#[derive(Default)]
struct Presentation {
    completed: Option<String>,
}

impl Presentation {
    fn update(&mut self, segment: &str) -> Vec<Issue> {
        self.update_with(
            segment,
            |segment| tmux::run(&["set-option", "-g", STATUS_OPTION, segment]).map(|_| ()),
            tmux::clients,
            |client| tmux::run(&["refresh-client", "-S", "-t", client]).map(|_| ()),
        )
    }

    fn update_with(
        &mut self,
        segment: &str,
        mut set_status: impl FnMut(&str) -> Result<()>,
        mut clients: impl FnMut() -> Result<Vec<String>>,
        mut refresh: impl FnMut(&str) -> Result<()>,
    ) -> Vec<Issue> {
        if self.completed.as_deref() == Some(segment) {
            return Vec::new();
        }
        let mut issues = Vec::new();
        if let Err(error) = set_status(segment) {
            issues.push(Issue::new("presentation:status", error));
        }
        match clients() {
            Ok(clients) => {
                for client in clients {
                    if let Err(error) = refresh(&client) {
                        issues.push(Issue::new(format!("presentation:refresh:{client}"), error));
                    }
                }
            }
            Err(error) => issues.push(Issue::new("presentation:clients", error)),
        }
        // An incomplete attempt is retried even when the requested segment stays unchanged.
        self.completed = issues.is_empty().then(|| segment.to_owned());
        issues
    }
}

/// Retain last good counters/unseen state only internally while their live pane is faulted.
/// The public batch contains exclusively agents observed successfully on this tick.
fn fold(
    previous: &mut HashMap<String, Agent>,
    mut batch: Batch<Detected>,
    panes: &[tmux::Pane],
    now: u64,
) -> Batch<Agent> {
    batch.values.sort_by(|a, b| {
        (&a.pane.session, a.pane.window_index, a.pane.pane_index).cmp(&(
            &b.pane.session,
            b.pane.window_index,
            b.pane.pane_index,
        ))
    });
    let mut result = Batch {
        values: Vec::new(),
        issues: batch.issues,
    };
    let mut next = HashMap::new();
    for d in batch.values {
        let pane = d.pane.id.clone();
        let kind = d.kind;
        let before = previous.get(&pane).filter(|a| a.kind == kind);
        match advance(before, d, now) {
            Ok(agent) => {
                next.insert(pane, agent.clone());
                result.values.push(agent);
            }
            Err(error) => {
                result.issues.push(Issue::new(
                    format!("advance:{}:{pane}", kind.as_str()),
                    error,
                ));
                if let Some(before) = before {
                    next.insert(pane, before.clone());
                }
            }
        }
    }
    for (pane, agent) in previous.drain() {
        if panes.iter().any(|p| p.id == pane)
            && result
                .issues
                .iter()
                .any(|e| e.component == format!("scan:{}", agent.kind.as_str()))
        {
            next.entry(pane).or_insert(agent);
        }
    }
    *previous = next;
    result
}

/// Fold one tick of detection into an agent's record.
///
/// A turn that ends while nobody is looking makes the agent `Done`; it becomes `Idle` once its
/// pane is on screen.
fn advance(before: Option<&Agent>, d: Detected, now: u64) -> Result<Agent> {
    let was_working = before.is_some_and(|a| a.state == State::Working);
    let completed = match (d.completed_turns, d.completion_epoch.as_deref()) {
        (Some(current), Some(epoch)) if !epoch.is_empty() => match before
            .filter(|a| a.completion_epoch == d.completion_epoch)
            .and_then(|a| a.reported_completions)
        {
            Some(previous) => current
                .checked_sub(previous)
                .context("agent completion counter regressed within a publisher lifetime")?,
            None => current,
        },
        (None, None) => u64::from(was_working && d.state == RawState::Idle),
        _ => bail!("agent completion counter requires a nonempty publisher epoch"),
    };
    let completions = before
        .map_or(0, |a| a.completions)
        .checked_add(completed)
        .context("agent completion count overflowed")?;
    let unseen = before.is_some_and(|a| a.state == State::Done) || completed > 0;
    let state = match State::from(d.state) {
        State::Idle if unseen && !d.pane.visible => State::Done,
        other => other,
    };
    // Some agents temporarily replace their task title while waiting.
    let title = match before {
        Some(a) if d.preserve_title => a.title.clone(),
        _ => d.title,
    };
    let name = if d.pane.name.is_empty() {
        format!("{}-{}", d.kind.as_str(), d.pane.id.trim_start_matches('%'))
    } else {
        d.pane.name.clone()
    };
    Ok(Agent {
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
        reported_completions: d.completed_turns,
        completion_epoch: d.completion_epoch,
        visible: d.pane.visible,
    })
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

#[derive(Clone)]
enum Destination {
    Desktop,
    Clients,
    Client(String),
}
#[derive(Clone)]
struct Notification {
    destination: Destination,
    name: String,
    message: String,
}
#[derive(Default)]
struct Notifications {
    pending: Vec<Notification>,
}
impl Notifications {
    fn push(&mut self, name: &str, message: &str) {
        for destination in [Destination::Clients, Destination::Desktop] {
            self.pending.push(Notification {
                destination,
                name: name.into(),
                message: message.into(),
            });
        }
    }
    fn deliver(&mut self) -> Vec<Issue> {
        self.deliver_with(tmux::clients, |notification| {
            match &notification.destination {
                Destination::Desktop => {
                    system_notification(&notification.name, &notification.message)
                }
                Destination::Client(client) => client_message(
                    client,
                    &format!("{} {}", notification.name, notification.message),
                ),
                Destination::Clients => unreachable!("expanded before delivery"),
            }
        })
    }
    fn deliver_with(
        &mut self,
        mut clients: impl FnMut() -> Result<Vec<String>>,
        mut send: impl FnMut(&Notification) -> Result<()>,
    ) -> Vec<Issue> {
        let mut issues = Vec::new();
        let mut deliveries = Vec::new();
        let mut retries = Vec::new();
        for notification in self.pending.drain(..) {
            if matches!(notification.destination, Destination::Clients) {
                match clients() {
                    Ok(clients) => {
                        for client in clients {
                            let mut n = notification.clone();
                            n.destination = Destination::Client(client);
                            deliveries.push(n);
                        }
                    }
                    Err(error) => {
                        issues.push(Issue::new("notification:clients", error));
                        retries.push(notification);
                    }
                }
            } else {
                deliveries.push(notification);
            }
        }
        for notification in deliveries {
            if let Err(error) = send(&notification) {
                let component = match &notification.destination {
                    Destination::Desktop => "notification:desktop".into(),
                    Destination::Client(client) => format!("notification:tmux:{client}"),
                    Destination::Clients => unreachable!(),
                };
                issues.push(Issue::new(component, error));
                retries.push(notification);
            }
        }
        self.pending = retries;
        issues
    }
}

fn client_message(client: &str, message: &str) -> Result<()> {
    tmux::run(&["display-message", "-c", client, "-d", "5000", message])?;
    Ok(())
}

fn tmux_message(message: &str) -> Result<()> {
    for client in tmux::clients()? {
        client_message(&client, message)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn system_notification(name: &str, message: &str) -> Result<()> {
    let quote = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let script = format!(
        "display notification \"{}\" with title \"{}\"",
        quote(message),
        quote(name)
    );
    let output = Command::new("osascript")
        .args(["-e", &script])
        .output()
        .context("failed to execute osascript")?;
    if !output.status.success() {
        bail!(
            "osascript notification failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn system_notification(name: &str, message: &str) -> Result<()> {
    let output = Command::new("notify-send")
        .args([name, message])
        .output()
        .context("failed to execute notify-send")?;
    if !output.status.success() {
        bail!(
            "notify-send failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

pub fn status_segment(agents: &[Agent]) -> String {
    let count = |s: State| agents.iter().filter(|a| a.state == s).count();
    [
        (State::Blocked, "#f38ba8", "◆"),
        (State::Done, "#a6e3a1", "✓"),
        (State::Working, "#cba6f7", "●"),
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
    use crate::index::Kind;
    use crate::tmux::Pane;

    fn observation(state: RawState, visible: bool) -> Detected {
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
        Detected {
            pane,
            kind: Kind::Claude,
            state,
            detail: None,
            title: "Task".into(),
            session_id: None,
            preserve_title: false,
            completed_turns: None,
            completion_epoch: None,
        }
    }

    fn tick(before: Option<&Agent>, state: RawState, visible: bool) -> Agent {
        advance(before, observation(state, visible), 0).unwrap()
    }

    #[test]
    fn refresh_failure_is_scoped_retried_and_recovers_without_replaying_completed_presentation() {
        let mut presentation = Presentation::default();
        let mut attempts = Vec::new();
        let issues = presentation.update_with(
            "segment",
            |_| Ok(()),
            || Ok(vec!["bad".into(), "good".into()]),
            |client| {
                attempts.push(client.to_owned());
                if client == "bad" {
                    anyhow::bail!("client disappeared");
                }
                Ok(())
            },
        );
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].component, "presentation:refresh:bad");
        assert_eq!(attempts, ["bad", "good"]);
        assert!(presentation.completed.is_none());
        let issues = presentation.update_with(
            "segment",
            |_| Ok(()),
            || Ok(vec!["good".into()]),
            |_| Ok(()),
        );
        assert!(issues.is_empty());
        assert_eq!(presentation.completed.as_deref(), Some("segment"));
        assert!(
            presentation
                .update_with(
                    "segment",
                    |_| panic!("already completed"),
                    || panic!("already completed"),
                    |_| panic!("already completed")
                )
                .is_empty()
        );
    }

    #[test]
    fn status_and_roster_failures_remain_visible_until_successful_presentation() {
        let mut presentation = Presentation::default();
        let issues = presentation.update_with(
            "segment",
            |_| anyhow::bail!("status write failed"),
            || anyhow::bail!("client roster failed"),
            |_| unreachable!(),
        );
        assert_eq!(
            issues
                .iter()
                .map(|e| e.component.as_str())
                .collect::<Vec<_>>(),
            ["presentation:status", "presentation:clients"]
        );
        assert!(presentation.completed.is_none());
        // A fresh roster/refresh is still attempted when the status write fails.
        let mut refreshed = false;
        let issues = presentation.update_with(
            "segment",
            |_| anyhow::bail!("status write failed"),
            || Ok(vec!["client".into()]),
            |_| {
                refreshed = true;
                Ok(())
            },
        );
        assert!(refreshed);
        assert_eq!(issues.len(), 1);
        assert!(
            presentation
                .update_with("segment", |_| Ok(()), || Ok(Vec::new()), |_| unreachable!())
                .is_empty()
        );
    }

    fn batch(values: Vec<Detected>, issues: Vec<Issue>) -> Batch<Detected> {
        Batch { values, issues }
    }

    #[test]
    fn scan_fault_withdraws_public_agent_preserves_unseen_state_and_drops_dead_panes() {
        let mut d = observation(RawState::Idle, false);
        d.completed_turns = Some(2);
        d.completion_epoch = Some("epoch".into());
        let pane = d.pane.clone();
        let agent = advance(None, d, 1).unwrap();
        let mut prev = HashMap::from([(agent.pane.clone(), agent)]);
        let result = fold(
            &mut prev,
            batch(
                Vec::new(),
                vec![Issue::new("scan:claude", anyhow::anyhow!("bad state"))],
            ),
            std::slice::from_ref(&pane),
            2,
        );
        assert!(result.values.is_empty());
        assert_eq!(prev["%1"].state, State::Done);
        let mut d = observation(RawState::Idle, false);
        d.completed_turns = Some(2);
        d.completion_epoch = Some("epoch".into());
        let recovered = fold(
            &mut prev,
            batch(vec![d], Vec::new()),
            std::slice::from_ref(&pane),
            3,
        );
        assert_eq!(
            (recovered.values[0].state, recovered.values[0].completions),
            (State::Done, 2)
        );
        fold(
            &mut prev,
            batch(
                Vec::new(),
                vec![Issue::new("scan:claude", anyhow::anyhow!("bad state"))],
            ),
            &[],
            4,
        );
        assert!(prev.is_empty());
    }

    #[test]
    fn regressed_counter_stays_faulted_until_caught_up_while_other_observation_advances() {
        let mut d = observation(RawState::Idle, false);
        d.completed_turns = Some(5);
        d.completion_epoch = Some("epoch".into());
        let agent = advance(None, d, 1).unwrap();
        let mut prev = HashMap::from([(agent.pane.clone(), agent)]);
        for count in [3, 4, 5] {
            let mut faulty = observation(RawState::Idle, false);
            faulty.completed_turns = Some(count);
            faulty.completion_epoch = Some("epoch".into());
            let mut healthy = observation(RawState::Working, false);
            healthy.pane.id = "%2".into();
            let panes = [faulty.pane.clone(), healthy.pane.clone()];
            let result = fold(
                &mut prev,
                batch(vec![faulty, healthy], Vec::new()),
                &panes,
                count,
            );
            assert!(
                result
                    .values
                    .iter()
                    .any(|a| a.pane == "%2" && a.state == State::Working)
            );
            assert_eq!(result.issues.len(), usize::from(count < 5));
            assert_eq!(prev["%1"].completions, 5);
            assert_eq!(prev["%1"].reported_completions, Some(5));
            if count < 5 {
                assert!(result.values.iter().all(|a| a.pane != "%1"));
            } else {
                let recovered = result.values.iter().find(|a| a.pane == "%1").unwrap();
                assert_eq!(recovered.state, State::Done);
            }
        }
        // A successfully observed different kind in the same pane replaces the baseline.
        let mut d = observation(RawState::Idle, false);
        d.kind = Kind::Codex;
        let panes = [d.pane.clone()];
        fold(
            &mut prev,
            batch(
                vec![d],
                vec![Issue::new("scan:claude", anyhow::anyhow!("fault"))],
            ),
            &panes,
            9,
        );
        assert_eq!(prev["%1"].kind, Kind::Codex);
        assert_eq!(prev["%1"].completions, 0);
    }

    #[test]
    fn desktop_failure_retries_only_failed_delivery_and_other_notifications_continue() {
        let mut notifications = Notifications::default();
        notifications.push("first", "finished");
        notifications.push("second", "needs you");
        let mut tmux_deliveries = Vec::new();
        let failures = notifications.deliver_with(
            || Ok(vec!["client".into()]),
            |n| {
                match n.destination {
                    Destination::Desktop => anyhow::bail!("desktop unavailable"),
                    Destination::Client(_) => tmux_deliveries.push(n.name.clone()),
                    Destination::Clients => unreachable!(),
                }
                Ok(())
            },
        );
        assert_eq!(failures.len(), 2);
        assert!(
            failures
                .iter()
                .all(|e| e.component == "notification:desktop")
        );
        assert_eq!(tmux_deliveries, ["first", "second"]);
        assert_eq!(notifications.pending.len(), 2);
        let mut retried = Vec::new();
        let issues = notifications.deliver_with(
            || panic!("successful clients must not be retried"),
            |n| {
                assert!(matches!(n.destination, Destination::Desktop));
                retried.push(n.name.clone());
                Ok(())
            },
        );
        assert!(issues.is_empty());
        assert!(notifications.pending.is_empty());
        assert_eq!(retried, ["first", "second"]);
    }

    #[test]
    fn client_failure_does_not_prevent_desktop_or_other_clients() {
        let mut notifications = Notifications::default();
        notifications.push("agent", "finished");
        let mut delivered = 0;
        let issues = notifications.deliver_with(
            || Ok(vec!["bad".into(), "good".into()]),
            |n| {
                if matches!(&n.destination, Destination::Client(client) if client == "bad") {
                    anyhow::bail!("client vanished");
                }
                delivered += 1;
                Ok(())
            },
        );
        assert_eq!(delivered, 2);
        assert_eq!(issues.len(), 1);
        assert_eq!(notifications.pending.len(), 1);
    }

    #[test]
    fn unseen_finish_is_done_until_viewed() {
        let a = tick(None, RawState::Working, false);
        let a = tick(Some(&a), RawState::Idle, false);
        assert_eq!((a.state, a.completions), (State::Done, 1));
        let a = tick(Some(&a), RawState::Idle, false);
        assert_eq!(a.state, State::Done);
        let a = tick(Some(&a), RawState::Idle, true);
        assert_eq!(a.state, State::Idle);
        let a = tick(Some(&a), RawState::Idle, false);
        assert_eq!((a.state, a.completions), (State::Idle, 1));
    }

    #[test]
    fn finish_while_watched_is_idle() {
        let a = tick(None, RawState::Working, true);
        let a = tick(Some(&a), RawState::Idle, true);
        assert_eq!((a.state, a.completions), (State::Idle, 1));
    }

    #[test]
    fn startup_idle_is_not_a_completion() {
        let a = tick(None, RawState::Idle, false);
        assert_eq!((a.state, a.completions), (State::Idle, 0));
    }

    #[test]
    fn notifies_on_done_and_blocked_only_when_unseen() {
        let working = tick(None, RawState::Working, false);
        let done = tick(Some(&working), RawState::Idle, false);
        assert!(transition_message(Some(&working), &done).is_some());
        let blocked = tick(Some(&working), RawState::Blocked, false);
        assert!(transition_message(Some(&working), &blocked).is_some());
        let watched = tick(Some(&working), RawState::Blocked, true);
        assert!(transition_message(Some(&working), &watched).is_none());
    }
    #[test]
    fn temporary_titles_preserve_previous_task_for_any_kind() {
        let before = tick(None, RawState::Working, false);
        let observation = || Detected {
            pane: Pane {
                id: before.pane.clone(),
                pid: 1,
                session: "s".into(),
                window: "@1".into(),
                window_index: 1,
                pane_index: 1,
                cwd: "/tmp".into(),
                title: "Action Required".into(),
                visible: false,
                name: String::new(),
                parent: String::new(),
            },
            kind: Kind::Claude,
            state: RawState::Blocked,
            detail: None,
            title: "Action Required".into(),
            session_id: None,
            preserve_title: true,
            completed_turns: None,
            completion_epoch: None,
        };
        assert_eq!(
            advance(Some(&before), observation(), 1).unwrap().title,
            "Task"
        );
        assert_eq!(
            advance(None, observation(), 1).unwrap().title,
            "Action Required"
        );
    }
    #[test]
    fn durable_completion_counts_fast_first_turn_without_working_tick() {
        let mut d = observation(RawState::Idle, false);
        d.session_id = Some("session".into());
        d.completion_epoch = Some("launch".into());
        d.completed_turns = Some(1);
        let a = advance(None, d, 0).unwrap();
        assert_eq!((a.state, a.completions), (State::Done, 1));
        let mut d = observation(RawState::Idle, false);
        d.session_id = a.session_id.clone();
        d.completion_epoch = a.completion_epoch.clone();
        d.completed_turns = Some(1);
        d.completion_epoch = Some("launch".into());
        let a = advance(Some(&a), d, 1).unwrap();
        assert_eq!(a.completions, 1);
        let mut d = observation(RawState::Idle, false);
        d.session_id = a.session_id.clone();
        d.completion_epoch = a.completion_epoch.clone();
        d.completed_turns = Some(3);
        let a = advance(Some(&a), d, 2).unwrap();
        assert_eq!(a.completions, 3);
        let mut d = observation(RawState::Idle, false);
        d.session_id = a.session_id.clone();
        d.completion_epoch = a.completion_epoch.clone();
        d.completed_turns = Some(2);
        assert!(advance(Some(&a), d, 3).is_err());
        let mut d = observation(RawState::Idle, false);
        d.session_id = Some("new/session".into());
        d.completion_epoch = Some("new-launch".into());
        d.completed_turns = Some(0);
        assert_eq!(advance(Some(&a), d, 4).unwrap().completions, 3);
    }

    #[test]
    fn durable_counter_does_not_double_count_observed_working_to_idle() {
        let mut d = observation(RawState::Working, false);
        d.completed_turns = Some(0);
        d.completion_epoch = Some("launch".into());
        let a = advance(None, d, 0).unwrap();
        let mut d = observation(RawState::Idle, false);
        d.completed_turns = Some(1);
        d.completion_epoch = Some("launch".into());
        let a = advance(Some(&a), d, 1).unwrap();
        assert_eq!((a.state, a.completions), (State::Done, 1));
    }
    #[test]
    fn native_session_navigation_does_not_replay_completions() {
        let mut d = observation(RawState::Idle, false);
        d.session_id = Some("first".into());
        d.completion_epoch = Some("launch".into());
        d.completed_turns = Some(2);
        let a = advance(None, d, 0).unwrap();
        for session in ["home", "second", "first"] {
            let mut d = observation(RawState::Idle, false);
            d.session_id = Some(session.into());
            d.completion_epoch = a.completion_epoch.clone();
            d.completed_turns = Some(2);
            let next = advance(Some(&a), d, 1).unwrap();
            assert_eq!(next.completions, 2);
        }
        let mut d = observation(RawState::Idle, false);
        d.completed_turns = Some(2);
        assert!(advance(None, d, 0).is_err());
    }
}
