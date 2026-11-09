//! Background tasks: long-running commands such as dev servers, watchers and tunnels, started
//! through the plane so every agent sees one list and nobody starts a second copy.
//!
//! A task is the only pane of a window named after it, in one detached session. tmux holds its
//! definition as pane options and, with `remain-on-exit`, keeps a finished task's output and exit
//! status, so tmux is the source of truth. The daemon folds tasks into the index with the TCP
//! ports they listen on. A task is never an agent.

use crate::agents::ProcessTable;
use crate::diagnostics::{Batch, Issue};
use crate::{commands, index, tmux};
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Command;

/// The detached session holding one window per task.
pub const SESSION: &str = "agent-plane-tasks";
const NAME: &str = "@agent-plane-task";
const CWD: &str = "@agent-plane-task-cwd";
/// The command as a JSON argv.
const COMMAND: &str = "@agent-plane-task-command";
const BY: &str = "@agent-plane-task-by";
/// Unix ms of the last start.
const SINCE: &str = "@agent-plane-task-since";
/// Listening ports are looked up at most this often.
const PORTS_EVERY_MS: u64 = 5_000;
const SEP: char = '\x1f';
/// What a new task window runs until its options are set and the command replaces it.
const PLACEHOLDER: [&str; 2] = ["sleep", "2147483647"];

#[derive(clap::Subcommand)]
pub enum Action {
    /// Start a long-running command in the background, or reuse the running task of that name
    Start {
        /// One word, unique across tmux, e.g. `web-dev`
        name: String,
        /// Working directory (default: current directory)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// The program and its arguments, after `--`
        #[arg(last = true, required = true, value_name = "COMMAND")]
        command: Vec<String>,
    },
    /// List tasks: state, listening ports, cwd, command and who started them
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print a task's screen
    Read {
        name: String,
        /// Lines of scrollback to include above the visible screen
        #[arg(long, default_value_t = 0)]
        lines: u32,
    },
    /// End a task; it leaves the list
    Stop { name: String },
    /// Run a task's command again in its pane
    Restart { name: String },
}

/// Everything needed to start a task again, as recorded on its pane.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub name: String,
    pub cwd: String,
    /// The program, then its arguments.
    pub command: Vec<String>,
    /// The agent that started it, when the caller was a known agent.
    pub started_by: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "lowercase")]
pub enum Status {
    Running,
    /// A signal shows as 128 + its number, as a shell reports it.
    Exited {
        exit_code: i32,
        exited_at: u64,
    },
}

impl Status {
    pub fn label(self) -> String {
        match self {
            Status::Running => "running".into(),
            Status::Exited { exit_code, .. } => format!("exited {exit_code}"),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Task {
    #[serde(flatten)]
    pub definition: Definition,
    /// tmux pane id, e.g. `%154`.
    pub pane: String,
    pub session: String,
    /// tmux window id, e.g. `@104`.
    pub window: String,
    /// `session:window.pane`
    pub location: String,
    /// Unix ms of the last start.
    pub started_at: u64,
    #[serde(flatten)]
    pub status: Status,
    /// TCP ports its processes listen on.
    pub ports: Vec<u16>,
}

/// A task as tmux reports it, with the pid its process tree hangs from.
#[derive(Clone, Debug)]
struct Live {
    task: Task,
    pid: i32,
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Start { name, cwd, command } => {
            let cwd = match cwd {
                Some(dir) => dir,
                None => std::env::current_dir()?,
            };
            let cwd = cwd
                .canonicalize()
                .with_context(|| format!("task cwd {}", cwd.display()))?;
            let definition = Definition {
                name,
                cwd: cwd.to_str().context("task cwd is not UTF-8")?.to_owned(),
                command,
                started_by: caller_agent()?,
            };
            let (task, outcome) = start(&definition)?;
            let note = match outcome {
                Outcome::Started => "",
                Outcome::AlreadyRunning => " (already running)",
                Outcome::Restarted => " (restarted)",
            };
            println!(
                "{} {} {}{note}",
                task.definition.name,
                task.pane,
                task.status.label()
            );
            Ok(())
        }
        Action::List { json } => list(json),
        Action::Read { name, lines } => {
            let screen = tmux::capture(&find(&name)?.task.pane, false, Some(lines))?;
            println!("{}", screen.trim_end());
            Ok(())
        }
        Action::Stop { name } => {
            let task = stop(&name, None)?;
            println!("stopped {name} ({})", task.pane);
            Ok(())
        }
        Action::Restart { name } => {
            let task = restart(&name, None)?;
            println!("{name} {} {}", task.pane, task.status.label());
            Ok(())
        }
    }
}

/// The calling agent's name, by the rule spawn uses for parents: Codex callers are unknown.
fn caller_agent() -> Result<Option<String>> {
    let me = if commands::codex_caller() {
        None
    } else {
        commands::self_pane()?
    };
    let Some(me) = me else { return Ok(None) };
    Ok(index::read()?
        .agents
        .into_iter()
        .find(|a| a.pane == me)
        .map(|a| a.name))
}

/// Task names are one word that is safe as a tmux window name and in any target.
pub fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    if !chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        bail!(
            "task names are one word of letters, digits, '-', '_' or '.', starting with a letter or digit: {name:?}"
        );
    }
    Ok(())
}

