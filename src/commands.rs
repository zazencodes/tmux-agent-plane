//! The commands agents (and people) use to see and steer other agents.

use crate::index::{self, Agent, Index, Kind, State};
use crate::tmux;
use anyhow::{Context, Result, bail};
use std::borrow::Cow;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(300);
/// How long a new agent gets to show up in the index after its shell is launched.
const SPAWN_DETECT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Placement {
    /// Split the caller's pane.
    Pane,
    /// A new window (in the caller's session when its pane is known).
    Window,
    /// A new detached session.
    Session,
}

fn codex_caller() -> bool {
    std::env::var_os("CODEX_THREAD_ID").is_some() || std::env::var_os("CODEX_SESSION_ID").is_some()
}

fn resolve_caller(codex: bool, pane: Option<String>) -> Result<Option<String>> {
    if codex {
        bail!(
            "cannot identify the Codex caller: CODEX_THREAD_ID/CODEX_SESSION_ID have no reliable published association to a tmux pane; Codex tool shells may inherit a stale TMUX_PANE. Use explicit agent names for targets. Self references require a caller whose pane can be identified"
        );
    }
    Ok(pane)
}

fn self_pane() -> Result<Option<String>> {
    resolve_caller(codex_caller(), std::env::var("TMUX_PANE").ok())
}

/// Resolve an explicit self reference, rejecting Codex's inherited pane value as well.
pub fn target_pane(target: &str) -> Result<Cow<'_, str>> {
    let inherited = std::env::var("TMUX_PANE").ok();
    if target == "self" || (codex_caller() && inherited.as_deref() == Some(target)) {
        let pane =
            self_pane()?.ok_or_else(|| anyhow::anyhow!("self requires a caller inside tmux"))?;
        return Ok(Cow::Owned(pane));
    }
    Ok(Cow::Borrowed(target))
}

fn find_agent<'a>(index: &'a Index, target: &str) -> Result<&'a Agent> {
    index::find(index, &target_pane(target)?)
}

fn display_parent(index: &Index, parent: &Option<String>) -> String {
    match parent {
        None => "-".to_string(),
        Some(pane) => index
            .agents
            .iter()
            .find(|a| &a.pane == pane)
            .map_or_else(|| pane.clone(), |a| a.name.clone()),
    }
}

fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path.to_string(),
    }
}

fn state_label(agent: &Agent) -> String {
    match (&agent.state, &agent.detail) {
        (State::Blocked, Some(detail)) => format!("blocked ({detail})"),
        (state, _) => state.as_str().to_string(),
    }
}

pub fn list(json: bool) -> Result<()> {
    let index = index::read()?;
    let me = if codex_caller() { None } else { self_pane()? };
    if json {
        let agents: Vec<serde_json::Value> = index
            .agents
            .iter()
            .map(|a| {
                let mut v = serde_json::to_value(a).expect("agent serializes");
                v["self"] = (me.as_deref() == Some(a.pane.as_str())).into();
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&agents)?);
        return Ok(());
    }
    if index.agents.is_empty() {
        println!("no agents running");
        return Ok(());
    }
    let rows: Vec<[String; 7]> = index
        .agents
        .iter()
        .map(|a| {
            let mark = if me.as_deref() == Some(a.pane.as_str()) {
                "*"
            } else {
                " "
            };
            [
                format!("{mark}{}", a.name),
                a.kind.as_str().to_string(),
                state_label(a),
                a.location.clone(),
                display_parent(&index, &a.parent),
                home_relative(&a.cwd),
                a.title.clone(),
            ]
        })
        .collect();
    let header = [" NAME", "KIND", "STATE", "WHERE", "PARENT", "CWD", "TASK"].map(String::from);
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
            .enumerate()
            .map(|(i, (cell, w))| {
                if i == 6 {
                    cell.clone()
                } else {
                    format!("{cell:<w$}")
                }
            })
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
    Ok(())
}

