//! Codex: ask its own app-server, which renews its own login. Calls only `initialize`,
//! `account/read` and `account/rateLimits/read`; never anything that consumes or resets limits.

use super::fetch::{find_on_path, stop, time};
use super::{Failure, FailureKind, Outcome, Reading, Role, Time, Window};
use chrono::Utc;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

const START_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(8);
const LOGIN_REMEDY: &str = "run `codex login`";

pub fn fetch() -> Outcome {
    let Some(binary) = find_on_path("codex") else {
        return Err(Failure::new(
            FailureKind::NotSetUp,
            "codex not found on the daemon's PATH",
            Some("install Codex"),
        ));
    };
    let mut child = super::fetch::group(&mut Command::new(binary))
        .args(["-s", "read-only", "-a", "never", "app-server"])
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Failure::definitive(format!("starting codex app-server: {e}"), None))?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            // Responses only: a server request has a method, and may reuse one of our ids.
            if let Ok(message) = serde_json::from_str::<Value>(&line)
                && message.get("method").is_none()
                && let Some(id) = message.get("id").and_then(Value::as_u64)
                && sender.send((id, message)).is_err()
            {
                break;
            }
        }
    });
    let mut rpc = Rpc {
        stdin: Some(stdin),
        responses: receiver,
        next: 1,
    };
    let outcome = converse(&mut rpc);
    // Closing stdin asks the server to exit; stop makes sure it does.
    rpc.stdin = None;
    stop(&mut child);
    outcome
}

fn converse(rpc: &mut Rpc) -> Outcome {
    rpc.call(
        "initialize",
        json!({"clientInfo": {"name": "tmux-agent-plane", "version": env!("CARGO_PKG_VERSION")}}),
        START_TIMEOUT,
    )
    .map_err(|e| e.failure("codex app-server did not start", false))?;
    let account = rpc.call("account/read", json!({}), CALL_TIMEOUT).ok();
    if let Some(failure) = signed_out(account.as_ref()) {
        return Err(failure);
    }
    let chatgpt = account
        .as_ref()
        .and_then(|a| a.pointer("/account/type"))
        .and_then(Value::as_str)
        == Some("chatgpt");
    let limits = rpc
        .call("account/rateLimits/read", json!({}), CALL_TIMEOUT)
        .map_err(|e| e.failure("reading rate limits", chatgpt))?;
    parse(account.as_ref(), &limits, Utc::now())
}

enum RpcError {
    TimedOut,
    Closed,
    Error(String),
}

impl RpcError {
    /// An error answer is transient only while the account is known to have plan limits.
    fn failure(self, doing: &str, chatgpt: bool) -> Failure {
        match self {
            RpcError::TimedOut => Failure::transient(format!("{doing}: timed out")),
            RpcError::Closed => Failure::transient(format!("{doing}: codex app-server exited")),
            RpcError::Error(message) if chatgpt => {
                Failure::transient(format!("{doing}: {message}"))
            }
            RpcError::Error(message) => Failure::definitive(format!("{doing}: {message}"), None),
        }
    }
}

struct Rpc {
    stdin: Option<ChildStdin>,
    responses: Receiver<(u64, Value)>,
    next: u64,
}

impl Rpc {
    fn call(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value, RpcError> {
        let id = self.next;
        self.next += 1;
        let stdin = self.stdin.as_mut().ok_or(RpcError::Closed)?;
        let request = json!({"id": id, "method": method, "params": params});
        writeln!(stdin, "{request}")
            .and_then(|()| stdin.flush())
            .map_err(|_| RpcError::Closed)?;
        let deadline = Instant::now() + timeout;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.responses.recv_timeout(left) {
                Ok((got, message)) if got == id => {
                    if let Some(error) = message.get("error") {
                        let text = error
                            .get("message")
                            .and_then(Value::as_str)
                            .map_or_else(|| error.to_string(), str::to_string);
                        return Err(RpcError::Error(text));
                    }
                    return Ok(message.get("result").cloned().unwrap_or(Value::Null));
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => return Err(RpcError::TimedOut),
                Err(RecvTimeoutError::Disconnected) => return Err(RpcError::Closed),
            }
        }
    }
}

/// `account: null` is signed out; a login other than ChatGPT has no plan limits.
fn signed_out(account: Option<&Value>) -> Option<Failure> {
    let account = account?.as_object()?.get("account")?;
    if account.is_null() {
        return Some(Failure::definitive("not logged in", Some(LOGIN_REMEDY)));
    }
    match account.get("type").and_then(Value::as_str) {
        Some("chatgpt") | None => None,
        Some(other) => Some(Failure::definitive(
            format!("logged in with {other}: no plan limits"),
            Some("log in to Codex with ChatGPT"),
        )),
    }
}

pub fn parse(account: Option<&Value>, limits: &Value, now: Time) -> Outcome {
    let container = ["rateLimits", "rate_limits", "rate_limit"]
        .iter()
        .find_map(|key| limits.get(*key).filter(|v| v.is_object()))
        .unwrap_or(limits);
    let mut windows: Vec<Window> = [
        (["primary_window", "primary"], ("five_hour", "5h")),
        (["secondary_window", "secondary"], ("weekly", "week")),
    ]
    .iter()
    .filter_map(|(keys, fallback)| {
        let raw = keys
            .iter()
            .find_map(|k| container.get(*k).filter(|v| v.is_object()))?;
        window(raw, *fallback, now)
    })
    .collect();
    if windows.is_empty() {
        return Err(Failure::definitive("Codex reported no rate limits", None));
    }
    let mut seen = std::collections::HashMap::<String, u32>::new();
    for window in &mut windows {
        let count = seen.entry(window.id.clone()).or_default();
        *count += 1;
        if *count > 1 {
            window.id = format!("{}_{count}", window.id);
        }
    }
    let plan = [
        account.and_then(|a| a.get("account")),
        account,
        Some(container),
        Some(limits),
    ]
    .into_iter()
    .flatten()
    .find_map(|v| text(v, &["planType", "plan_type"]));
    Ok(Reading { plan, windows })
}

fn number(value: &Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| match value.get(*k)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    })
}

