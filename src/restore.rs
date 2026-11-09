//! Restore: after the tmux server dies (a crash, a reboot, `kill-server`), bring its agents back
//! on request.
//!
//! The daemon keeps `roster.json`: the tmux server it watches and, for every window holding an
//! agent, the window's session, index, name and layout and each pane's cwd, with the agent's
//! kind, session id, name and parent. Running tasks are recorded too. A daemon that starts on a
//! different server turns a roster with agents left to bring back into `restore.json`, which
//! waits until someone restores or discards it: the CLI's `restore`, or `b` in the sidebar.
//! Nothing is restored without being asked, every time.

mod layout;

use crate::agents::Launch;
use crate::diagnostics::{self, Issue};
use crate::index::{self, Agent, Kind};
use crate::tasks::{self, Definition, Outcome, Status, Task};
use crate::{commands, tmux};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

const ROSTER: &str = "roster.json";
const PENDING: &str = "restore.json";
/// A changed roster is written once it has held this long, so the moments while a machine shuts
/// down, when agents may exit before tmux does, never replace the roster worth restoring.
const SETTLE_MS: u64 = 3_000;
const SEP: char = '\x1f';

/// One tmux server: a restarted server has a new pid and start time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub pid: u32,
    /// Unix seconds.
    pub start_time: u64,
}

impl Server {
    fn current() -> Result<Server> {
        let out = tmux::run(&["display-message", "-p", "#{pid} #{start_time}"])?;
        let (pid, start) = out
            .trim()
            .split_once(' ')
            .with_context(|| format!("unexpected tmux server identity {out:?}"))?;
        Ok(Server {
            pid: pid.parse().context("tmux server pid")?,
            start_time: start.parse().context("tmux server start time")?,
        })
    }
}

/// What the daemon last saw worth bringing back.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Roster {
    pub server: Server,
    /// Every window holding an agent, by session name and window index.
    pub windows: Vec<Window>,
    /// Every running task.
    pub tasks: Vec<Definition>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub session: String,
    pub index: u32,
    pub name: String,
    /// `#{window_layout}` without the sidebar.
    pub layout: String,
    /// Every pane but the sidebar, in pane order.
    pub panes: Vec<Pane>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Pane {
    pub index: u32,
    pub cwd: String,
    pub agent: Option<Recorded>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Recorded {
    /// Its pane id then, which parent links refer to.
    pub pane: String,
    pub kind: Kind,
    pub session_id: Option<String>,
    pub name: String,
    /// The name was given (by spawn, rename or AI naming) rather than made from the pane id.
    pub named: bool,
    /// Pane id of its parent then.
    pub parent: Option<String>,
}

impl Recorded {
    /// The command that reopens its session, or why there is none.
    fn resume(&self) -> std::result::Result<Launch, String> {
        let Some(resume) = self.kind.adapter().resume else {
            return Err(format!(
                "{} publishes no session id to resume",
                display_kind(self.kind)
            ));
        };
        match &self.session_id {
            Some(id) => Ok(resume(id)),
            None => Err("it published no session id".into()),
        }
    }
}

fn display_kind(kind: Kind) -> &'static str {
    match kind {
        Kind::Claude => "Claude Code",
        Kind::Codex => "Codex",
        Kind::Pi => "Pi",
        Kind::Opencode => "OpenCode",
        Kind::Agy => "Antigravity",
        Kind::Grok => "Grok Build",
    }
}

impl Roster {
    fn agents(&self) -> impl Iterator<Item = (&Window, &Recorded)> {
        self.windows.iter().flat_map(|w| {
            w.panes
                .iter()
                .filter_map(move |p| Some((w, p.agent.as_ref()?)))
        })
    }

    /// Agents a restore would bring back, before checking what runs now.
    pub fn restorable(&self) -> usize {
        self.agents().filter(|(_, a)| a.resume().is_ok()).count()
    }
}