/// An agent's pane, or any pane by id: an agent that has not started yet (a trust prompt,
/// say) is not in the index but can still be read and answered.
fn pane_of(target: &str) -> Result<String> {
    let resolved = target_pane(target)?;
    let target = resolved.as_ref();
    let index = index::read()?;
    match find_agent(&index, target) {
        Ok(agent) => Ok(agent.pane.clone()),
        Err(_) if target.starts_with('%') => {
            Ok(
                tmux::run(&["display-message", "-p", "-t", target, "#{pane_id}"])?
                    .trim()
                    .to_string(),
            )
        }
        Err(e) => Err(e),
    }
}

pub fn read(target: &str, lines: u32) -> Result<()> {
    let screen = tmux::capture(&pane_of(target)?, false, Some(lines))?;
    println!("{}", screen.trim_end());
    Ok(())
}

pub fn send(
    target: &str,
    text: &str,
    answer: bool,
    wait: bool,
    timeout: Option<u64>,
) -> Result<()> {
    let index = index::read()?;
    let agent = find_agent(&index, target)?.clone();
    // Validate before loading a buffer or sending any input to the pane.
    match (agent.state, answer) {
        (State::Blocked, false) => bail!(
            "{} ({}) is waiting on a question or permission. Read it with `tmux-agent-plane read {}`; then answer with `tmux-agent-plane keys {} ...` or deliberately `tmux-agent-plane send {} --answer <text>`. Status messages must wait",
            agent.name,
            agent.pane,
            agent.name,
            agent.name,
            agent.name
        ),
        (State::Blocked, true) | (_, false) => {}
        (_, true) => bail!(
            "--answer requires a blocked target; {} ({}) is {}. Read it before deciding what to send",
            agent.name,
            agent.pane,
            agent.state.as_str()
        ),
    }
    let before = agent.completions;
    let sent_at = index::now_ms();
    paste(&agent.pane, text)?;
    if wait {
        let done = wait_for(&agent.pane, timeout, |a| {
            a.completions > before
                || (a.state == State::Blocked
                    && a.state_since >= sent_at
                    && (agent.state != State::Blocked || a.state_since > agent.state_since))
        })?;
        print_settled(&done);
    }
    Ok(())
}

/// Bracketed paste, then Enter, so multi-line prompts arrive as one message.
fn paste(pane: &str, text: &str) -> Result<()> {
    let buffer = format!("tmux-agent-plane-{}", std::process::id());
    tmux::run_with_stdin(&["load-buffer", "-b", &buffer, "-"], text)?;
    tmux::run(&["paste-buffer", "-p", "-d", "-b", &buffer, "-t", pane])?;
    // Agents read a paste and an Enter that arrive together as one paste.
    std::thread::sleep(Duration::from_millis(300));
    tmux::run(&["send-keys", "-t", pane, "Enter"])?;
    Ok(())
}

/// Names are one word, unique, and never look like a pane id.
fn check_name(index: &Index, name: &str, pane: Option<&str>) -> Result<()> {
    if name.is_empty() || name.chars().any(char::is_whitespace) || name.starts_with('%') {
        bail!("agent names are one word, without spaces, not starting with %: {name:?}");
    }
    if let Some(other) = index
        .agents
        .iter()
        .find(|a| a.name == name && Some(a.pane.as_str()) != pane)
    {
        bail!(
            "{name:?} is already the name of the agent in {}",
            other.pane
        );
    }
    Ok(())
}

/// Name an agent. The name lives in its pane, so it lasts as long as the pane.
pub fn set_name(target: &str, name: &str) -> Result<Agent> {
    let index = index::read()?;
    let agent = find_agent(&index, target)?.clone();
    check_name(&index, name, Some(&agent.pane))?;
    tmux::run(&[
        "set-option",
        "-p",
        "-t",
        &agent.pane,
        "@agent-plane-name",
        name,
    ])?;
    Ok(agent)
}

pub fn rename(target: &str, name: &str) -> Result<()> {
    let agent = set_name(target, name)?;
    println!("{} ({}) is now {name}", agent.name, agent.pane);
    Ok(())
}

