//! The daemon's side: one fetch of every provider at a time, on the thread of the control
//! request that asked for it, so the one-second tick never waits. Requests that arrive during a
//! fetch join it.

use super::http::{self, Client};
use super::{Failure, Outcome, ProviderId, Snapshot, Status, Time, agy, claude, codex, grok};
use crate::diagnostics;
use anyhow::{Result, anyhow};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::io::Read;
use std::path::PathBuf;
use std::process::Child;
use std::sync::{Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

struct Fetches {
    running: bool,
    /// Fetches finished since the daemon started.
    finished: u64,
    /// The last one's result, for everyone who joined it.
    last: std::result::Result<(), String>,
}

static FETCHES: LazyLock<(Mutex<Fetches>, Condvar)> = LazyLock::new(|| {
    (
        Mutex::new(Fetches {
            running: false,
            finished: 0,
            last: Ok(()),
        }),
        Condvar::new(),
    )
});

/// `quota-refresh` (and `quota-allow-keychain` with `allow_keychain`): returns when the fetch
/// it started or joined has written quota.json. An allow request never joins a fetch without
/// consent: it waits for it, then runs its own.
pub fn refresh(allow_keychain: bool) -> Result<()> {
    let (lock, done) = &*FETCHES;
    let mut fetches = lock
        .lock()
        .map_err(|_| anyhow!("quota fetch lock poisoned"))?;
    if fetches.running && !allow_keychain {
        let target = fetches.finished + 1;
        while fetches.finished < target {
            fetches = done
                .wait(fetches)
                .map_err(|_| anyhow!("quota fetch lock poisoned"))?;
        }
        return fetches.last.clone().map_err(|e| anyhow!(e));
    }
    while fetches.running {
        fetches = done
            .wait(fetches)
            .map_err(|_| anyhow!("quota fetch lock poisoned"))?;
    }
    fetches.running = true;
    drop(fetches);
    let result = run(allow_keychain).map_err(|e| format!("{e:#}"));
    let mut fetches = lock
        .lock()
        .map_err(|_| anyhow!("quota fetch lock poisoned"))?;
    fetches.running = false;
    fetches.finished += 1;
    fetches.last = result.clone();
    done.notify_all();
    result.map_err(|e| anyhow!(e))
}

/// Every provider in parallel, settled against the last snapshot and written.
fn run(allow_keychain: bool) -> Result<()> {
    let previous = super::read()?;
    let client = Client::new();
    let marker = super::keychain_marker()?;
    let outcomes: Vec<(ProviderId, Outcome)> = std::thread::scope(|scope| {
        let client = &client;
        let marker = &marker;
        let handles: Vec<_> = ProviderId::ALL
            .into_iter()
            .map(|id| {
                scope.spawn(move || {
                    let outcome = match id {
                        ProviderId::Claude => {
                            claude::Setup::from_env(marker.clone(), allow_keychain).and_then(
                                |setup| claude::fetch(client, &claude::SecurityCli, &setup),
                            )
                        }
                        ProviderId::Codex => codex::fetch(),
                        ProviderId::Agy => agy::fetch(),
                        ProviderId::Grok => grok::fetch(client),
                    };
                    (id, outcome)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("quota provider thread panicked"))
            .collect()
    });
    let now = Utc::now();
    let providers: Vec<_> = outcomes
        .into_iter()
        .map(|(id, outcome)| {
            let before = previous
                .as_ref()
                .and_then(|s| s.providers.iter().find(|p| p.provider == id));
            let card = super::settle(id, before, outcome, now);
            log_change(before, &card)?;
            Ok(card)
        })
        .collect::<Result<_>>()?;
    super::write(&Snapshot {
        updated_at: now,
        providers,
    })
}

/// A provider failure goes to daemon.log once, when it appears or changes, and so does its
/// recovery. It is not a tracking fault: the status bar stays as it is.
fn log_change(before: Option<&super::Provider>, after: &super::Provider) -> Result<()> {
    let name = after.provider.as_str();
    let was = before.and_then(|b| b.error.as_deref());
    match (&after.error, was) {
        (Some(error), was) if was != Some(error.as_str()) => {
            let remedy = after
                .remedy
                .as_deref()
                .map_or(String::new(), |r| format!(" ({r})"));
            diagnostics::record("quota", &format!("{name}: {error}{remedy}"))
        }
        (None, Some(_)) if after.status == Status::Ok => {
            diagnostics::record("quota", &format!("{name}: recovered"))
        }
        _ => Ok(()),
    }
}

/// A request that got no response.
pub fn transport(error: http::Error) -> Failure {
    match error {
        http::Error::Transport(message) => Failure::transient(message),
        http::Error::TooLarge => Failure::definitive("response too large", None),
    }
}

/// A non-success status other than a sign-out: 429 and 5xx pass, the rest are definitive.
pub fn http_status(status: u16) -> Failure {
    match status {
        429 => Failure::transient("rate limited (HTTP 429)"),
        500..=599 => Failure::transient(format!("HTTP {status}")),
        _ => Failure::definitive(format!("unexpected HTTP {status}"), None),
    }
}

/// An epoch-seconds number or an RFC 3339 string.
pub fn time(value: &Value) -> Option<Time> {
    match value {
        Value::Number(n) => Utc.timestamp_opt(n.as_f64()? as i64, 0).single(),
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&Utc)),
        _ => None,
    }
}

/// The first executable `name` on the daemon's PATH.
pub fn find_on_path(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| {
            std::fs::metadata(path)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

#[derive(Debug, PartialEq)]
pub enum WaitError {
    TimedOut,
    Failed(String),
}

impl From<WaitError> for String {
    fn from(error: WaitError) -> String {
        match error {
            WaitError::TimedOut => "timed out".into(),
            WaitError::Failed(message) => message,
        }
    }
}

/// Waits for a child with piped stdout, keeping at most `limit` bytes of it. A child still
/// running at `timeout` is killed. Returns its exit code (None when killed by a signal).
pub fn wait_output(
    child: Child,
    timeout: Duration,
    limit: u64,
) -> std::result::Result<(Option<i32>, Vec<u8>), String> {
    wait_child(child, timeout, limit).map_err(String::from)
}

pub fn wait_child(
    mut child: Child,
    timeout: Duration,
    limit: u64,
) -> std::result::Result<(Option<i32>, Vec<u8>), WaitError> {
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| WaitError::Failed("child has no stdout".into()))?;
    // Shared, so a grandchild that keeps stdout open cannot hold the result back.
    let out = std::sync::Arc::new(Mutex::new((Vec::new(), false)));
    let sink = std::sync::Arc::clone(&out);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let mut out = sink.lock().unwrap();
                    let room = (limit as usize).saturating_sub(out.0.len());
                    out.0.extend_from_slice(&buffer[..n.min(room)]);
                }
            }
        }
        sink.lock().unwrap().1 = true;
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                stop(&mut child);
                return Err(WaitError::TimedOut);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                stop(&mut child);
                return Err(WaitError::Failed(e.to_string()));
            }
        }
    };
    let drained = Instant::now() + Duration::from_millis(500);
    while !out.lock().unwrap().1 && Instant::now() < drained {
        std::thread::sleep(Duration::from_millis(10));
    }
    let out = std::mem::take(&mut out.lock().unwrap().0);
    Ok((status.code(), out))
}

