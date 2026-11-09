//! Antigravity: `agy -p /quota --output-format json`, with `open` and `xdg-open` stubbed out so it
//! can never open a browser. Each model group (Gemini, Claude/GPT, ...) is its own budget.

use super::fetch::{WaitError, find_on_path, time, wait_child};
use super::{Failure, FailureKind, Outcome, Reading, Role, Window};
use serde_json::Value;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::process::{Command, Stdio};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(15);
const OUTPUT_LIMIT: u64 = 1024 * 1024;
const FIVE_HOURS: u64 = 18_000;
const WEEK: u64 = 604_800;
const LOGIN_REMEDY: &str = "run agy to check its login";

pub fn fetch() -> Outcome {
    let Some(agy) = find_on_path("agy") else {
        return Err(Failure::new(
            FailureKind::NotSetUp,
            "agy not found on the daemon's PATH",
            Some("install Antigravity"),
        ));
    };
    let guard =
        OpenerGuard::new().map_err(|e| Failure::definitive(format!("preparing agy: {e}"), None))?;
    let path = match std::env::var_os("PATH") {
        Some(path) => {
            let mut dirs = vec![guard.0.clone()];
            dirs.extend(std::env::split_paths(&path));
            std::env::join_paths(dirs)
                .map_err(|e| Failure::definitive(format!("preparing agy: {e}"), None))?
        }
        None => guard.0.clone().into_os_string(),
    };
    let child = super::fetch::group(&mut Command::new(agy))
        .args(["-p", "/quota", "--output-format", "json"])
        .env("PATH", path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Failure::definitive(format!("starting agy: {e}"), None))?;
    let (code, out) = wait_child(child, TIMEOUT, OUTPUT_LIMIT).map_err(|e| match e {
        // A signed-out agy waits a minute for a browser login it can never get.
        WaitError::TimedOut => Failure::new(
            FailureKind::Transient,
            "agy /quota timed out",
            Some(LOGIN_REMEDY),
        ),
        WaitError::Failed(message) => Failure::definitive(format!("agy /quota: {message}"), None),
    })?;
    if code != Some(0) {
        return Err(failed(code, &out));
    }
    parse(&out)
}

/// A failed run prints `{"status": "ERROR", "error": ...}`.
fn failed(code: Option<i32>, stdout: &[u8]) -> Failure {
    let reported = serde_json::from_slice::<Value>(stdout).ok().and_then(|v| {
        (v.get("status").and_then(Value::as_str) == Some("ERROR"))
            .then(|| v.get("error")?.as_str().map(str::to_string))
            .flatten()
    });
    let error = match (reported, code) {
        (Some(error), _) => format!("agy /quota: {error}"),
        (None, Some(code)) => format!("agy /quota failed (exit {code})"),
        (None, None) => "agy /quota was killed".into(),
    };
    Failure::definitive(error, Some(LOGIN_REMEDY))
}

/// A private directory first on agy's PATH, whose `open` and `xdg-open` fail.
struct OpenerGuard(std::path::PathBuf);

impl OpenerGuard {
    fn new() -> std::io::Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "tmux-agent-plane-agy-{}-{}",
            std::process::id(),
            crate::index::now_ms()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let guard = Self(dir);
        for name in ["open", "xdg-open"] {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o700)
                .open(guard.0.join(name))?
                .write_all(b"#!/bin/sh\nexit 1\n")?;
        }
        Ok(guard)
    }
}

impl Drop for OpenerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn parse(stdout: &[u8]) -> Outcome {
    let malformed = || Failure::definitive("agy /quota returned unexpected output", None);
    let root: Value = serde_json::from_slice(stdout).map_err(|_| malformed())?;
    let command = root.get("command").ok_or_else(malformed)?;
    let name = command.get("name").and_then(Value::as_str);
    if !matches!(name, Some("quota" | "/quota" | "usage" | "/usage")) {
        return Err(malformed());
    }
    let data = command.get("data").ok_or_else(malformed)?;
    let payload = data
        .get("response")
        .filter(|v| v.is_object())
        .or_else(|| data.get("summary").filter(|v| v.is_object()))
        .or_else(|| {
            data.get("groups")
                .is_some_and(Value::is_array)
                .then_some(data)
        })
        .ok_or_else(malformed)?;
    let mut windows: Vec<(u8, u8, Window)> = payload
        .get("groups")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|group| {
            let name = text(group, &["displayName", "name"])
                .unwrap_or("Quota")
                .to_string();
            group
                .get("buckets")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(move |bucket| window(&name, bucket))
        })
        .collect();
    windows.sort_by_key(|(group, kind, _)| (*group, *kind));
    if windows.is_empty() {
        return Err(malformed());
    }
    Ok(Reading {
        plan: None,
        windows: windows.into_iter().map(|(_, _, w)| w).collect(),
    })
}

fn text<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|k| value.get(*k)?.as_str().filter(|s| !s.is_empty()))
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// `remainingFraction` directly, or inside a `remaining` oneof.
fn remaining_fraction(bucket: &Value) -> Option<f64> {
    number(bucket.get("remainingFraction"))
        .or_else(|| number(bucket.get("remaining_fraction")))
        .or_else(|| {
            let remaining = bucket.get("remaining")?;
            number(Some(remaining))
                .or_else(|| number(remaining.get("remainingFraction")))
                .or_else(|| number(remaining.get("remaining_fraction")))
                .or_else(|| {
                    matches!(
                        remaining.get("case").and_then(Value::as_str),
                        Some("remainingFraction" | "remaining_fraction")
                    )
                    .then(|| number(remaining.get("value")))
                    .flatten()
                })
        })
}