/// The roster of this server, from the daemon's snapshot of it.
fn record(
    server: &Server,
    agents: &[Agent],
    panes: &[tmux::Pane],
    windows: &[tmux::WindowSize],
    tasks: &[Task],
) -> Result<Roster> {
    let mut recorded = Vec::new();
    let mut seen = HashSet::new();
    for agent in agents {
        if !seen.insert(agent.window.as_str()) {
            continue;
        }
        let window = windows
            .iter()
            .find(|w| w.id == agent.window)
            .with_context(|| format!("window {} missing from its own snapshot", agent.window))?;
        let mut members: Vec<&tmux::Pane> =
            panes.iter().filter(|p| p.window == agent.window).collect();
        members.sort_by_key(|p| p.pane_index);
        let sidebars: Vec<&str> = members
            .iter()
            .filter(|p| p.sidebar)
            .map(|p| p.id.as_str())
            .collect();
        let first = members.first().context("agent window without panes")?;
        recorded.push(Window {
            session: first.session.clone(),
            index: first.window_index,
            name: window.name.clone(),
            layout: if sidebars.is_empty() {
                window.layout.clone()
            } else {
                layout::without(&window.layout, &sidebars)?
            },
            panes: members
                .iter()
                .filter(|p| !p.sidebar)
                .map(|p| Pane {
                    index: p.pane_index,
                    cwd: p.cwd.clone(),
                    agent: agents.iter().find(|a| a.pane == p.id).map(|a| Recorded {
                        pane: a.pane.clone(),
                        kind: a.kind,
                        session_id: a.session_id.clone(),
                        name: a.name.clone(),
                        named: !p.name.is_empty(),
                        parent: a.parent.clone(),
                    }),
                })
                .collect(),
        });
    }
    recorded.sort_by(|a, b| (&a.session, a.index).cmp(&(&b.session, b.index)));
    Ok(Roster {
        server: server.clone(),
        windows: recorded,
        tasks: tasks
            .iter()
            .filter(|t| t.status == Status::Running)
            .map(|t| t.definition.clone())
            .collect(),
    })
}

fn path(name: &str) -> Result<PathBuf> {
    Ok(index::state_dir()?.join(name))
}

fn read(name: &str) -> Result<Option<Roster>> {
    let path = path(name)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

fn write(name: &str, roster: &Roster) -> Result<()> {
    let path = path(name)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(roster)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))
}

/// The agents waiting to be brought back, if any.
pub fn pending() -> Result<Option<Roster>> {
    read(PENDING)
}

/// A roster left by another server, with an agent to bring back that is not running here,
/// becomes the pending restore, replacing an older one.
fn pending_from(previous: Option<Roster>, server: &Server, live: &[Agent]) -> Option<Roster> {
    let previous = previous.filter(|r| r.server != *server)?;
    let waiting = previous
        .agents()
        .any(|(_, a)| a.resume().is_ok() && running(a, live).is_none());
    waiting.then_some(previous)
}

/// The live agent holding a recorded session, if one does.
fn running<'a>(recorded: &Recorded, live: &'a [Agent]) -> Option<&'a Agent> {
    let id = recorded.session_id.as_ref()?;
    live.iter()
        .find(|a| a.kind == recorded.kind && a.session_id.as_ref() == Some(id))
}

/// The daemon's part: take the pending restore once, then keep the roster.
pub struct Recorder {
    server: Server,
    /// The roster on disk is this server's, so nothing else is left to take.
    taken: bool,
    written: Option<Roster>,
    /// A changed roster and when it was first seen.
    candidate: Option<(Roster, u64)>,
}

pub struct Tick {
    /// Agents a restore would bring back: zero without a pending restore.
    pub pending: usize,
    pub issues: Vec<Issue>,
}

impl Recorder {
    pub fn start() -> Result<Self> {
        Ok(Self {
            server: Server::current().context("reading the tmux server's identity")?,
            taken: false,
            written: None,
            candidate: None,
        })
    }

    /// `clean`: every kind scanned without fault, so `agents` is the whole truth. An agent
    /// withdrawn for a fault stays in the roster until it is seen to be gone.
    pub fn tick(
        &mut self,
        agents: &[Agent],
        panes: &[tmux::Pane],
        geometry: &tmux::Geometry,
        tasks: &[Task],
        clean: bool,
        now: u64,
    ) -> Tick {
        let mut issues = Vec::new();
        // The previous server's roster is read before anything here replaces it.
        if !self.taken {
            match self.take(agents) {
                Ok(()) => self.taken = true,
                Err(error) => issues.push(Issue::new("restore:roster", error)),
            }
        }
        if self.taken
            && clean
            && let Err(error) = self.keep(agents, panes, geometry, tasks, now)
        {
            issues.push(Issue::new("restore:roster", error));
        }
        let pending = match pending() {
            Ok(pending) => pending.map_or(0, |p| p.restorable()),
            Err(error) => {
                issues.push(Issue::new("restore:pending", error));
                0
            }
        };
        Tick { pending, issues }
    }

