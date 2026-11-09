//! The watcher: once a second, detect agents, track done/seen, write the index,
//! update the status bar and notify on transitions.

use crate::agents::{ProcessTable, RawState};
use crate::detect::{self, Detected};
use crate::diagnostics::{self, Active, Batch, Issue};
use crate::fit::Fit;
use crate::index::{self, Agent, Index, RecentEvent, State, WindowView};
use crate::popup::{self, Announcements};
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
    if let Ok(clients) = tmux::clients() {
        for client in clients {
            // The popup outlives the exiting daemon and is shown at once: nothing will retry it.
            let _ = popup::show(
                &client.name,
                &format!(
                    "tmux-agent-plane stopped: {reason}; log: {}",
                    path.display()
                ),
            );
        }
    }
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
            diagnostics::record("daemon", "watcher skipped: daemon.lock is already owned")?;
            return Ok(());
        }
        return Err(error).context("locking daemon index");
    }
    diagnostics::record("daemon", "active")?;
    let previous = index::read_any()?;
    let mut order = previous.as_ref().map_or_else(Vec::new, |i| i.order.clone());
    let mut last_agent = previous.as_ref().and_then(|i| i.last_agent.clone());
    let mut prev: HashMap<String, Agent> = previous
        .map(|i| i.agents.into_iter().map(|a| (a.pane.clone(), a)).collect())
        .unwrap_or_default();
    let mut presentation = Presentation::default();
    let mut active = Active::default();
    let mut notifications = Notifications::default();
    let mut recent_event: Option<RecentEvent> = None;
    // Diagnostic announcements use only tmux and are never sent through notification retry.
    let mut announcements = Announcements::default();
    // Optional naming runs outside tmux and never blocks this loop.
    let mut auto_name = None;
    let mut fit = Fit::default();
    let mut placer = crate::projects::Placer::default();
    let mut tasks = crate::tasks::Tracker::default();
    let mut roster = crate::restore::Recorder::start()?;

    loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            return Ok(());
        }
        // Stamp the index before reading anything, so `updated_at` never postdates what it says:
        // a sidebar compares it with the moment of a key press.
        let now = index::now_ms();
        let (panes, geometry, keys) = tmux::snapshot().context("listing tmux panes")?;
        let before = prev.clone();
        // A shared process-table failure invalidates the whole scan and is fatal.
        let processes = ProcessTable::load().context("loading process table")?;
        let batch = fold(
            &mut prev,
            detect::detect_with(&panes, &processes),
            &panes,
            now,
        );
        let agents = batch.values;
        let mut issues = batch.issues;
        issues.extend(crate::autoname::tick(&mut auto_name, &agents, &panes, now));
        order = next_order(order, &agents, &panes);
        last_agent = last_used(last_agent, &before, &agents, &panes);
        recent_event = recent_event.filter(|event| event.is_live(now));
        for agent in &agents {
            if let Some(message) = transition_message(
                before.get(&agent.pane).filter(|a| a.kind == agent.kind),
                agent,
            ) {
                recent_event = RecentEvent::new(agent, now);
                notifications.push(&agent.name, &message);
            }
        }
        issues.extend(notifications.deliver());
        // A failed popup is retried while its client is idle. Its failure never announces itself.
        issues.extend(announcements.deliver());
        let tasks = tasks.tick(&panes, &processes, now);
        issues.extend(tasks.issues);
        let clean = !issues
            .iter()
            .any(|i| i.component.starts_with("scan:") || i.component.starts_with("advance:"));
        let restore = roster.tick(&agents, &panes, &geometry, &tasks.values, clean, now);
        issues.extend(restore.issues);
        let views = windows(&panes);
        issues.extend(fit.tick(&views, &geometry));
        // Index persistence remains fatal and independent of status presentation.
        index::write(&Index {
            updated_at: now,
            agents: agents.clone(),
            recent_event: recent_event.clone(),
            windows: views,
            order: order.clone(),
            last_agent: last_agent.clone(),
            tasks: tasks.values,
            pending_restore: restore.pending,
        })
        .context("writing agent index")?;
        // Last, so everything above saw one consistent placement of the agents.
        issues.extend(placer.tick(&agents, &panes)?);
        // Use the last diagnostic snapshot for presentation, then fold this attempt's issues
        // exactly once. A new fault/recovery reaches the status on the next tick.
        let segment = active.status()
            + &crate::restore::status(restore.pending)
            + &keys_status(&keys)
            + &status_segment(&agents);
        issues.extend(presentation.update(&segment));
        let changes = active.update(&issues)?;
        let changes: Vec<_> = changes
            .into_iter()
            .filter(|message| !message.starts_with("diagnostics:tmux:"))
            .collect();
        if !changes.is_empty() {
            announcements.push(format!(
                "tmux-agent-plane: {}; log: {}",
                changes.join("; "),
                diagnostics::log_path()?.display()
            ))?;
        }
        std::thread::sleep(TICK);
    }
}