/// Group: Gemini (Argon apart: its own quota on paid plans), Claude/GPT, or the group's name.
fn group(name: &str, bucket_id: &str) -> (u8, String, String) {
    let both = format!("{name} {bucket_id}").to_lowercase();
    if both.contains("gemini") && both.contains("argon") {
        (2, "gemini_argon".into(), "gemini argon".into())
    } else if both.contains("gemini") {
        (0, "gemini".into(), "gemini".into())
    } else if both.contains("claude") || both.contains("gpt") || both.contains("3p") {
        (1, "claude_gpt".into(), "claude/gpt".into())
    } else {
        let slug: String = name
            .to_lowercase()
            .split(|c: char| !c.is_ascii_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        let slug = if slug.is_empty() {
            "quota".into()
        } else {
            slug
        };
        (2, slug, name.to_lowercase())
    }
}

/// 5h or weekly from the bucket's window, id or name; anything else is a window we do not know.
fn kind(bucket: &Value) -> (u8, &'static str, &'static str, Role, Option<u64>) {
    let names: Vec<String> = ["window", "bucketId", "bucket_id", "displayName", "name"]
        .iter()
        .filter_map(|k| bucket.get(*k)?.as_str())
        .map(str::to_lowercase)
        .collect();
    let strip = |name: &str| {
        let name = [
            "gemini-",
            "gemini_",
            "3p-",
            "3p_",
            "third-party-",
            "third-party_",
        ]
        .iter()
        .find_map(|p| name.strip_prefix(p))
        .unwrap_or(name);
        name.strip_suffix(" limit").unwrap_or(name).to_string()
    };
    if names.iter().any(|n| {
        matches!(
            strip(n).as_str(),
            "5h" | "5-hour" | "five hour" | "five_hour" | "five-hour"
        )
    }) {
        return (0, "5h", "5h", Role::Bound, Some(FIVE_HOURS));
    }
    if names.iter().any(|n| strip(n) == "weekly") {
        return (1, "weekly", "week", Role::Bound, Some(WEEK));
    }
    (2, "unknown", "quota", Role::Unknown, None)
}

fn window(group_name: &str, bucket: &Value) -> Option<(u8, u8, Window)> {
    if bucket.get("disabled").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let bucket_id = text(bucket, &["bucketId", "bucket_id", "id"])?;
    let (group_rank, group_id, group_label) = group(group_name, bucket_id);
    let (kind_rank, kind_id, label, role, seconds) = kind(bucket);
    let resets_at = ["resetTime", "reset_time"]
        .iter()
        .find_map(|k| bucket.get(*k).and_then(time));
    let percent_used = remaining_fraction(bucket).map(|f| (1.0 - f.clamp(0.0, 1.0)) * 100.0);
    if percent_used.is_none() && resets_at.is_none() {
        return None;
    }
    Some((
        group_rank,
        kind_rank,
        Window {
            id: format!("{group_id}_{kind_id}"),
            label: label.into(),
            group: Some(group_label),
            role,
            percent_used,
            resets_at,
            starts_at: None,
            window_seconds: seconds,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_become_sub_cards_sorted_gemini_first() {
        let reading = parse(include_bytes!("fixtures/agy_quota.json")).unwrap();
        let w: Vec<_> = reading
            .windows
            .iter()
            .map(|w| {
                (
                    w.group.as_deref().unwrap(),
                    w.id.as_str(),
                    w.label.as_str(),
                    w.role,
                )
            })
            .collect();
        assert_eq!(
            w,
            [
                ("gemini", "gemini_5h", "5h", Role::Bound),
                ("gemini", "gemini_weekly", "week", Role::Bound),
                ("claude/gpt", "claude_gpt_5h", "5h", Role::Bound),
                ("claude/gpt", "claude_gpt_weekly", "week", Role::Bound),
                (
                    "gemini argon",
                    "gemini_argon_unknown",
                    "quota",
                    Role::Unknown
                ),
            ]
        );
        let used: Vec<_> = reading.windows.iter().map(|w| w.percent_used).collect();
        assert_eq!(used[0], Some(25.0));
        assert_eq!(used[1], Some(100.0), "fraction 0 is exhausted");
        assert_eq!(used[2], None, "no fraction: unknown, not invented");
        assert_eq!(used[3], Some(50.0), "oneof remaining");
        assert_eq!(
            reading.windows[0].resets_at.unwrap().to_rfc3339(),
            "2026-11-08T20:00:00+00:00"
        );
    }

    #[test]
    fn a_failed_run_reports_agys_own_error() {
        let out = br#"{"conversation_id":"","status":"ERROR","response":"","error":"authentication failed or timed out","finished_reason":"error"}"#;
        let failure = failed(Some(1), out);
        assert_eq!(
            failure.error,
            "agy /quota: authentication failed or timed out"
        );
        assert_eq!(failure.kind, FailureKind::Definitive);
        assert_eq!(failed(Some(2), b"").error, "agy /quota failed (exit 2)");
    }

    #[test]
    fn summary_payload_and_rejections() {
        let summary = br#"{"command":{"name":"/usage","data":{"summary":{"groups":[{"name":"Gemini","buckets":[{"bucketId":"gemini-weekly","remainingFraction":0.9}]}]}}}}"#;
        let reading = parse(summary).unwrap();
        assert_eq!(reading.windows[0].id, "gemini_weekly");
        assert!((reading.windows[0].percent_used.unwrap() - 10.0).abs() < 1e-9);
        assert!(parse(b"not json").is_err());
        assert!(parse(br#"{"command":{"name":"help","data":{}}}"#).is_err());
        assert!(parse(br#"{"command":{"name":"quota","data":{"groups":[]}}}"#).is_err());
    }
}
