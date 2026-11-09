//! How much of each AI subscription is left. The daemon fetches every provider on a worker thread
//! (`fetch`) and writes `quota.json`; the sidebar's quota view and the `quota` command read it.
//! Never a credential value: tokens go only to their own provider's endpoint.

mod agy;
mod claude;
mod codex;
pub mod fetch;
mod grok;
mod http;
pub mod pace;
pub mod render;
pub mod view;

use crate::index;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

pub type Time = DateTime<Utc>;

/// The providers in the order every list shows them.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    Claude,
    Codex,
    Agy,
    Grok,
}

impl ProviderId {
    pub const ALL: [ProviderId; 4] = [Self::Claude, Self::Codex, Self::Agy, Self::Grok];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
            Self::Agy => "agy",
            Self::Grok => "grok",
        }
    }
}

/// How a window limits its group.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Bounds everything in the group: the headline is the least left over these.
    Bound,
    /// A further bound on one model only (Claude's per-model weeks). Shown, not in the headline.
    Model,
    /// A window the provider reported that we do not understand. Shown; it makes the group's
    /// headline unknown, because it might bound everything.
    Unknown,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Window {
    pub id: String,
    pub label: String,
    /// Sub-card for providers whose groups are separate budgets (Antigravity).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub role: Role,
    /// 0–100. Unknown when the provider did not say; never invented.
    pub percent_used: Option<f64>,
    pub resets_at: Option<Time>,
    pub starts_at: Option<Time>,
    pub window_seconds: Option<u64>,
}