/// One record per window, read from its active pane: tmux keeps exactly one per window.
fn windows(panes: &[tmux::Pane]) -> Vec<WindowView> {
    panes
        .iter()
        .filter(|pane| pane.active)
        .map(|pane| WindowView {
            id: pane.window.clone(),
            active_pane: pane.id.clone(),
            viewed: pane.viewed,
            sidebar: panes
                .iter()
                .find(|p| p.window == pane.window && p.sidebar)
                .map(|p| p.id.clone()),
        })
        .collect()
}

/// Rows never move: arrivals append. A live pane keeps its place while its agent is away, so an
/// agent withdrawn by a brief scan fault returns to the same row.
fn next_order(mut order: Vec<String>, agents: &[Agent], panes: &[tmux::Pane]) -> Vec<String> {
    order.retain(|pane| panes.iter().any(|p| &p.id == pane));
    for agent in agents {
        if !order.contains(&agent.pane) {
            order.push(agent.pane.clone());
        }
    }
    order
}

/// The agent a client arrived at most recently. Several clients take turns by arrival, so one
/// staying put never takes it back from another that has moved.
fn last_used(
    last: Option<String>,
    before: &HashMap<String, Agent>,
    agents: &[Agent],
    panes: &[tmux::Pane],
) -> Option<String> {
    let visible = || agents.iter().filter(|a| a.visible);
    visible()
        .find(|a| !before.get(&a.pane).is_some_and(|b| b.visible))
        .map(|a| a.pane.clone())
        .or_else(|| last.filter(|pane| panes.iter().any(|p| &p.id == pane)))
        .or_else(|| visible().next().map(|a| a.pane.clone()))
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
            || Ok(tmux::clients()?.into_iter().map(|c| c.name).collect()),
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
        State::Idle if unseen && !d.pane.visible() => State::Done,
        other => other,
    };
    // Some agents temporarily replace their task title while waiting.
    let title = match before {
        Some(a) if d.preserve_title => a.title.clone(),
        _ => d.title,
    };
    let visible = d.pane.visible();
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
        visible,
    })
}