pub fn keys(target: &str, keys: &[String]) -> Result<()> {
    let pane = pane_of(target)?;
    let mut args = vec!["send-keys", "-t", pane.as_str()];
    args.extend(keys.iter().map(String::as_str));
    tmux::run(&args)?;
    Ok(())
}

/// Show an agent on the user's screen. Viewing it marks a `done` agent seen.
pub fn focus(target: &str) -> Result<()> {
    let index = index::read()?;
    let agent = find_agent(&index, target)?;
    tmux::jump(&agent.session, &agent.window, &agent.pane)
}

/// Close an agent's pane, ending the agent and its shell.
pub fn kill(target: &str) -> Result<()> {
    let index = index::read()?;
    let agent = find_agent(&index, target)?;
    tmux::run(&["kill-pane", "-t", &agent.pane])?;
    println!("killed {} ({})", agent.name, agent.pane);
    Ok(())
}

pub fn wait(target: &str, timeout: Option<u64>) -> Result<()> {
    let index = index::read()?;
    let pane = find_agent(&index, target)?.pane.clone();
    let done = wait_for(&pane, timeout, |a| a.state != State::Working)?;
    print_settled(&done);
    Ok(())
}

fn print_settled(agent: &Agent) {
    println!("{} {}: {}", agent.name, state_label(agent), agent.title);
}

fn wait_for(pane: &str, timeout: Option<u64>, settled: impl Fn(&Agent) -> bool) -> Result<Agent> {
    let deadline = timeout.map(|s| Instant::now() + Duration::from_secs(s));
    loop {
        let index = index::read()?;
        let Some(agent) = index.agents.iter().find(|a| a.pane == pane) else {
            bail!("agent in pane {pane} exited");
        };
        if settled(agent) {
            return Ok(agent.clone());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            bail!(
                "timed out after {}s; {} is {}",
                timeout.expect("deadline implies timeout"),
                agent.name,
                state_label(agent)
            );
        }
        std::thread::sleep(POLL);
    }
}

pub struct Spawn {
    pub kind: Kind,
    pub placement: Placement,
    pub cwd: Option<PathBuf>,
    pub name: Option<String>,
    pub task: Option<String>,
    pub wait: bool,
    pub timeout: Option<u64>,
}