    fn take(&self, live: &[Agent]) -> Result<()> {
        let previous = read(ROSTER)?;
        if previous.as_ref().is_some_and(|r| r.server == self.server) {
            return Ok(());
        }
        if let Some(pending) = pending_from(previous, &self.server, live) {
            write(PENDING, &pending)?;
            diagnostics::record(
                "restore",
                &format!(
                    "{} agents from before tmux restarted can be brought back",
                    pending.restorable()
                ),
            )?;
        }
        // From here the roster on disk is this server's.
        write(ROSTER, &self.empty())
    }

    fn empty(&self) -> Roster {
        Roster {
            server: self.server.clone(),
            windows: Vec::new(),
            tasks: Vec::new(),
        }
    }

    fn keep(
        &mut self,
        agents: &[Agent],
        panes: &[tmux::Pane],
        geometry: &tmux::Geometry,
        tasks: &[Task],
        now: u64,
    ) -> Result<()> {
        let roster = record(&self.server, agents, panes, &geometry.windows, tasks)?;
        if self.written.as_ref().unwrap_or(&self.empty()) == &roster {
            self.candidate = None;
            return Ok(());
        }
        match &self.candidate {
            Some((candidate, since)) if *candidate == roster => {
                if now.saturating_sub(*since) >= SETTLE_MS {
                    write(ROSTER, &roster)?;
                    self.written = Some(roster);
                    self.candidate = None;
                }
            }
            _ => self.candidate = Some((roster, now)),
        }
        Ok(())
    }
}

/// The status bar segment while agents wait to be brought back.
pub fn status(pending: usize) -> String {
    if pending == 0 {
        return String::new();
    }
    format!("#[fg=#89b4fa]↺ {pending}#[default] ")
}

/// One agent as the lists show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Listed {
    pub name: String,
    pub kind: Kind,
    /// Its session: sessions are projects.
    pub project: String,
    pub cwd: String,
}

/// What a restore would do, from the pending restore and what runs now.
#[derive(Debug)]
pub struct Plan {
    pub windows: Vec<WindowPlan>,
    /// Agents it brings back.
    pub agents: Vec<Listed>,
    /// Agents it cannot bring back, and why.
    pub skipped: Vec<(Listed, String)>,
    /// Tasks that were running.
    pub tasks: Vec<Definition>,
}

#[derive(Debug)]
pub struct WindowPlan {
    pub session: String,
    pub index: u32,
    pub name: String,
    pub layout: String,
    pub panes: Vec<PanePlan>,
}

#[derive(Debug)]
pub struct PanePlan {
    pub cwd: String,
    /// The folder no longer exists, so tmux opens the pane in your home folder.
    pub gone: bool,
    /// The agent this pane brings back, with the command that resumes it. Every other pane is a
    /// login shell, including one whose agent cannot come back.
    pub agent: Option<(Recorded, Launch)>,
}

/// A window is rebuilt when it brings back at least one agent.
pub fn plan(pending: &Roster, live: &[Agent], exists: impl Fn(&str) -> bool) -> Plan {
    let mut plan = Plan {
        windows: Vec::new(),
        agents: Vec::new(),
        skipped: Vec::new(),
        tasks: pending.tasks.clone(),
    };
    for window in &pending.windows {
        let mut panes = Vec::new();
        for pane in &window.panes {
            let gone = !exists(&pane.cwd);
            let agent = pane.agent.as_ref().and_then(|agent| {
                let listed = Listed {
                    name: agent.name.clone(),
                    kind: agent.kind,
                    project: window.session.clone(),
                    cwd: pane.cwd.clone(),
                };
                let launch = match (agent.resume(), running(agent, live)) {
                    (Err(why), _) => Err(why),
                    (Ok(_), Some(other)) => Err(format!("already running as {}", other.name)),
                    (Ok(_), None) if gone => Err("its folder is gone".into()),
                    (Ok(launch), None) => Ok(launch),
                };
                match launch {
                    Ok(launch) => {
                        plan.agents.push(listed);
                        Some((agent.clone(), launch))
                    }
                    Err(why) => {
                        plan.skipped.push((listed, why));
                        None
                    }
                }
            });
            panes.push(PanePlan {
                cwd: pane.cwd.clone(),
                gone,
                agent,
            });
        }
        if panes.iter().any(|p| p.agent.is_some()) {
            plan.windows.push(WindowPlan {
                session: window.session.clone(),
                index: window.index,
                name: window.name.clone(),
                layout: window.layout.clone(),
                panes,
            });
        }
    }
    plan
}