impl Window {
    pub fn remaining(&self) -> Option<f64> {
        self.percent_used.map(|used| 100.0 - used)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    /// The last fetch failed transiently; windows are from an earlier one and not yet reset.
    Stale,
    Failed,
    NotSetUp,
    /// Claude on macOS: reading its login from the Keychain waits for one consent.
    NeedsKeychain,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Provider {
    pub provider: ProviderId,
    pub status: Status,
    pub plan: Option<String>,
    pub windows: Vec<Window>,
    /// When `windows` were read from the provider.
    pub fetched_at: Option<Time>,
    pub error: Option<String>,
    pub remedy: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Snapshot {
    pub updated_at: Time,
    pub providers: Vec<Provider>,
}

/// What one provider read returned.
#[derive(Debug, PartialEq)]
pub struct Reading {
    pub plan: Option<String>,
    pub windows: Vec<Window>,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum FailureKind {
    /// Timeout, network, 429 or 5xx: earlier windows stay until their reset, marked stale.
    Transient,
    /// Signed out, rejected or unreadable: earlier windows are dropped.
    Definitive,
    NotSetUp,
    NeedsKeychain,
}

#[derive(Debug, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    pub error: String,
    pub remedy: Option<String>,
}

impl Failure {
    pub fn new(kind: FailureKind, error: impl Into<String>, remedy: Option<&str>) -> Self {
        Self {
            kind,
            error: error.into(),
            remedy: remedy.map(str::to_string),
        }
    }
    pub fn transient(error: impl Into<String>) -> Self {
        Self::new(FailureKind::Transient, error, None)
    }
    pub fn definitive(error: impl Into<String>, remedy: Option<&str>) -> Self {
        Self::new(FailureKind::Definitive, error, remedy)
    }
}

pub type Outcome = std::result::Result<Reading, Failure>;

/// The provider card after a read, given the card before it (port of quota-axi's stale rules).
pub fn settle(
    id: ProviderId,
    previous: Option<&Provider>,
    outcome: Outcome,
    now: Time,
) -> Provider {
    let card = |status, failure: Failure| Provider {
        provider: id,
        status,
        plan: None,
        windows: Vec::new(),
        fetched_at: None,
        error: Some(failure.error),
        remedy: failure.remedy,
    };
    match outcome {
        Ok(reading) => Provider {
            provider: id,
            status: Status::Ok,
            plan: reading.plan,
            windows: reading.windows,
            fetched_at: Some(now),
            error: None,
            remedy: None,
        },
        Err(failure) => match failure.kind {
            FailureKind::NotSetUp => card(Status::NotSetUp, failure),
            FailureKind::NeedsKeychain => card(Status::NeedsKeychain, failure),
            FailureKind::Definitive => card(Status::Failed, failure),
            FailureKind::Transient => {
                let kept = previous
                    .map(|p| servable(p, now))
                    .filter(|windows| !windows.is_empty());
                match (previous, kept) {
                    (Some(previous), Some(windows)) => Provider {
                        provider: id,
                        status: Status::Stale,
                        plan: previous.plan.clone(),
                        windows,
                        fetched_at: previous.fetched_at,
                        error: Some(failure.error),
                        remedy: failure.remedy,
                    },
                    _ => card(Status::Failed, failure),
                }
            }
        },
    }
}

/// Earlier windows still true at `now`: those whose reset is still ahead, and resetless ones
/// younger than their own duration. Nothing when the clock has moved backwards since.
pub fn servable(previous: &Provider, now: Time) -> Vec<Window> {
    let Some(fetched_at) = previous.fetched_at.filter(|at| *at <= now) else {
        return Vec::new();
    };
    let age = (now - fetched_at).num_seconds();
    previous
        .windows
        .iter()
        .filter(|w| match w.resets_at {
            Some(reset) => reset > now,
            None => w.window_seconds.is_some_and(|s| s > 0 && age < s as i64),
        })
        .cloned()
        .collect()
}

/// The windows of a card that may be shown at `now`, and whether any had to be dropped: a
/// number is never shown past its reset.
pub fn live(provider: &Provider, now: Time) -> (Vec<&Window>, bool) {
    let shown: Vec<_> = provider
        .windows
        .iter()
        .filter(|w| w.resets_at.is_none_or(|reset| reset > now))
        .collect();
    let dropped = shown.len() != provider.windows.len();
    (shown, dropped)
}

pub fn state_path() -> Result<PathBuf> {
    Ok(index::state_dir()?.join("quota.json"))
}

/// The non-secret marker that Claude's Keychain item may be read.
pub fn keychain_marker() -> Result<PathBuf> {
    Ok(index::state_dir()?.join("quota-keychain-granted"))
}

pub fn read() -> Result<Option<Snapshot>> {
    let path = state_path()?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(
            serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing {}", path.display()))?,
        )),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Written atomically, by the daemon only.
pub fn write(snapshot: &Snapshot) -> Result<()> {
    let path = state_path()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(snapshot)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))
}

/// The longest a refresh may take: providers run in parallel, and a Keychain consent dialog may
/// wait up to a minute.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// `tmux-agent-plane quota [--json]`: always a fresh read through the daemon.
pub fn command(json: bool) -> Result<()> {
    crate::lifecycle::ask("quota-refresh", REQUEST_TIMEOUT)?;
    let snapshot =
        read()?.context("the daemon finished a quota refresh but wrote no quota.json")?;
    let now = Utc::now();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&render::report(&snapshot, now))?
        );
    } else {
        for line in render::text(&snapshot, now, 60) {
            println!("{line}");
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use chrono::TimeZone;

    pub fn at(secs: i64) -> Time {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    pub fn window(id: &str, used: f64, resets: Option<i64>, seconds: Option<u64>) -> Window {
        Window {
            id: id.into(),
            label: id.into(),
            group: None,
            role: Role::Bound,
            percent_used: Some(used),
            resets_at: resets.map(at),
            starts_at: None,
            window_seconds: seconds,
        }
    }

    fn ok(windows: Vec<Window>, fetched: i64) -> Provider {
        Provider {
            provider: ProviderId::Claude,
            status: Status::Ok,
            plan: Some("max".into()),
            windows,
            fetched_at: Some(at(fetched)),
            error: None,
            remedy: None,
        }
    }

    #[test]
    fn transient_failure_keeps_unreset_windows_as_stale() {
        let previous = ok(
            vec![
                window("five_hour", 40.0, Some(1_000), Some(18_000)),
                window("seven_day", 20.0, Some(9_000), Some(604_800)),
                window("resetless_young", 5.0, None, Some(18_000)),
                window("resetless_unbounded", 5.0, None, None),
            ],
            0,
        );
        let card = settle(
            ProviderId::Claude,
            Some(&previous),
            Err(Failure::transient("timed out")),
            at(2_000),
        );
        assert_eq!(card.status, Status::Stale);
        assert_eq!(card.plan.as_deref(), Some("max"));
        assert_eq!(card.fetched_at, Some(at(0)));
        assert_eq!(card.error.as_deref(), Some("timed out"));
        let ids: Vec<_> = card.windows.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["seven_day", "resetless_young"]);
    }

    #[test]
    fn transient_failure_with_nothing_left_fails_and_sign_out_drops_the_snapshot() {
        let previous = ok(vec![window("five_hour", 40.0, Some(1_000), None)], 0);
        let card = settle(
            ProviderId::Claude,
            Some(&previous),
            Err(Failure::transient("HTTP 503")),
            at(1_000),
        );
        assert_eq!(card.status, Status::Failed);
        assert!(card.windows.is_empty());
        let previous = ok(vec![window("seven_day", 40.0, Some(9_000), None)], 0);
        let card = settle(
            ProviderId::Claude,
            Some(&previous),
            Err(Failure::definitive(
                "login expired",
                Some("open Claude Code"),
            )),
            at(1_000),
        );
        assert_eq!(card.status, Status::Failed);
        assert!(card.windows.is_empty());
        assert_eq!(card.remedy.as_deref(), Some("open Claude Code"));
    }

    #[test]
    fn a_clock_that_moved_backwards_serves_nothing_stale() {
        let previous = ok(vec![window("seven_day", 40.0, Some(9_000), None)], 5_000);
        assert!(servable(&previous, at(4_000)).is_empty());
    }

    #[test]
    fn live_never_shows_a_number_past_its_reset() {
        let card = ok(
            vec![
                window("old", 90.0, Some(100), None),
                window("new", 10.0, Some(900), None),
            ],
            0,
        );
        let (shown, dropped) = live(&card, at(500));
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].id, "new");
        assert!(dropped);
        assert!(!live(&card, at(50)).1);
    }
}