pub fn spawn(s: Spawn) -> Result<()> {
    // Validate the interactive launch before creating a pane or contacting tmux.
    let launch = (s.kind.adapter().launch)(s.task.as_deref())?;
    let index = index::read()?;
    if let Some(name) = &s.name {
        check_name(&index, name, None)?;
    }
    let cwd = match s.cwd {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };
    let cwd = cwd.canonicalize()?;
    let cwd = cwd.to_str().expect("cwd is UTF-8");
    let me = if codex_caller() {
        if matches!(s.placement, Placement::Pane) {
            bail!(
                "cannot identify the Codex caller's pane for --in pane; use --in window or --in session"
            );
        }
        None
    } else {
        self_pane()?
    };

    // Pass the command as argv to an interactive login shell. Shell startup finishes
    // before -c is evaluated; no terminal input can race startup or hit its line limit.
    let shell = tmux::default_shell(me.as_deref())?;
    let command = shell_launch(&shell, launch.command, &launch.args);
    let shell_args = [shell.as_str(), "-ilc", command.as_str()];
    let pane = match (s.placement, &me) {
        (Placement::Pane, None) => {
            bail!("--in pane splits the caller's pane, so it must run inside tmux")
        }
        (Placement::Pane, Some(me)) => tmux::run(&[
            "split-window",
            "-d",
            "-h",
            "-P",
            "-F",
            "#{pane_id}",
            "-c",
            cwd,
            "-t",
            me,
            shell_args[0],
            shell_args[1],
            shell_args[2],
        ])?,
        (Placement::Window, _) => {
            // `session_id:` puts the window at the next free index of the caller's session.
            let session = match &me {
                Some(me) => Some(format!(
                    "{}:",
                    tmux::run(&["display-message", "-p", "-t", me, "#{session_id}"])?.trim()
                )),
                None => None,
            };
            let mut args = vec!["new-window", "-d", "-P", "-F", "#{pane_id}", "-c", cwd];
            if let Some(session) = &session {
                args.extend(["-t", session]);
            }
            if let Some(name) = &s.name {
                args.extend(["-n", name]);
            }
            args.extend(shell_args);
            tmux::run(&args)?
        }
        (Placement::Session, _) => {
            let mut args = vec!["new-session", "-d", "-P", "-F", "#{pane_id}", "-c", cwd];
            if let Some(name) = &s.name {
                args.extend(["-s", name]);
            }
            args.extend(shell_args);
            tmux::run(&args)?
        }
    };
    let pane = pane.trim().to_string();

    let name = s
        .name
        .unwrap_or_else(|| format!("{}-{}", s.kind.as_str(), pane.trim_start_matches('%')));
    tmux::run(&["set-option", "-p", "-t", &pane, "@agent-plane-name", &name])?;
    if let Some(me) = &me {
        tmux::run(&["set-option", "-p", "-t", &pane, "@agent-plane-parent", me])?;
    }

    let started = Instant::now();
    let agent = loop {
        // The shell can launch before pane metadata is set. Wait for the daemon
        // to publish the assigned identity, not its earlier default-name snapshot.
        if let Some(agent) = index::read()?.agents.into_iter().find(|a| {
            a.pane == pane
                && a.name == name
                && me
                    .as_ref()
                    .is_none_or(|parent| a.parent.as_ref() == Some(parent))
        }) {
            break agent;
        }
        let dead = tmux::run(&["display-message", "-p", "-t", &pane, "#{pane_dead}"])
            .with_context(|| {
                format!("spawn shell or agent exited in pane {pane} before detection")
            })?;
        if dead.trim() == "1" {
            bail!("spawn shell or agent exited in pane {pane} before detection");
        }
        if started.elapsed() > SPAWN_DETECT_TIMEOUT {
            bail!(
                "{} did not start in pane {pane} within {}s. It may be showing a prompt such as \
                 folder trust: see it with `tmux-agent-plane read {pane}` and answer with \
                 `tmux-agent-plane keys {pane} ...`",
                s.kind.as_str(),
                SPAWN_DETECT_TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL);
    };
    println!("{} {} {}", agent.name, agent.pane, agent.location);

    if s.wait {
        let has_task = s.task.is_some();
        let done = wait_for(&pane, s.timeout, |a| {
            if has_task {
                a.completions >= 1 || a.state == State::Blocked
            } else {
                a.state != State::Working
            }
        })?;
        print_settled(&done);
    }
    Ok(())
}

/// Keep the command name literal so interactive aliases/functions apply. Arguments
/// are shell data, and after the agent returns a fresh login shell owns the terminal.
fn shell_launch(shell: &str, command: &str, args: &[String]) -> String {
    let mut script = command.to_owned();
    for arg in args {
        script.push(' ');
        script.push_str(&shell_quote(arg));
    }
    script.push_str("\nexec ");
    script.push_str(&shell_quote(shell));
    script.push_str(" -il");
    script
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launch_arguments_round_trip_through_shell() {
        // Exercise the generated shell program, including newlines, command
        // substitution syntax, empty arguments and a task beyond terminal line limits.
        let args = vec![
            String::new(),
            "quotes '\" $HOME $(exit 37) `exit 38`; | & <> \\ \t\nnext line\n".into(),
            "é漢🙂".repeat(3000),
        ];
        let script = shell_launch("/usr/bin/true", "printf '%s\\0'", &args);
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success());
        let expected: Vec<u8> = args
            .iter()
            .flat_map(|arg| arg.bytes().chain(std::iter::once(0)))
            .collect();
        assert_eq!(output.stdout, expected);
    }

    #[test]
    fn codex_never_uses_inherited_tmux_pane() {
        assert!(resolve_caller(true, Some("%14".into())).is_err());
        assert!(resolve_caller(true, None).is_err());
        assert_eq!(
            resolve_caller(false, Some("%7".into())).unwrap(),
            Some("%7".into())
        );
        assert_eq!(resolve_caller(false, None).unwrap(), None);
    }
}