fn dir_exists(path: &str) -> bool {
    Path::new(path).is_dir()
}

/// The plan for the pending restore, if any.
pub fn current() -> Result<Option<Plan>> {
    let Some(pending) = pending()? else {
        return Ok(None);
    };
    Ok(Some(plan(&pending, &index::read()?.agents, dir_exists)))
}

/// What a restore did.
#[derive(Default, Debug)]
pub struct Report {
    pub done: Vec<String>,
    /// What came back differently, and what stays closed and why.
    pub notes: Vec<String>,
    pub failed: Vec<String>,
}

/// Restores and discards one at a time, so nothing is brought back twice.
fn lock() -> Result<std::fs::File> {
    let dir = index::state_dir()?;
    std::fs::create_dir_all(&dir).context("creating state directory")?;
    let file = std::fs::File::create(dir.join("restore.lock")).context("opening restore.lock")?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("locking restore.lock");
    }
    Ok(file)
}

fn remove_pending() -> Result<()> {
    let path = path(PENDING)?;
    std::fs::remove_file(&path).with_context(|| format!("removing {}", path.display()))
}

/// Forget the pending restore. Returns how many agents it would have brought back.
pub fn discard() -> Result<usize> {
    let _lock = lock()?;
    let pending = pending()?.context("nothing to restore")?;
    remove_pending()?;
    let count = pending.restorable();
    diagnostics::record(
        "restore",
        &format!("discarded {count} agents from before tmux restarted"),
    )?;
    Ok(count)
}

/// Bring back the pending restore's agents, and its tasks when asked, then forget it. A part
/// that fails is reported and logged; the rest still comes back.
pub fn perform(with_tasks: bool) -> Result<Report> {
    let _lock = lock()?;
    let pending = pending()?.context("nothing to restore")?;
    let index = index::read()?;
    let plan = plan(&pending, &index.agents, dir_exists);
    let mut report = Report::default();
    let mut builder = Builder {
        shell: tmux::default_shell(None)?,
        names: index.agents.iter().map(|a| a.name.clone()).collect(),
        windows: occupied_windows()?,
        moved: HashMap::new(),
    };
    for window in &plan.windows {
        if let Err(error) = builder.build(window, &mut report) {
            report.failed.push(format!(
                "window {} of {}: {error:#}",
                window.name, window.session
            ));
        }
    }
    for window in &plan.windows {
        for (agent, _) in window.panes.iter().filter_map(|p| p.agent.as_ref()) {
            let (Some(new), Some(parent)) = (builder.moved.get(&agent.pane), &agent.parent) else {
                continue;
            };
            // A parent that did not come back leaves no link.
            if let Some(parent) = builder.moved.get(parent)
                && let Err(error) =
                    tmux::run(&["set-option", "-p", "-t", new, "@agent-plane-parent", parent])
            {
                report
                    .failed
                    .push(format!("parent of {}: {error:#}", agent.name));
            }
        }
    }
    if with_tasks {
        for task in &plan.tasks {
            match tasks::start(task) {
                Ok((_, Outcome::AlreadyRunning)) => report
                    .done
                    .push(format!("task {} was already running", task.name)),
                Ok(_) => report.done.push(format!("task {} is running", task.name)),
                Err(error) => report.failed.push(format!("task {}: {error:#}", task.name)),
            }
        }
    }
    remove_pending()?;
    for (agent, why) in &plan.skipped {
        report.notes.push(format!(
            "{} ({}) stays closed: {why}",
            agent.name,
            agent.kind.as_str()
        ));
    }
    diagnostics::record(
        "restore",
        &format!(
            "restored {} agents{}",
            builder.moved.len(),
            if with_tasks { " and tasks" } else { "" }
        ),
    )?;
    for failure in &report.failed {
        diagnostics::record("restore", failure)?;
    }
    Ok(report)
}

/// Session name and window index of every window there is.
fn occupied_windows() -> Result<HashSet<(String, u32)>> {
    tmux::run(&[
        "list-windows",
        "-a",
        "-F",
        "#{session_name}\x1f#{window_index}",
    ])?
    .lines()
    .map(|line| {
        let (session, index) = line
            .split_once(SEP)
            .with_context(|| format!("unexpected list-windows row {line:?}"))?;
        Ok((session.to_owned(), index.parse().context("window_index")?))
    })
    .collect()
}

