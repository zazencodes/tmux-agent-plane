//! The agent index: one JSON file, written by the daemon every tick and read by everything else.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// The daemon rewrites the index every second; older than this means it is not running.
const STALE_AFTER_MS: u64 = 5_000;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Claude,
    Codex,
}

impl Kind {
    pub fn adapter(self) -> crate::agents::Adapter {
        use crate::agents::{Adapter, claude, codex};
        match self {
            Self::Claude => Adapter {
                scan: claude::scan,
                launch: claude::launch,
            },
            Self::Codex => Adapter {
                scan: codex::scan,
                launch: codex::launch,
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Running a turn.
    Working,
    /// Waiting on a person: a permission prompt, a question, a dialog.
    Blocked,
    /// Finished a turn that nobody has looked at yet.
    Done,
    /// Ready for input, and already seen.
    Idle,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Working => "working",
            State::Blocked => "blocked",
            State::Done => "done",
            State::Idle => "idle",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Agent {
    pub name: String,
    /// tmux pane id, e.g. `%154`. Stable for the life of the pane.
    pub pane: String,
    pub kind: Kind,
    pub state: State,
    /// What a blocked agent is waiting for, when the agent says.
    pub detail: Option<String>,
    /// The task, taken from the pane title the agent sets.
    pub title: String,
    /// `session:window.pane`
    pub location: String,
    pub session: String,
    /// tmux window id, e.g. `@104`.
    pub window: String,
    pub cwd: String,
    /// Pane id of the agent that spawned this one.
    pub parent: Option<String>,
    /// Session identifier published by the agent, when available.
    pub session_id: Option<String>,
    /// Unix ms when `state` last changed.
    pub state_since: u64,
    /// Number of finished turns seen by the daemon.
    pub completions: u64,
    /// The pane is on screen right now.
    pub visible: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub updated_at: u64,
    pub agents: Vec<Agent>,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_millis() as u64
}

pub fn state_dir() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_STATE_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => {
            PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/state")
        }
    };
    Ok(base.join("tmux-agent-plane"))
}

fn index_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("agents.json"))
}

/// The index as last written, however old. `None` before the daemon's first run.
pub fn read_any() -> Result<Option<Index>> {
    let path = index_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// The current index. Fails when the daemon is not keeping it fresh.
pub fn read() -> Result<Index> {
    let not_running = "the tmux-agent-plane daemon is not running. It starts with tmux via \
        the tmux-agent-plane.tmux plugin, or run `tmux-agent-plane daemon` inside tmux";
    let Some(index) = read_any()? else {
        bail!(not_running)
    };
    if now_ms().saturating_sub(index.updated_at) > STALE_AFTER_MS {
        bail!(not_running);
    }
    Ok(index)
}

pub fn write(index: &Index) -> Result<()> {
    let path = index_path()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(index)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Find an agent by name or pane id.
pub fn find<'a>(index: &'a Index, target: &str) -> Result<&'a Agent> {
    if let Some(agent) = index
        .agents
        .iter()
        .find(|a| a.name == target || a.pane == target)
    {
        return Ok(agent);
    }
    let names: Vec<&str> = index.agents.iter().map(|a| a.name.as_str()).collect();
    if names.is_empty() {
        bail!("no agent named {target:?}: no agents are running");
    }
    bail!("no agent named {target:?}. Agents: {}", names.join(", "))
}