/// A word the shell reads literally: no quoting, expansion or syntax.
fn plain_word(word: &str) -> bool {
    !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./+@%=,:~".contains(c))
}

impl Definition {
    fn validate(&self) -> Result<()> {
        validate_name(&self.name)?;
        let Some(program) = self.command.first() else {
            bail!("a task needs a command after --");
        };
        // The program stays literal so aliases and functions apply, so it must be one plain word.
        if !plain_word(program) {
            bail!(
                "the command must start with a program name, one plain word such as `npm`, with its arguments after it; for shell syntax run `sh -c '...'`: {program:?}"
            );
        }
        if !std::path::Path::new(&self.cwd).is_absolute() {
            bail!("task cwd must be absolute: {}", self.cwd);
        }
        if !std::path::Path::new(&self.cwd).is_dir() {
            bail!("task cwd is not a directory: {}", self.cwd);
        }
        Ok(())
    }

    fn same_work(&self, other: &Definition) -> bool {
        self.cwd == other.cwd && self.command == other.command
    }
}

/// The command for people to read: plain words bare, everything else quoted.
pub fn display_command(command: &[String]) -> String {
    command
        .iter()
        .map(|arg| {
            if plain_word(arg) {
                arg.clone()
            } else {
                commands::shell_quote(arg)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Started,
    AlreadyRunning,
    Restarted,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Create,
    Reuse,
    Respawn,
}

/// One name, one piece of work: the same work reuses or reruns its task, other work fails.
fn decide(existing: Option<&Task>, wanted: &Definition) -> Result<Decision> {
    let Some(existing) = existing else {
        return Ok(Decision::Create);
    };
    let have = &existing.definition;
    if !have.same_work(wanted) {
        bail!(
            "task {:?} already exists with different work ({}):\n  cwd: {}\n  command: {}\n  started by: {}\nReuse it, choose another name, or stop it first with `tmux-agent-plane task stop {}` if it is yours",
            have.name,
            existing.status.label(),
            have.cwd,
            display_command(&have.command),
            have.started_by.as_deref().unwrap_or("-"),
            have.name
        );
    }
    Ok(match existing.status {
        Status::Running => Decision::Reuse,
        Status::Exited { .. } => Decision::Respawn,
    })
}

/// Start a task from its definition: the CLI's start, and the one way to bring a recorded task
/// back. A running task with the same name and work is reused; an exited one runs again.
pub fn start(definition: &Definition) -> Result<(Task, Outcome)> {
    definition.validate()?;
    // Two starts of one name must not both create it.
    let _lock = lock()?;
    let existing = strict()?
        .into_iter()
        .find(|live| live.task.definition.name == definition.name);
    match decide(existing.as_ref().map(|live| &live.task), definition)? {
        Decision::Reuse => Ok((
            existing.expect("reuse has a task").task,
            Outcome::AlreadyRunning,
        )),
        Decision::Respawn => {
            let pane = existing.expect("respawn has a task").task.pane;
            launch(&pane, definition)?;
            Ok((find(&definition.name)?.task, Outcome::Restarted))
        }
        Decision::Create => {
            let pane = create_window(definition)?;
            if let Err(error) = launch(&pane, definition) {
                // Leave nothing half-made behind; the launch error is the one that matters.
                let _ = commands::kill_pane(&pane);
                return Err(error);
            }
            Ok((find(&definition.name)?.task, Outcome::Started))
        }
    }
}

/// End a task. `pane` pins the exact task a person confirmed.
pub fn stop(name: &str, pane: Option<&str>) -> Result<Task> {
    let task = pinned(name, pane)?;
    commands::kill_pane(&task.pane)?;
    Ok(task)
}

/// Run a task's recorded command again in its pane, ending it first if it is running.
pub fn restart(name: &str, pane: Option<&str>) -> Result<Task> {
    let task = pinned(name, pane)?;
    launch(&task.pane, &task.definition)?;
    Ok(find(name)?.task)
}

fn pinned(name: &str, pane: Option<&str>) -> Result<Task> {
    let task = find(name)?.task;
    if pane.is_some_and(|pane| pane != task.pane) {
        bail!("task {name} changed; try again");
    }
    Ok(task)
}

fn lock() -> Result<std::fs::File> {
    let dir = index::state_dir()?;
    std::fs::create_dir_all(&dir).context("creating state directory")?;
    let file = std::fs::File::create(dir.join("tasks.lock")).context("opening tasks.lock")?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("locking tasks.lock");
    }
    Ok(file)
}

/// A new window in the tasks session, creating the session on first use. It holds a placeholder
/// until `launch` sets its options and replaces it, so a command that ends at once still leaves
/// its pane and exit status.
fn create_window(definition: &Definition) -> Result<String> {
    let exists = tmux::run(&["list-sessions", "-F", "#{session_name}"])?
        .lines()
        .any(|name| name == SESSION);
    let target = format!("={SESSION}:");
    let mut args = if exists {
        vec!["new-window", "-d", "-t", &target]
    } else {
        vec!["new-session", "-d", "-s", SESSION]
    };
    args.extend([
        "-P",
        "-F",
        "#{pane_id}",
        "-n",
        &definition.name,
        "-c",
        &definition.cwd,
    ]);
    args.extend(PLACEHOLDER);
    Ok(tmux::run(&args)?.trim().to_owned())
}

/// The pane options that record a definition, in the order they are set: the name last, so a
/// pane is a task only once its definition is complete.
fn options(definition: &Definition, since: u64) -> Result<Vec<(&'static str, String)>> {
    Ok(vec![
        ("remain-on-exit", "on".into()),
        (CWD, definition.cwd.clone()),
        (COMMAND, serde_json::to_string(&definition.command)?),
        (BY, definition.started_by.clone().unwrap_or_default()),
        (SINCE, since.to_string()),
        (NAME, definition.name.clone()),
    ])
}

/// tmux ends a command at an argument ending in `;` unless it is escaped.
fn tmux_arg(arg: &str) -> String {
    match arg.strip_suffix(';') {
        Some(head) => format!("{head}\\;"),
        None => arg.to_owned(),
    }
}

/// Record the definition on the pane and run its command there, through an interactive login
/// shell as `spawn` does, so PATH and aliases apply. Earlier output is cleared. One tmux command,
/// so nothing sees a half-recorded task.
fn launch(pane: &str, definition: &Definition) -> Result<()> {
    let shell = tmux::default_shell(None)?;
    let script = commands::command_line(&definition.command[0], &definition.command[1..]);
    let mut args: Vec<String> = Vec::new();
    for (option, value) in options(definition, index::now_ms())? {
        args.extend(["set-option", "-p", "-t", pane, option].map(String::from));
        args.push(tmux_arg(&value));
        args.push(";".into());
    }
    args.extend(["respawn-pane", "-k", "-t", pane, "-c"].map(String::from));
    args.extend([
        tmux_arg(&definition.cwd),
        shell,
        "-ilc".into(),
        tmux_arg(&script),
    ]);
    args.extend([";", "clear-history", "-t", pane].map(String::from));
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    tmux::run(&args).map(drop)
}

const FIELDS: [&str; 15] = [
    "#{pane_id}",
    "#{pane_pid}",
    "#{session_name}",
    "#{window_id}",
    "#{window_index}",
    "#{pane_index}",
    "#{@agent-plane-task}",
    "#{@agent-plane-task-cwd}",
    "#{@agent-plane-task-command}",
    "#{@agent-plane-task-by}",
    "#{@agent-plane-task-since}",
    "#{pane_dead}",
    "#{pane_dead_status}",
    "#{pane_dead_signal}",
    "#{pane_dead_time}",
];

fn parse_row(line: &str) -> Result<Live> {
    let f: Vec<&str> = line.split(SEP).collect();
    let [
        pane,
        pid,
        session,
        window,
        window_index,
        pane_index,
        name,
        cwd,
        command,
        by,
        since,
        dead,
        status,
        signal,
        dead_time,
    ] = f[..]
    else {
        bail!("unexpected task row: {line:?}");
    };
    let status = match dead {
        "0" => Status::Running,
        "1" => {
            let exit_code = match (status, signal) {
                (code, "") if !code.is_empty() => code.parse().context("pane_dead_status")?,
                ("", signal) if !signal.is_empty() => 128 + signal_number(signal)?,
                _ => bail!("task {name} ended without an exit status or signal"),
            };
            Status::Exited {
                exit_code,
                exited_at: dead_time.parse::<u64>().context("pane_dead_time")? * 1000,
            }
        }
        other => bail!("unexpected pane_dead {other:?}"),
    };
    Ok(Live {
        task: Task {
            definition: Definition {
                name: name.into(),
                cwd: cwd.into(),
                command: serde_json::from_str(command)
                    .with_context(|| format!("task {name} command {command:?}"))?,
                started_by: Some(by.to_owned()).filter(|by| !by.is_empty()),
            },
            pane: pane.into(),
            session: session.into(),
            window: window.into(),
            location: format!("{session}:{window_index}.{pane_index}"),
            started_at: since
                .parse()
                .with_context(|| format!("task {name} start time {since:?}"))?,
            status,
            ports: Vec::new(),
        },
        pid: pid.parse().context("pane_pid")?,
    })
}

/// tmux names the signal that ended a pane as `term`, `kill` and so on.
fn signal_number(signal: &str) -> Result<i32> {
    if let Ok(number) = signal.parse() {
        return Ok(number);
    }
    Ok(
        match signal.to_ascii_lowercase().trim_start_matches("sig") {
            "hup" => libc::SIGHUP,
            "int" => libc::SIGINT,
            "quit" => libc::SIGQUIT,
            "ill" => libc::SIGILL,
            "trap" => libc::SIGTRAP,
            "abrt" => libc::SIGABRT,
            "bus" => libc::SIGBUS,
            "fpe" => libc::SIGFPE,
            "kill" => libc::SIGKILL,
            "usr1" => libc::SIGUSR1,
            "segv" => libc::SIGSEGV,
            "usr2" => libc::SIGUSR2,
            "pipe" => libc::SIGPIPE,
            "alrm" => libc::SIGALRM,
            "term" => libc::SIGTERM,
            "xcpu" => libc::SIGXCPU,
            "xfsz" => libc::SIGXFSZ,
            "vtalrm" => libc::SIGVTALRM,
            "prof" => libc::SIGPROF,
            "sys" => libc::SIGSYS,
            _ => bail!("unknown signal {signal:?} in pane_dead_signal"),
        },
    )
}

/// Every task pane, each once however many sessions its window is linked into. A pane whose
/// definition cannot be read is an issue of its own.
fn parse(output: &str) -> Batch<Live> {
    let mut batch = Batch {
        values: Vec::new(),
        issues: Vec::new(),
    };
    for line in output.lines() {
        match parse_row(line) {
            Ok(live) => {
                if !batch
                    .values
                    .iter()
                    .any(|l: &Live| l.task.pane == live.task.pane)
                {
                    batch.values.push(live);
                }
            }
            Err(error) => batch.issues.push(Issue::new("tasks", error)),
        }
    }
    batch
        .values
        .sort_by(|a, b| a.task.definition.name.cmp(&b.task.definition.name));
    batch
}

fn load() -> Result<Batch<Live>> {
    let output = tmux::run(&[
        "list-panes",
        "-a",
        "-f",
        &format!("#{{{NAME}}}"),
        "-F",
        &FIELDS.join(&SEP.to_string()),
    ])?;
    Ok(parse(&output))
}

/// Tasks for a command, which refuses to act on an unreadable definition.
fn strict() -> Result<Vec<Live>> {
    let batch = load()?;
    if let Some(issue) = batch.issues.first() {
        bail!("{}", issue.message);
    }
    Ok(batch.values)
}

fn find(name: &str) -> Result<Live> {
    let tasks = strict()?;
    let names: Vec<String> = tasks
        .iter()
        .map(|live| live.task.definition.name.clone())
        .collect();
    tasks
        .into_iter()
        .find(|live| live.task.definition.name == name)
        .ok_or_else(|| {
            if names.is_empty() {
                anyhow!("no task named {name:?}: no tasks are running")
            } else {
                anyhow!("no task named {name:?}. Tasks: {}", names.join(", "))
            }
        })
}

fn list(json: bool) -> Result<()> {
    let tasks = index::read()?.tasks;
    if json {
        println!("{}", serde_json::to_string_pretty(&tasks)?);
        return Ok(());
    }
    if tasks.is_empty() {
        println!("no tasks running");
        return Ok(());
    }
    let header = ["NAME", "STATE", "PORTS", "CWD", "BY", "STARTED", "COMMAND"].map(String::from);
    let rows: Vec<[String; 7]> = tasks
        .iter()
        .map(|task| {
            [
                task.definition.name.clone(),
                task.status.label(),
                ports_label(&task.ports, ",").unwrap_or_else(|| "-".into()),
                commands::home_relative(&task.definition.cwd),
                task.definition
                    .started_by
                    .clone()
                    .unwrap_or_else(|| "-".into()),
                format!("{} ago", crate::ui::age(task.started_at)),
                display_command(&task.definition.command),
            ]
        })
        .collect();
    let mut widths = [0; 7];
    for row in std::iter::once(&header).chain(&rows) {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    for row in std::iter::once(&header).chain(&rows) {
        let line: Vec<String> = row
            .iter()
            .zip(widths)
            .map(|(cell, w)| format!("{cell:<w$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
    Ok(())
}

/// `:5173` style, or None without ports.
pub fn ports_label(ports: &[u16], separator: &str) -> Option<String> {
    (!ports.is_empty()).then(|| {
        ports
            .iter()
            .map(|port| format!(":{port}"))
            .collect::<Vec<_>>()
            .join(separator)
    })
}

/// The daemon's view of tasks, once a tick. Costs nothing while no pane is a task.
#[derive(Default)]
pub struct Tracker {
    /// Listening ports by task pane, from the last lookup.
    ports: HashMap<String, Vec<u16>>,
    looked_at: Option<u64>,
}

impl Tracker {
    pub fn tick(
        &mut self,
        panes: &[tmux::Pane],
        processes: &ProcessTable,
        now: u64,
    ) -> Batch<Task> {
        if !panes.iter().any(|pane| pane.task) {
            self.ports.clear();
            self.looked_at = None;
            return Batch {
                values: Vec::new(),
                issues: Vec::new(),
            };
        }
        let mut batch = match load() {
            Ok(batch) => batch,
            Err(error) => {
                return Batch {
                    values: Vec::new(),
                    issues: vec![Issue::new("tasks", error)],
                };
            }
        };
        if self
            .looked_at
            .is_none_or(|at| now.saturating_sub(at) >= PORTS_EVERY_MS)
        {
            self.looked_at = Some(now);
            let running: Vec<&Live> = batch
                .values
                .iter()
                .filter(|live| live.task.status == Status::Running)
                .collect();
            match listening_ports(&running, processes) {
                Ok(ports) => self.ports = ports,
                Err(error) => {
                    // Never show ports that could not be checked.
                    self.ports.clear();
                    batch.issues.push(Issue::new("tasks:ports", error));
                }
            }
        }
        Batch {
            values: batch
                .values
                .into_iter()
                .map(|live| {
                    let mut task = live.task;
                    if task.status == Status::Running {
                        task.ports = self.ports.get(&task.pane).cloned().unwrap_or_default();
                    }
                    task
                })
                .collect(),
            issues: batch.issues,
        }
    }
}

/// TCP LISTEN ports of each running task's process tree, by task pane: one lsof for all tasks.
fn listening_ports(tasks: &[&Live], processes: &ProcessTable) -> Result<HashMap<String, Vec<u16>>> {
    let mut owner: HashMap<i32, &str> = HashMap::new();
    for live in tasks {
        owner.insert(live.pid, &live.task.pane);
        for process in processes.processes.values() {
            if processes.is_descendant(process.pid, live.pid) {
                owner.insert(process.pid, &live.task.pane);
            }
        }
    }
    if owner.is_empty() {
        return Ok(HashMap::new());
    }
    let mut pids: Vec<i32> = owner.keys().copied().collect();
    pids.sort_unstable();
    let pids = pids
        .iter()
        .map(i32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let output = Command::new("lsof")
        .args([
            "-nP",
            "-w",
            "-a",
            "-p",
            &pids,
            "-iTCP",
            "-sTCP:LISTEN",
            "-Fpn",
        ])
        .output()
        .context("failed to execute lsof")?;
    // lsof exits 1, silently, when no process listens.
    let nothing = output.status.code() == Some(1) && output.stderr.is_empty();
    if !output.status.success() && !nothing {
        bail!(
            "lsof failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let mut ports: HashMap<String, BTreeSet<u16>> = HashMap::new();
    for (pid, found) in parse_lsof(&String::from_utf8(output.stdout).context("lsof output")?)? {
        let pane = owner
            .get(&pid)
            .with_context(|| format!("lsof reported pid {pid}, which was not asked for"))?;
        ports.entry((*pane).to_owned()).or_default().extend(found);
    }
    Ok(ports
        .into_iter()
        .map(|(pane, ports)| (pane, ports.into_iter().collect()))
        .collect())
}

/// `lsof -F pn` output: a `p<pid>` line, then `f<fd>` and `n<address>:<port>` lines per socket.
fn parse_lsof(output: &str) -> Result<HashMap<i32, BTreeSet<u16>>> {
    let mut ports: HashMap<i32, BTreeSet<u16>> = HashMap::new();
    let mut pid = None;
    for line in output.lines() {
        let (field, value) = line.split_at_checked(1).context("empty lsof line")?;
        match field {
            "p" => pid = Some(value.parse::<i32>().context("lsof pid")?),
            "f" => {}
            "n" => {
                let pid = pid.context("lsof address before its pid")?;
                let (_, port) = value
                    .rsplit_once(':')
                    .with_context(|| format!("lsof address without a port: {value:?}"))?;
                let port = port
                    .parse()
                    .with_context(|| format!("lsof port in {value:?}"))?;
                ports.entry(pid).or_default().insert(port);
            }
            _ => bail!("unexpected lsof line {line:?}"),
        }
    }
    Ok(ports)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definition(name: &str) -> Definition {
        Definition {
            name: name.into(),
            cwd: "/tmp".into(),
            command: vec!["python3".into(), "-m".into(), "http.server".into()],
            started_by: Some("claude-3".into()),
        }
    }

    fn row(definition: &Definition, since: u64, dead: [&str; 4]) -> String {
        let values: HashMap<&str, String> =
            options(definition, since).unwrap().into_iter().collect();
        [
            "%7",
            "4242",
            SESSION,
            "@3",
            "2",
            "0",
            &values[NAME],
            &values[CWD],
            &values[COMMAND],
            &values[BY],
            &values[SINCE],
            dead[0],
            dead[1],
            dead[2],
            dead[3],
        ]
        .join(&SEP.to_string())
    }

    fn task(definition: Definition, status: Status) -> Task {
        Task {
            definition,
            pane: "%7".into(),
            session: SESSION.into(),
            window: "@3".into(),
            location: format!("{SESSION}:2.0"),
            started_at: 5,
            status,
            ports: Vec::new(),
        }
    }

    #[test]
    fn names_are_one_safe_word() {
        for good in ["web", "web-dev", "api_2", "v1.2", "9lives"] {
            validate_name(good).unwrap();
        }
        for bad in [
            "",
            "two words",
            "%7",
            "-web",
            ".hidden",
            "web;",
            "a#b",
            "a:b",
            "tab\t",
            "é",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_program_is_one_plain_word_and_arguments_are_anything() {
        let mut d = definition("web");
        d.command = vec![
            "npm".into(),
            "run".into(),
            "dev -- --port 5173; rm -rf /".into(),
        ];
        d.validate().unwrap();
        for program in ["npm run dev", "$(evil)", "a;b", "'npm'", "", "x|y"] {
            d.command = vec![program.into()];
            assert!(d.validate().is_err(), "{program:?}");
        }
        d.command.clear();
        assert!(d.validate().is_err());
        d = definition("web");
        d.cwd = "relative".into();
        assert!(d.validate().is_err());
        d.cwd = "/definitely/not/here".into();
        assert!(d.validate().is_err());
    }

    #[test]
    fn same_work_is_reused_or_rerun_and_other_work_fails_loudly() {
        let wanted = definition("web");
        assert_eq!(decide(None, &wanted).unwrap(), Decision::Create);
        let running = task(wanted.clone(), Status::Running);
        assert_eq!(decide(Some(&running), &wanted).unwrap(), Decision::Reuse);
        let exited = task(
            wanted.clone(),
            Status::Exited {
                exit_code: 1,
                exited_at: 9,
            },
        );
        assert_eq!(decide(Some(&exited), &wanted).unwrap(), Decision::Respawn);
        // Who asks does not matter; what runs, and where, does.
        let mut other_caller = wanted.clone();
        other_caller.started_by = None;
        assert_eq!(
            decide(Some(&running), &other_caller).unwrap(),
            Decision::Reuse
        );
        let mut other_cwd = wanted.clone();
        other_cwd.cwd = "/var".into();
        let mut other_command = wanted.clone();
        other_command.command.push("8001".into());
        for clash in [other_cwd, other_command] {
            for existing in [&running, &exited] {
                let error = decide(Some(existing), &clash).unwrap_err().to_string();
                assert!(error.contains("cwd: /tmp"), "{error}");
                assert!(error.contains("command: python3 -m http.server"), "{error}");
                assert!(error.contains("started by: claude-3"), "{error}");
            }
        }
    }

    #[test]
    fn a_definition_round_trips_through_pane_options() {
        let mut d = definition("web");
        d.command = vec![
            "printf".into(),
            "quotes '\" $HOME\n\t\x1f #{pane_id} ; é漢".into(),
            String::new(),
        ];
        let live = parse_row(&row(&d, 1234, ["0", "", "", ""])).unwrap();
        assert_eq!(live.task.definition, d);
        assert_eq!(live.task.started_at, 1234);
        assert_eq!(live.task.status, Status::Running);
        assert_eq!(live.task.location, format!("{SESSION}:2.0"));
        assert_eq!(live.pid, 4242);
        d.started_by = None;
        let live = parse_row(&row(&d, 1, ["0", "", "", ""])).unwrap();
        assert_eq!(live.task.definition.started_by, None);
    }

    #[test]
    fn state_comes_from_pane_dead_and_its_status_or_signal() {
        let d = definition("web");
        let status = |dead| parse_row(&row(&d, 1, dead)).map(|live| live.task.status);
        assert_eq!(status(["0", "", "", ""]).unwrap(), Status::Running);
        assert_eq!(
            status(["1", "3", "", "100"]).unwrap(),
            Status::Exited {
                exit_code: 3,
                exited_at: 100_000
            }
        );
        assert_eq!(
            status(["1", "", "15", "100"]).unwrap(),
            Status::Exited {
                exit_code: 143,
                exited_at: 100_000
            }
        );
        assert_eq!(status(["1", "0", "", "7"]).unwrap().label(), "exited 0");
        assert_eq!(
            status(["1", "", "term", "1"]).unwrap(),
            Status::Exited {
                exit_code: 128 + libc::SIGTERM,
                exited_at: 1000
            }
        );
        assert_eq!(
            status(["1", "", "kill", "1"]).unwrap().label(),
            "exited 137"
        );
        assert!(status(["1", "", "bogus", "1"]).is_err());
        assert!(status(["1", "", "", "7"]).is_err());
        assert!(status(["1", "x", "", "7"]).is_err());
        assert!(status(["2", "", "", ""]).is_err());
    }

    #[test]
    fn an_unreadable_task_is_its_own_issue_and_linked_panes_count_once() {
        let good = row(&definition("web"), 1, ["0", "", "", ""]);
        let mut other = definition("api");
        other.command = vec!["api".into()];
        let other = row(&other, 1, ["0", "", "", ""]).replace("%7", "%8");
        let broken = good.replace("%7", "%9").replace("[\"python3\"", "[python3");
        let batch = parse(&[good.as_str(), &good, &broken, &other].join("\n"));
        let names: Vec<_> = batch
            .values
            .iter()
            .map(|live| live.task.definition.name.as_str())
            .collect();
        assert_eq!(names, ["api", "web"]);
        assert_eq!(batch.issues.len(), 1);
        assert_eq!(batch.issues[0].component, "tasks");
        assert!(parse_row("too\x1ffew").is_err());
    }

    #[test]
    fn index_json_reads_back_the_definition() {
        let t = task(
            definition("web"),
            Status::Exited {
                exit_code: 2,
                exited_at: 9,
            },
        );
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(json["name"], "web");
        assert_eq!(json["state"], "exited");
        assert_eq!(json["exit_code"], 2);
        assert_eq!(json["command"][2], "http.server");
        assert_eq!(serde_json::from_value::<Task>(json).unwrap(), t);
        let running = serde_json::to_value(task(definition("web"), Status::Running)).unwrap();
        assert_eq!(running["state"], "running");
        assert!(running.get("exit_code").is_none());
    }

    #[test]
    fn lsof_fields_give_ports_per_process() {
        let output =
            "p100\nf5\nn*:8000\nf6\nn[::1]:5173\nf7\nn127.0.0.1:8000\np200\nf3\nn*:24678\n";
        let ports = parse_lsof(output).unwrap();
        assert_eq!(
            ports[&100].iter().copied().collect::<Vec<_>>(),
            [5173, 8000]
        );
        assert_eq!(ports[&200].iter().copied().collect::<Vec<_>>(), [24678]);
        assert!(parse_lsof("").unwrap().is_empty());
        assert!(parse_lsof("n*:80\n").is_err());
        assert!(parse_lsof("p1\nn*:http\n").is_err());
        assert!(parse_lsof("p1\nx?\n").is_err());
    }

    #[test]
    fn a_trailing_semicolon_stays_literal_for_tmux() {
        assert_eq!(tmux_arg("/a/b;"), "/a/b\\;");
        assert_eq!(tmux_arg("a;b"), "a;b");
        assert_eq!(tmux_arg(""), "");
    }

    #[test]
    fn commands_read_back_as_typed() {
        let command =
            ["python3", "-m", "http.server", "it's here", "--port=8000"].map(String::from);
        assert_eq!(
            display_command(&command),
            "python3 -m http.server 'it'\\''s here' --port=8000"
        );
        assert_eq!(ports_label(&[5173, 8000], ","), Some(":5173,:8000".into()));
        assert_eq!(ports_label(&[], ","), None);
    }
}