/// SIGTERM to the child's process group (every child here is spawned as its own group
/// leader, see `group`), then SIGKILL after two seconds; always reaped.
pub fn stop(child: &mut Child) {
    if let Ok(Some(_)) = child.try_wait() {
        return;
    }
    unsafe { libc::kill(-(child.id() as i32), libc::SIGTERM) };
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.wait();
}

/// Makes the command its own process group, so `stop` ends whatever it started too.
pub fn group(command: &mut std::process::Command) -> &mut std::process::Command {
    use std::os::unix::process::CommandExt;
    command.process_group(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_epoch_seconds_or_rfc3339() {
        assert_eq!(
            time(&Value::from(1_762_600_000)).unwrap().timestamp(),
            1_762_600_000
        );
        assert_eq!(
            time(&Value::from("2026-11-08T18:40:00Z"))
                .unwrap()
                .to_rfc3339(),
            "2026-11-08T18:40:00+00:00"
        );
        assert_eq!(time(&Value::from("soon")), None);
        assert_eq!(time(&Value::Null), None);
    }

    #[test]
    fn statuses_split_transient_from_definitive() {
        use super::super::FailureKind::*;
        assert_eq!(http_status(429).kind, Transient);
        assert_eq!(http_status(502).kind, Transient);
        assert_eq!(http_status(404).kind, Definitive);
        assert_eq!(
            transport(http::Error::Transport("x".into())).kind,
            Transient
        );
        assert_eq!(transport(http::Error::TooLarge).kind, Definitive);
    }

    #[test]
    fn a_slow_child_is_killed_at_its_timeout() {
        let child = group(&mut std::process::Command::new("sleep"))
            .arg("5")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert_eq!(
            wait_child(child, Duration::from_millis(200), 1024),
            Err(WaitError::TimedOut)
        );
        assert!(started.elapsed() < Duration::from_secs(3));
        let child = std::process::Command::new("printf")
            .arg("hello")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        assert_eq!(
            wait_child(child, Duration::from_secs(5), 3),
            Ok((Some(0), b"hel".to_vec()))
        );
    }
}