fn text(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| value.get(*k)?.as_str().filter(|s| !s.is_empty()))
        .map(str::to_string)
}

/// 18000 s is the 5h window and 604800 s the week; any other duration is named in hours. Without
/// a duration the window keeps its slot's name and no duration is invented.
fn window(raw: &Value, (fallback_id, fallback_label): (&str, &str), now: Time) -> Option<Window> {
    let used = number(raw, &["used_percent", "usedPercent"])?;
    let seconds = number(raw, &["limit_window_seconds"])
        .or_else(|| number(raw, &["windowDurationMins", "window_minutes"]).map(|m| m * 60.0))
        .filter(|s| *s > 0.0)
        .map(|s| s as u64);
    let resets_at = ["reset_at", "resets_at", "resetsAt"]
        .iter()
        .find_map(|k| raw.get(*k).and_then(time))
        .or_else(|| {
            number(raw, &["reset_after_seconds"])
                .filter(|s| *s >= 0.0)
                .map(|s| now + chrono::Duration::seconds(s as i64))
        });
    let (id, label) = match seconds {
        None => (fallback_id.to_string(), fallback_label.to_string()),
        Some(18_000) => ("five_hour".into(), "5h".into()),
        Some(604_800) => ("weekly".into(), "week".into()),
        Some(other) => {
            let hours = other as f64 / 3600.0;
            let hours = if hours.fract() == 0.0 {
                format!("{hours:.0}h")
            } else {
                format!("{}h", (hours * 100.0).round() / 100.0)
            };
            (format!("window:{hours}"), hours)
        }
    };
    Some(Window {
        id,
        label,
        group: None,
        role: Role::Bound,
        percent_used: Some(used.clamp(0.0, 100.0)),
        resets_at,
        starts_at: None,
        window_seconds: seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::tests::at;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "account" => include_str!("fixtures/codex_account.json"),
            "limits" => include_str!("fixtures/codex_rate_limits.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn app_server_rate_limits_become_5h_and_week() {
        let reading = parse(Some(&fixture("account")), &fixture("limits"), at(0)).unwrap();
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        let w = &reading.windows;
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id.as_str(), w[0].label.as_str()), ("five_hour", "5h"));
        assert_eq!(w[0].percent_used, Some(12.0));
        assert_eq!(w[0].window_seconds, Some(18_000));
        assert_eq!(w[0].resets_at, Some(at(1_762_620_000)));
        assert_eq!((w[1].id.as_str(), w[1].label.as_str()), ("weekly", "week"));
        assert!(w.iter().all(|w| w.role == Role::Bound));
    }

    #[test]
    fn a_pro_plan_reporting_only_its_week() {
        let limits: Value =
            serde_json::from_str(include_str!("fixtures/codex_rate_limits_week_only.json"))
                .unwrap();
        let reading = parse(None, &limits, at(0)).unwrap();
        assert_eq!(reading.plan.as_deref(), Some("pro"));
        assert_eq!(reading.windows.len(), 1);
        assert_eq!(reading.windows[0].id, "weekly");
        assert_eq!(reading.windows[0].window_seconds, Some(604_800));
    }

    #[test]
    fn snake_case_unfamiliar_durations_and_missing_ones() {
        let limits = json!({"rate_limits": {
            "primary": {"used_percent": "50", "limit_window_seconds": 7200, "reset_after_seconds": 60},
            "secondary": {"used_percent": 101}
        }, "plan_type": "plus"});
        let reading = parse(None, &limits, at(1_000)).unwrap();
        assert_eq!(reading.plan.as_deref(), Some("plus"));
        let w = &reading.windows;
        assert_eq!((w[0].id.as_str(), w[0].label.as_str()), ("window:2h", "2h"));
        assert_eq!(w[0].resets_at, Some(at(1_060)));
        assert_eq!((w[1].id.as_str(), w[1].window_seconds), ("weekly", None));
        assert_eq!(w[1].percent_used, Some(100.0));
    }

    #[test]
    fn duplicate_ids_get_suffixes_and_empty_limits_fail() {
        let limits = json!({"rateLimits": {
            "primary": {"usedPercent": 1, "windowDurationMins": 300},
            "secondary": {"usedPercent": 2, "windowDurationMins": 300}
        }});
        let ids: Vec<_> = parse(None, &limits, at(0))
            .unwrap()
            .windows
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(ids, ["five_hour", "five_hour_2"]);
        let failure = parse(None, &json!({"rateLimits": null}), at(0)).unwrap_err();
        assert_eq!(failure.kind, FailureKind::Definitive);
    }

    #[test]
    fn signed_out_and_api_key_accounts() {
        assert_eq!(
            signed_out(Some(&json!({"account": null})))
                .unwrap()
                .remedy
                .as_deref(),
            Some(LOGIN_REMEDY)
        );
        assert!(signed_out(Some(&json!({"account": {"type": "apiKey"}}))).is_some());
        assert!(signed_out(Some(&fixture("account"))).is_none());
        assert!(signed_out(None).is_none());
    }
}