struct Builder {
    shell: String,
    /// Names in use, so a restored name never takes another agent's.
    names: HashSet<String>,
    windows: HashSet<(String, u32)>,
    /// New pane by recorded pane, for every agent brought back.
    moved: HashMap<String, String>,
}

impl Builder {
    /// The command a pane runs: an agent resuming through the same interactive login shell as
    /// spawn, or a login shell.
    fn command(&self, pane: &PanePlan) -> Vec<String> {
        match &pane.agent {
            Some((_, launch)) => vec![
                self.shell.clone(),
                "-ilc".into(),
                commands::shell_launch(&self.shell, launch.command, &launch.args),
            ],
            None => vec![self.shell.clone(), "-il".into()],
        }
    }

    /// The window, its panes in order, then its layout. A sidebar opening meanwhile waits for
    /// the window to be complete, so it never comes between the panes and their layout.
    fn build(&mut self, window: &WindowPlan, report: &mut Report) -> Result<()> {
        let _sidebars = crate::sidebar::Lock::take()?;
        let [first, rest @ ..] = window.panes.as_slice() else {
            bail!("no panes");
        };
        let new_session = !self.windows.iter().any(|(s, _)| *s == window.session);
        let free = !self
            .windows
            .contains(&(window.session.clone(), window.index));
        let target = if free {
            format!("={}:{}", window.session, window.index)
        } else {
            format!("={}:", window.session)
        };
        let mut args: Vec<String> = if new_session {
            ["new-session", "-d", "-s", &window.session]
                .map(String::from)
                .to_vec()
        } else {
            ["new-window", "-d", "-t", &target]
                .map(String::from)
                .to_vec()
        };
        args.extend(
            [
                "-P",
                "-F",
                "#{window_id}\x1f#{window_index}\x1f#{pane_id}",
                "-n",
                &window.name,
                "-c",
                &first.cwd,
            ]
            .map(String::from),
        );
        args.extend(self.command(first));
        let out = run(&args)?;
        let [id, index, pane] = out.trim().split(SEP).collect::<Vec<_>>()[..] else {
            bail!("unexpected new window output {out:?}");
        };
        let mut index: u32 = index.parse().context("window_index")?;
        // A new session opens on its first index; move the window to the one it had.
        if new_session && index != window.index {
            let to = format!("={}:{}", window.session, window.index);
            tmux::run(&["move-window", "-s", id, "-t", &to])?;
            index = window.index;
        }
        self.windows.insert((window.session.clone(), index));
        let location = format!("{}:{index}", window.session);
        let mut last = pane.to_owned();
        self.started(first, &last, &location, report)?;
        for pane in rest {
            let mut args: Vec<String> = [
                "split-window",
                "-d",
                "-t",
                &last,
                "-P",
                "-F",
                "#{pane_id}",
                "-c",
                &pane.cwd,
            ]
            .map(String::from)
            .to_vec();
            args.extend(self.command(pane));
            last = run(&args)?.trim().to_owned();
            // Even out the panes so far, so each split has room in a small new window.
            tmux::run(&["select-layout", "-t", id, "tiled"])?;
            self.started(pane, &last, &location, report)?;
        }
        if let Err(error) = tmux::run(&["select-layout", "-t", id, &window.layout]) {
            report
                .failed
                .push(format!("layout of {location}: {error:#}"));
        }
        Ok(())
    }

    /// Mark a new pane as placed, give its agent back its name and note it.
    fn started(
        &mut self,
        pane: &PanePlan,
        id: &str,
        location: &str,
        report: &mut Report,
    ) -> Result<()> {
        if pane.gone {
            report.notes.push(format!(
                "{} is gone: a shell in {location} opened in your home folder",
                pane.cwd
            ));
        }
        let Some((agent, _)) = &pane.agent else {
            return Ok(());
        };
        // Restored agents are where their window was, so they are never moved to a project.
        tmux::mark_placed(id)?;
        self.moved.insert(agent.pane.clone(), id.to_owned());
        let name = if !agent.named {
            format!("{}-{}", agent.kind.as_str(), id.trim_start_matches('%'))
        } else if self.names.insert(agent.name.clone()) {
            tmux::run(&[
                "set-option",
                "-p",
                "-t",
                id,
                "@agent-plane-name",
                &agent.name,
            ])?;
            agent.name.clone()
        } else {
            let default = format!("{}-{}", agent.kind.as_str(), id.trim_start_matches('%'));
            report.notes.push(format!(
                "{} is taken by another agent: brought back as {default}",
                agent.name
            ));
            default
        };
        report.done.push(if name == agent.name {
            format!("{name} ({}) is back in {location}", agent.kind.as_str())
        } else {
            format!(
                "{} ({}) is back in {location} as {name}",
                agent.name,
                agent.kind.as_str()
            )
        });
        Ok(())
    }
}

