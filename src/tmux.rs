use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

/// Field separator for `list-panes -F` output. Pane titles never contain it.
const SEP: char = '\x1f';

pub fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to execute tmux")?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn run_with_stdin(args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new("tmux")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to execute tmux")?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn server_running() -> bool {
    Command::new("tmux")
        .args(["list-sessions"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub id: String,
    pub pid: i32,
    pub session: String,
    pub window: String,
    pub window_index: u32,
    pub pane_index: u32,
    pub cwd: String,
    pub title: String,
    /// The pane is on screen in an attached session: active pane of the active window.
    pub visible: bool,
    pub name: String,
    pub parent: String,
}

impl Pane {
    pub fn location(&self) -> String {
        format!("{}:{}.{}", self.session, self.window_index, self.pane_index)
    }
}

pub fn list_panes() -> Result<Vec<Pane>> {
    let fields = [
        "#{pane_id}",
        "#{pane_pid}",
        "#{session_name}",
        "#{window_id}",
        "#{window_index}",
        "#{pane_index}",
        "#{pane_current_path}",
        "#{pane_title}",
        "#{&&:#{&&:#{pane_active},#{window_active}},#{session_attached}}",
        "#{@agent-plane-name}",
        "#{@agent-plane-parent}",
    ];
    let format = fields.join(&SEP.to_string());
    let out = run(&["list-panes", "-a", "-F", &format])?;
    out.lines()
        .map(|line| {
            let f: Vec<&str> = line.split(SEP).collect();
            if f.len() != fields.len() {
                bail!("unexpected list-panes line: {line:?}");
            }
            Ok(Pane {
                id: f[0].to_string(),
                pid: f[1].parse().context("pane_pid")?,
                session: f[2].to_string(),
                window: f[3].to_string(),
                window_index: f[4].parse().context("window_index")?,
                pane_index: f[5].parse().context("pane_index")?,
                cwd: f[6].to_string(),
                title: f[7].to_string(),
                visible: f[8] == "1",
                name: f[9].to_string(),
                parent: f[10].to_string(),
            })
        })
        .collect()
}

pub fn clients() -> Result<Vec<String>> {
    Ok(run(&["list-clients", "-F", "#{client_name}"])?
        .lines()
        .map(str::to_string)
        .collect())
}

/// The visible screen of a pane, with or without colour escapes.
pub fn capture(pane: &str, ansi: bool, lines: Option<u32>) -> Result<String> {
    let start = lines.map(|n| format!("-{n}"));
    let mut args = vec!["capture-pane", "-p", "-t", pane];
    if ansi {
        args.push("-e");
    } else {
        args.push("-J");
    }
    if let Some(start) = &start {
        args.extend(["-S", start]);
    }
    run(&args)
}

/// Make the most recently used client show this pane.
pub fn jump(session: &str, window: &str, pane: &str) -> Result<()> {
    run(&["switch-client", "-t", session])?;
    run(&["select-window", "-t", window])?;
    run(&["select-pane", "-t", pane])?;
    Ok(())
}
