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
    Pi,
    Opencode,
    Agy,
    Grok,
}

impl Kind {
    pub fn adapter(self) -> crate::agents::Adapter {
        use crate::agents::{Adapter, agy, claude, codex, grok, opencode, pi};
        match self {
            Self::Claude => Adapter {
                scan: claude::scan,
                launch: claude::launch,
            },
            Self::Codex => Adapter {
                scan: codex::scan,
                launch: codex::launch,
            },
            Self::Pi => Adapter {
                scan: pi::scan,
                launch: pi::launch,
            },
            Self::Opencode => Adapter {
                scan: opencode::scan,
                launch: opencode::launch,
            },
            Self::Agy => Adapter {
                scan: agy::scan,
                launch: agy::launch,
            },
            Self::Grok => Adapter {
                scan: grok::scan,
                launch: grok::launch,
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
            Kind::Pi => "pi",
            Kind::Opencode => "opencode",
            Kind::Agy => "agy",
            Kind::Grok => "grok",
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
    /// Last durable completion counter from the publisher.
    #[serde(default)]
    pub reported_completions: Option<u64>,
    /// Stable publisher lifetime, independent of the selected native session.
    #[serde(default)]
    pub completion_epoch: Option<String>,
    /// The pane is on screen right now.
    pub visible: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Index {
    pub updated_at: u64,
    pub agents: Vec<Agent>,
    /// Latest off-screen transition. Never restored by a restarted daemon.
    #[serde(default)]
    pub recent_event: Option<RecentEvent>,
    /// Every tmux window. Sidebars learn their surroundings here: idle, they run no tmux.
    #[serde(default)]
    pub windows: Vec<WindowView>,
    /// Pane ids in the row order every sidebar shows. Survives a daemon restart.
    #[serde(default)]
    pub order: Vec<String>,
    /// The agent a client most recently viewed as the active pane. Survives a daemon restart.
    #[serde(default)]
    pub last_agent: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WindowView {
    /// tmux window id, e.g. `@104`.
    pub id: String,
    pub active_pane: String,
    /// The window is the current one of an attached session.
    pub viewed: bool,
    /// The window's sidebar pane, while it has one.
    pub sidebar: Option<String>,
}

pub const EVENT_LIFETIME_MS: u64 = 5_000;

/// Only states that warrant a transition notification may appear in the footer.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventState {
    Done,
    Blocked,
}

impl EventState {
    pub fn state(self) -> State {
        match self {
            Self::Done => State::Done,
            Self::Blocked => State::Blocked,
        }
    }
}

/// One bounded notice shared by every sidebar, with an absolute expiry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RecentEvent {
    pub name: String,
    pub state: EventState,
    pub occurred_at: u64,
    pub expires_at: u64,
}

impl RecentEvent {
    pub fn new(agent: &Agent, now: u64) -> Option<Self> {
        let state = match agent.state {
            State::Done => EventState::Done,
            State::Blocked => EventState::Blocked,
            State::Working | State::Idle => return None,
        };
        Some(Self {
            name: agent.name.chars().take(128).collect(),
            state,
            occurred_at: now,
            expires_at: now.saturating_add(EVENT_LIFETIME_MS),
        })
    }

    pub fn is_live(&self, now: u64) -> bool {
        self.occurred_at <= now
            && now < self.expires_at
            && self.expires_at.saturating_sub(self.occurred_at) <= EVENT_LIFETIME_MS
    }
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
    let not_running = format!(
        "the tmux-agent-plane daemon is not running. Run `tmux-agent-plane daemon start` inside tmux; diagnostics: {}",
        state_dir()?.join("daemon.log").display()
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_index_has_no_event_and_invalid_event_states_fail() {
        let old: Index = serde_json::from_str(r#"{"updated_at":1,"agents":[]}"#).unwrap();
        assert!(old.recent_event.is_none());
        assert!(old.windows.is_empty() && old.order.is_empty() && old.last_agent.is_none());
        for state in ["working", "idle", "unknown"] {
            let value = serde_json::json!({
                "updated_at": 1, "agents": [], "recent_event": {
                    "name": "agent", "state": state, "occurred_at": 1, "expires_at": 5001
                }
            });
            assert!(serde_json::from_value::<Index>(value).is_err());
        }
    }

    #[test]
    fn absolute_event_expiry_rejects_future_and_unbounded_lifetimes() {
        let mut event = RecentEvent {
            name: "agent".into(),
            state: EventState::Done,
            occurred_at: 100,
            expires_at: 5100,
        };
        assert!(!event.is_live(99));
        assert!(event.is_live(100));
        assert!(event.is_live(5099));
        assert!(!event.is_live(5100));
        event.expires_at = 5101;
        assert!(!event.is_live(100));
    }
}