fn run(args: &[String]) -> Result<String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    tmux::run(&args)
}

/// The lists `restore` prints: what comes back, what cannot and why, and the tasks.
pub fn describe(plan: &Plan) -> String {
    let mut out = String::new();
    let rows = |rows: Vec<Vec<String>>| -> String {
        let mut widths = Vec::new();
        for row in &rows {
            widths.resize(widths.len().max(row.len()), 0);
            for (w, cell) in widths.iter_mut().zip(row) {
                *w = (*w).max(cell.chars().count());
            }
        }
        rows.iter()
            .map(|row| {
                let cells: Vec<String> = row
                    .iter()
                    .zip(&widths)
                    .map(|(cell, w)| format!("{cell:<w$}"))
                    .collect();
                format!("  {}\n", cells.join("  ").trim_end())
            })
            .collect()
    };
    out.push_str(&format!(
        "{} agents from before tmux restarted can be brought back:\n",
        plan.agents.len()
    ));
    out.push_str(&rows(
        plan.agents
            .iter()
            .map(|a| {
                vec![
                    a.name.clone(),
                    a.kind.as_str().into(),
                    a.project.clone(),
                    commands::home_relative(&a.cwd),
                ]
            })
            .collect(),
    ));
    if !plan.skipped.is_empty() {
        out.push_str("Cannot be brought back:\n");
        out.push_str(&rows(
            plan.skipped
                .iter()
                .map(|(a, why)| {
                    vec![
                        a.name.clone(),
                        a.kind.as_str().into(),
                        a.project.clone(),
                        commands::home_relative(&a.cwd),
                        why.clone(),
                    ]
                })
                .collect(),
        ));
    }
    if !plan.tasks.is_empty() {
        out.push_str("Tasks that were running:\n");
        out.push_str(&rows(
            plan.tasks
                .iter()
                .map(|t| {
                    vec![
                        t.name.clone(),
                        format!("started by {}", t.started_by.as_deref().unwrap_or("-")),
                        commands::home_relative(&t.cwd),
                        tasks::display_command(&t.command),
                    ]
                })
                .collect(),
        ));
    }
    out
}