fn transition_message(before: Option<&Agent>, agent: &Agent) -> Option<String> {
    let before = before?;
    // Durable publishers can complete another turn between polls while still unseen/Done.
    // Footer and desktop delivery share this policy so neither loses that completion.
    let completed_again = agent.state == State::Done && agent.completions > before.completions;
    if (before.state == agent.state && !completed_again) || agent.visible {
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

struct Notification {
    name: String,
    message: String,
}
#[derive(Default)]
struct Notifications {
    pending: Vec<Notification>,
}
impl Notifications {
    fn push(&mut self, name: &str, message: &str) {
        self.pending.push(Notification {
            name: name.into(),
            message: message.into(),
        });
    }
    fn deliver(&mut self) -> Vec<Issue> {
        self.deliver_with(|n| system_notification(&n.name, &n.message))
    }
    fn deliver_with(&mut self, mut send: impl FnMut(&Notification) -> Result<()>) -> Vec<Issue> {
        let mut issues = Vec::new();
        self.pending.retain(|notification| {
            if let Err(error) = send(notification) {
                issues.push(Issue::new("notification:desktop", error));
                true
            } else {
                false
            }
        });
        issues
    }
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

/// While the global keys are off, say so and how to turn them back on.
fn keys_status(keys: &tmux::GlobalKeys) -> String {
    if !keys.off {
        return String::new();
    }
    match &keys.toggle {
        Some(key) => format!("#[fg=#f9e2af]keys off · prefix {key}#[default] "),
        None => "#[fg=#f9e2af]keys off#[default] ".into(),
    }
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

    #[test]
    fn the_status_says_when_global_keys_are_off_and_how_to_turn_them_on() {
        let mut keys = tmux::GlobalKeys::default();
        assert_eq!(keys_status(&keys), "");
        keys.toggle = Some("g".into());
        assert_eq!(keys_status(&keys), "");
        keys.off = true;
        assert_eq!(
            keys_status(&keys),
            "#[fg=#f9e2af]keys off · prefix g#[default] "
        );
    }
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
            active: visible,
            viewed: visible,
            sidebar: false,
            name: String::new(),
            parent: String::new(),
            placed: false,
            task: false,
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

    fn pane(id: &str, window: &str) -> Pane {
        let mut pane = observation(RawState::Idle, false).pane;
        pane.id = id.into();
        pane.window = window.into();
        pane
    }

    fn agent_in(id: &str, state: RawState, since: u64) -> Agent {
        let mut d = observation(state, false);
        d.pane.id = id.into();
        advance(None, d, since).unwrap()
    }

    fn panes(ids: &[&str]) -> Vec<Pane> {
        ids.iter().map(|id| pane(id, "@1")).collect()
    }

    #[test]
    fn windows_report_active_pane_viewing_and_their_sidebar() {
        let mut agent = pane("%1", "@1");
        agent.active = true;
        agent.viewed = true;
        let mut sidebar = pane("%2", "@1");
        sidebar.viewed = true;
        sidebar.sidebar = true;
        let mut hidden = pane("%3", "@2");
        hidden.active = true;
        hidden.sidebar = true;
        assert_eq!(
            windows(&[agent, sidebar, hidden]),
            [
                WindowView {
                    id: "@1".into(),
                    active_pane: "%1".into(),
                    viewed: true,
                    sidebar: Some("%2".into()),
                },
                WindowView {
                    id: "@2".into(),
                    active_pane: "%3".into(),
                    viewed: false,
                    sidebar: Some("%3".into()),
                },
            ]
        );
        assert!(windows(&[pane("%4", "@3")]).is_empty());
        let mut bare = pane("%4", "@3");
        bare.active = true;
        assert_eq!(windows(&[bare])[0].sidebar, None);
    }

    #[test]
    fn order_appends_arrivals_keeps_withdrawn_and_forgets_closed_panes() {
        let idle = agent_in("%1", RawState::Idle, 0);
        let blocked = agent_in("%2", RawState::Blocked, 0);
        let live = panes(&["%1", "%2", "%3"]);
        let order = next_order(Vec::new(), &[idle.clone(), blocked.clone()], &live);
        assert_eq!(order, ["%1", "%2"]);
        // A state change never moves a row.
        let now_blocked = agent_in("%1", RawState::Blocked, 0);
        let order = next_order(order, &[now_blocked, blocked.clone()], &live);
        assert_eq!(order, ["%1", "%2"]);
        // Arrivals append, whatever their state.
        let arrival = agent_in("%3", RawState::Blocked, 0);
        let order = next_order(order, &[idle.clone(), blocked, arrival.clone()], &live);
        assert_eq!(order, ["%1", "%2", "%3"]);
        // Withdrawn by a fault with its pane alive: keeps its place, and returns to it.
        let order = next_order(order, std::slice::from_ref(&arrival), &live);
        assert_eq!(order, ["%1", "%2", "%3"]);
        // A closed pane is forgotten: tmux never reuses its id within a server.
        let order = next_order(order, &[idle], &panes(&["%1", "%3"]));
        assert_eq!(order, ["%1", "%3"]);
    }

    #[test]
    fn last_used_follows_arrivals_and_forgets_closed_panes() {
        let live = panes(&["%1", "%2"]);
        let viewed = |id: &str, on: bool| {
            let mut agent = agent_in(id, RawState::Idle, 0);
            agent.visible = on;
            agent
        };
        let before = |agents: &[Agent]| -> HashMap<String, Agent> {
            agents.iter().map(|a| (a.pane.clone(), a.clone())).collect()
        };
        // First sight of a viewed agent.
        let on_one = [viewed("%1", true), viewed("%2", false)];
        let last = last_used(None, &HashMap::new(), &on_one, &live);
        assert_eq!(last.as_deref(), Some("%1"));
        // Nobody looking: it stays.
        let on_none = [viewed("%1", false), viewed("%2", false)];
        let last = last_used(last, &before(&on_one), &on_none, &live);
        assert_eq!(last.as_deref(), Some("%1"));
        // A second client arrives elsewhere: the arrival wins, and the first never takes it back.
        let on_both = [viewed("%1", true), viewed("%2", true)];
        let last = last_used(Some("%1".into()), &before(&on_one), &on_both, &live);
        assert_eq!(last.as_deref(), Some("%2"));
        let last = last_used(last, &before(&on_both), &on_both, &live);
        assert_eq!(last.as_deref(), Some("%2"));
        // A restart reads its previous agents back: still viewed is no new arrival.
        let last = last_used(Some("%1".into()), &before(&on_one), &on_one, &live);
        assert_eq!(last.as_deref(), Some("%1"));
        // Upgrading from an index with no record of it: whoever is on screen.
        assert_eq!(
            last_used(None, &before(&on_one), &on_one, &live).as_deref(),
            Some("%1")
        );
        // A closed pane is forgotten.
        assert_eq!(
            last_used(Some("%9".into()), &before(&on_none), &on_none, &live),
            None
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
        let mut delivered = Vec::new();
        let failures = notifications.deliver_with(|n| {
            if n.name == "first" {
                anyhow::bail!("desktop unavailable");
            }
            delivered.push(n.name.clone());
            Ok(())
        });
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].component, "notification:desktop");
        assert_eq!(delivered, ["second"]);
        assert_eq!(notifications.pending.len(), 1);
        assert!(
            notifications
                .deliver_with(|n| {
                    delivered.push(n.name.clone());
                    Ok(())
                })
                .is_empty()
        );
        assert!(notifications.pending.is_empty());
        assert_eq!(delivered, ["second", "first"]);
    }

    #[test]
    fn event_metadata_accepts_only_notification_states_and_bounds_name() {
        let working = tick(None, RawState::Working, false);
        assert!(RecentEvent::new(&working, 100).is_none());
        let idle = tick(None, RawState::Idle, false);
        assert!(RecentEvent::new(&idle, 100).is_none());
        for raw in [RawState::Idle, RawState::Blocked] {
            let mut agent = tick(Some(&working), raw, false);
            agent.name = "界".repeat(200);
            let event = RecentEvent::new(&agent, 100).unwrap();
            assert_eq!(event.name.chars().count(), 128);
            assert_eq!(event.state.state(), agent.state);
            assert_eq!((event.occurred_at, event.expires_at), (100, 5100));
        }
    }

    #[test]
    fn durable_new_turn_while_still_done_notifies_once() {
        let completed = |count| {
            let mut d = observation(RawState::Idle, false);
            d.completed_turns = Some(count);
            d.completion_epoch = Some("launch".into());
            d
        };
        let before = advance(None, completed(1), 100).unwrap();
        let after = advance(Some(&before), completed(2), 200).unwrap();
        assert_eq!((before.state, after.state), (State::Done, State::Done));
        assert!(transition_message(Some(&before), &after).is_some());
        let unchanged = advance(Some(&after), completed(2), 300).unwrap();
        assert!(transition_message(Some(&after), &unchanged).is_none());
        let mut watched = after.clone();
        watched.visible = true;
        assert!(transition_message(Some(&before), &watched).is_none());
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
                active: false,
                viewed: false,
                sidebar: false,
                name: String::new(),
                parent: String::new(),
                placed: false,
                task: false,
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