pub fn run_cli(with_tasks: bool, no_tasks: bool, forget: bool) -> Result<()> {
    if forget {
        let count = discard()?;
        println!("forgot {count} agents from before tmux restarted");
        return Ok(());
    }
    if !with_tasks && !no_tasks {
        match current()? {
            None => println!("nothing to restore"),
            Some(plan) => {
                print!("{}", describe(&plan));
                println!(
                    "Bring them back with `tmux-agent-plane restore --tasks` (agents and tasks) or `--no-tasks` (agents only); forget them with `--discard`"
                );
            }
        }
        return Ok(());
    }
    let report = perform(with_tasks)?;
    for line in report.done.iter().chain(&report.notes) {
        println!("{line}");
    }
    for line in &report.failed {
        eprintln!("not restored: {line}");
    }
    if !report.failed.is_empty() {
        bail!(
            "{} parts of the restore failed; see above and {}",
            report.failed.len(),
            diagnostics::log_path()?.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::State;

    fn server(pid: u32) -> Server {
        Server {
            pid,
            start_time: 1_700_000_000,
        }
    }

    fn agent(pane: &str, kind: Kind, session_id: Option<&str>, name: &str) -> Agent {
        Agent {
            name: name.into(),
            pane: pane.into(),
            kind,
            state: State::Idle,
            detail: None,
            title: String::new(),
            location: "pro:1.0".into(),
            session: "pro".into(),
            window: "@1".into(),
            cwd: "/work/pro".into(),
            parent: None,
            session_id: session_id.map(Into::into),
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    fn tmux_pane(id: &str, window: &str, index: u32, sidebar: bool, name: &str) -> tmux::Pane {
        tmux::Pane {
            id: id.into(),
            pid: 1,
            session: "pro".into(),
            window: window.into(),
            window_index: 2,
            pane_index: index,
            cwd: "/work/pro".into(),
            title: String::new(),
            active: false,
            viewed: false,
            sidebar,
            name: name.into(),
            parent: String::new(),
            placed: true,
            task: false,
        }
    }

    fn geometry() -> tmux::Geometry {
        tmux::Geometry {
            clients: Vec::new(),
            windows: vec![tmux::WindowSize {
                id: "@1".into(),
                size: tmux::Size {
                    width: 120,
                    height: 40,
                },
                zoomed: false,
                sizing: "latest".into(),
                name: "api".into(),
                layout:
                    "1846,120x40,0,0{38x40,0,0,3,40x40,39,0,0,40x40,80,0[40x20,80,0,1,40x19,80,21,2]}"
                        .into(),
            }],
        }
    }

    /// A window with a sidebar (%3), a named Claude (%0), a shell (%1) and a Codex child (%2).
    fn snapshot() -> (Vec<Agent>, Vec<tmux::Pane>) {
        let claude = agent("%0", Kind::Claude, Some("c-1"), "reviewer");
        let mut codex = agent("%2", Kind::Codex, None, "codex-2");
        codex.parent = Some("%0".into());
        let panes = vec![
            tmux_pane("%3", "@1", 0, true, ""),
            tmux_pane("%0", "@1", 1, false, "reviewer"),
            tmux_pane("%1", "@1", 2, false, ""),
            tmux_pane("%2", "@1", 3, false, ""),
            // A window without agents is not recorded.
            tmux_pane("%9", "@7", 0, false, ""),
        ];
        (vec![claude, codex], panes)
    }

    fn task(name: &str, status: Status) -> Task {
        Task {
            definition: Definition {
                name: name.into(),
                cwd: "/work/pro".into(),
                command: vec!["npm".into(), "run".into(), "dev".into()],
                started_by: Some("reviewer".into()),
            },
            pane: "%20".into(),
            session: tasks::SESSION.into(),
            window: "@20".into(),
            location: "agent-plane-tasks:0.0".into(),
            started_at: 0,
            status,
            ports: Vec::new(),
        }
    }

    #[test]
    fn the_roster_records_agent_windows_without_sidebars_and_round_trips() {
        let (agents, panes) = snapshot();
        let tasks = [
            task("web", Status::Running),
            task(
                "old",
                Status::Exited {
                    exit_code: 1,
                    exited_at: 0,
                },
            ),
        ];
        let roster = record(&server(7), &agents, &panes, &geometry().windows, &tasks).unwrap();
        assert_eq!(roster.windows.len(), 1);
        let window = &roster.windows[0];
        assert_eq!((window.session.as_str(), window.index), ("pro", 2));
        assert_eq!(window.name, "api");
        assert_eq!(layout::panes(&window.layout).unwrap(), 3);
        let indexes: Vec<u32> = window.panes.iter().map(|p| p.index).collect();
        assert_eq!(indexes, [1, 2, 3]);
        let claude = window.panes[0].agent.as_ref().unwrap();
        assert!(claude.named && claude.session_id.as_deref() == Some("c-1"));
        assert!(window.panes[1].agent.is_none());
        let codex = window.panes[2].agent.as_ref().unwrap();
        assert!(!codex.named && codex.parent.as_deref() == Some("%0"));
        // Only running tasks.
        assert_eq!(roster.tasks.len(), 1);
        assert_eq!(roster.tasks[0].name, "web");
        let json = serde_json::to_string(&roster).unwrap();
        assert_eq!(serde_json::from_str::<Roster>(&json).unwrap(), roster);
        assert_eq!(roster.restorable(), 1);
    }

    #[test]
    fn another_servers_roster_with_agents_not_running_becomes_pending() {
        let (agents, panes) = snapshot();
        let roster = record(&server(7), &agents, &panes, &geometry().windows, &[]).unwrap();
        // The same server, as after a daemon restart: nothing to restore.
        assert!(pending_from(Some(roster.clone()), &server(7), &[]).is_none());
        assert!(pending_from(None, &server(8), &[]).is_none());
        let pending = pending_from(Some(roster.clone()), &server(8), &[]).unwrap();
        assert_eq!(pending, roster);
        // Its only resumable agent already runs again.
        let live = [agent("%40", Kind::Claude, Some("c-1"), "claude-40")];
        assert!(pending_from(Some(roster.clone()), &server(8), &live).is_none());
        // Only a Codex agent: nothing could come back.
        let codex_only = Roster {
            windows: roster
                .windows
                .iter()
                .cloned()
                .map(|mut w| {
                    w.panes
                        .retain(|p| p.agent.as_ref().is_none_or(|a| a.kind == Kind::Codex));
                    w
                })
                .collect(),
            ..roster
        };
        assert!(pending_from(Some(codex_only), &server(8), &[]).is_none());
    }

    #[test]
    fn every_kind_but_codex_resumes_its_session_and_the_rest_say_why_not() {
        let recorded = |kind, session_id: Option<&str>| Recorded {
            pane: "%1".into(),
            kind,
            session_id: session_id.map(Into::into),
            name: "a".into(),
            named: false,
            parent: None,
        };
        for (kind, command, flag) in [
            (Kind::Claude, "claude", "--resume"),
            (Kind::Opencode, "opencode", "--session"),
            (Kind::Grok, "grok", "--resume"),
            (Kind::Agy, "agy", "--conversation"),
            (Kind::Pi, "pi", "--session"),
        ] {
            let launch = recorded(kind, Some("id-1")).resume().unwrap();
            assert_eq!(launch.command, command);
            assert_eq!(launch.args, [flag, "id-1"]);
            assert_eq!(
                recorded(kind, None).resume().unwrap_err(),
                "it published no session id"
            );
        }
        assert_eq!(
            recorded(Kind::Codex, None).resume().unwrap_err(),
            "Codex publishes no session id to resume"
        );
    }

    #[test]
    fn the_plan_rebuilds_windows_with_an_agent_to_bring_back_and_lists_the_rest() {
        let (agents, panes) = snapshot();
        let tasks = [task("web", Status::Running)];
        let mut roster = record(&server(7), &agents, &panes, &geometry().windows, &tasks).unwrap();
        // A second window whose Claude runs again, and a third whose folder is gone.
        let mut again = roster.windows[0].clone();
        again.index = 3;
        again.panes[0].agent.as_mut().unwrap().session_id = Some("c-2".into());
        let mut gone = roster.windows[0].clone();
        gone.index = 4;
        gone.panes[0].agent.as_mut().unwrap().session_id = Some("c-3".into());
        gone.panes[0].cwd = "/gone".into();
        roster.windows.extend([again, gone]);
        let live = [agent("%40", Kind::Claude, Some("c-2"), "claude-40")];
        let plan = plan(&roster, &live, |cwd| cwd != "/gone");
        assert_eq!(plan.windows.len(), 1);
        let window = &plan.windows[0];
        assert_eq!((window.session.as_str(), window.index), ("pro", 2));
        assert_eq!(window.name, "api");
        assert_eq!(window.panes.len(), 3);
        let (claude, launch) = window.panes[0].agent.as_ref().unwrap();
        assert_eq!(claude.name, "reviewer");
        assert_eq!(launch.args, ["--resume", "c-1"]);
        // The shell and the Codex pane come back as shells, keeping the layout whole.
        assert!(window.panes[1..].iter().all(|p| p.agent.is_none()));
        let names: Vec<&str> = plan.agents.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["reviewer"]);
        let skipped: Vec<(&str, &str)> = plan
            .skipped
            .iter()
            .map(|(a, why)| (a.name.as_str(), why.as_str()))
            .collect();
        assert_eq!(
            skipped,
            [
                ("codex-2", "Codex publishes no session id to resume"),
                ("reviewer", "already running as claude-40"),
                ("codex-2", "Codex publishes no session id to resume"),
                ("reviewer", "its folder is gone"),
                ("codex-2", "Codex publishes no session id to resume"),
            ]
        );
        assert_eq!(plan.tasks.len(), 1);
        let text = describe(&plan);
        assert!(text.starts_with("1 agents from before tmux restarted can be brought back:\n"));
        assert!(text.contains("Cannot be brought back:\n"));
        assert!(text.contains("  web  started by reviewer  /work/pro  npm run dev\n"));
    }

    #[test]
    fn the_status_bar_shows_waiting_agents_only() {
        assert_eq!(status(0), "");
        assert_eq!(status(3), "#[fg=#89b4fa]↺ 3#[default] ");
    }
}
