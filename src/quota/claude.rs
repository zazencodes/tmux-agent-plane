//! Claude: the OAuth usage endpoint, with the token Claude Code stores. On macOS that is the
//! Keychain, read only after the user allows it once; on Linux, the credentials file. Never a
//! refresh: Claude Code rotates its own tokens on use.

use super::http::{Http, Method, Request};
use super::{Failure, FailureKind, Outcome, Reading, Role, Window};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OAUTH_BETA: &str = "oauth-2025-04-20";
/// quota-axi sends Claude Code's user agent to this endpoint; we do the same.
const USER_AGENT: &str = "claude-code/2.1.202";
const SERVICE: &str = "Claude Code-credentials";
const FIVE_HOURS: u64 = 18_000;
const WEEK: u64 = 604_800;
/// A consent dialog waits on a person.
const KEYCHAIN_READ_TIMEOUT: Duration = Duration::from_secs(60);
const KEYCHAIN_PRESENCE_TIMEOUT: Duration = Duration::from_secs(5);
/// `security` exits 44 both for a missing item and for a Keychain it cannot reach.
const SECURITY_NOT_REACHED: i32 = 44;

pub const ALLOW_REMEDY: &str = "allow it from the sidebar quota panel: u, i, then enter";
const RENEW_REMEDY: &str = "open Claude Code to renew it";
const LOGIN_REMEDY: &str = "log in with Claude Code";

pub enum Presence {
    Present,
    NotReached,
    Failed(String),
}

pub enum ReadError {
    NotReached,
    /// Denied, cancelled or timed out: the user did not allow it.
    Refused(String),
}

/// The login store, behind a seam so tests never touch the real Keychain.
pub trait Keychain: Sync {
    fn present(&self, service: &str, account: &str) -> Presence;
    /// The item's value. Shows the macOS consent dialog unless the user chose Always Allow.
    fn read(&self, service: &str, account: &str) -> Result<String, ReadError>;
}

pub struct SecurityCli;

fn security(args: &[&str], timeout: Duration) -> Result<(i32, Vec<u8>), String> {
    let child = super::fetch::group(&mut Command::new("security"))
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("running security: {e}"))?;
    super::fetch::wait_output(child, timeout, 64 * 1024)
        .map(|(code, out)| (code.unwrap_or(-1), out))
}

impl Keychain for SecurityCli {
    fn present(&self, service: &str, account: &str) -> Presence {
        // Without -w nothing secret is read and macOS shows no dialog.
        match security(
            &["find-generic-password", "-a", account, "-s", service],
            KEYCHAIN_PRESENCE_TIMEOUT,
        ) {
            Ok((0, _)) => Presence::Present,
            Ok((SECURITY_NOT_REACHED, _)) => Presence::NotReached,
            Ok((code, _)) => Presence::Failed(format!("security exited {code}")),
            Err(error) => Presence::Failed(error),
        }
    }

    fn read(&self, service: &str, account: &str) -> Result<String, ReadError> {
        match security(
            &["find-generic-password", "-a", account, "-w", "-s", service],
            KEYCHAIN_READ_TIMEOUT,
        ) {
            Ok((0, out)) => String::from_utf8(out)
                .map(|s| s.trim_end().to_string())
                .map_err(|_| ReadError::Refused("Keychain item is not text".into())),
            Ok((SECURITY_NOT_REACHED, _)) => Err(ReadError::NotReached),
            Ok((code, _)) => Err(ReadError::Refused(format!(
                "Keychain access not allowed (security exited {code})"
            ))),
            Err(error) => Err(ReadError::Refused(error)),
        }
    }
}

/// Where Claude Code keeps its login, and whether we may read it.
pub struct Setup {
    pub macos: bool,
    /// `CLAUDE_CONFIG_DIR`, as set.
    pub config_dir: Option<PathBuf>,
    pub home: PathBuf,
    pub user: String,
    /// The non-secret consent marker.
    pub marker: PathBuf,
    /// The user just asked to allow the Keychain read.
    pub allow: bool,
}

impl Setup {
    pub fn from_env(marker: PathBuf, allow: bool) -> Result<Self, Failure> {
        let var = |name| std::env::var_os(name).filter(|v| !v.is_empty());
        Ok(Self {
            macos: cfg!(target_os = "macos"),
            config_dir: var("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            home: var("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| Failure::definitive("HOME is not set", None))?,
            user: std::env::var("USER")
                .ok()
                .filter(|u| !u.is_empty())
                .ok_or_else(|| Failure::definitive("USER is not set", None))?,
            marker,
            allow,
        })
    }

    /// Claude Code names its item after a hash of `CLAUDE_CONFIG_DIR` when that is set.
    pub fn service(&self) -> String {
        match &self.config_dir {
            Some(dir) => {
                let digest = Sha256::digest(dir.as_os_str().as_encoded_bytes());
                let hex: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
                format!("{SERVICE}-{hex}")
            }
            None => SERVICE.to_string(),
        }
    }

    fn credentials_file(&self) -> PathBuf {
        self.config_dir
            .clone()
            .unwrap_or_else(|| self.home.join(".claude"))
            .join(".credentials.json")
    }
}

fn not_reached() -> Failure {
    Failure::definitive(
        "no Claude login in the Keychain, or the Keychain is locked",
        Some(LOGIN_REMEDY),
    )
}

/// The stored login: token and plan. The token is used for one request and then dropped.
struct Login {
    token: String,
    plan: Option<String>,
}

fn parse_login(text: &str) -> Result<Login, Failure> {
    let unreadable = || Failure::definitive("Claude login unreadable", Some(LOGIN_REMEDY));
    let value: Value = serde_json::from_str(text).map_err(|_| unreadable())?;
    let oauth = value.get("claudeAiOauth").ok_or_else(unreadable)?;
    let token = oauth
        .get("accessToken")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .ok_or_else(unreadable)?;
    Ok(Login {
        token: token.to_string(),
        plan: oauth
            .get("subscriptionType")
            .and_then(Value::as_str)
            .filter(|p| !p.is_empty())
            .map(str::to_string),
    })
}

fn login(setup: &Setup, keychain: &dyn Keychain) -> Result<Login, Failure> {
    if !setup.macos {
        let path = setup.credentials_file();
        return match std::fs::read_to_string(&path) {
            Ok(text) => parse_login(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Failure::new(
                FailureKind::NotSetUp,
                "not logged in",
                Some(LOGIN_REMEDY),
            )),
            Err(e) => Err(Failure::definitive(
                format!("reading {}: {e}", path.display()),
                None,
            )),
        };
    }
    let service = setup.service();
    if !setup.allow && !setup.marker.exists() {
        return Err(match keychain.present(&service, &setup.user) {
            Presence::Present => Failure::new(
                FailureKind::NeedsKeychain,
                "needs Keychain access",
                Some(ALLOW_REMEDY),
            ),
            Presence::NotReached => not_reached(),
            Presence::Failed(error) => {
                Failure::definitive(format!("Keychain check failed: {error}"), None)
            }
        });
    }
    match keychain.read(&service, &setup.user) {
        Ok(text) => {
            grant(&setup.marker)?;
            parse_login(&text)
        }
        Err(ReadError::NotReached) => Err(not_reached()),
        Err(ReadError::Refused(error)) => {
            // Stop asking on every refresh until the user allows it again.
            withdraw(&setup.marker)?;
            Err(Failure::new(
                FailureKind::NeedsKeychain,
                error,
                Some(ALLOW_REMEDY),
            ))
        }
    }
}

fn grant(marker: &Path) -> Result<(), Failure> {
    use std::os::unix::fs::OpenOptionsExt;
    if marker.exists() {
        return Ok(());
    }
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(marker)
        .map(drop)
        .map_err(|e| Failure::definitive(format!("writing {}: {e}", marker.display()), None))
}

fn withdraw(marker: &Path) -> Result<(), Failure> {
    match std::fs::remove_file(marker) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Failure::definitive(
            format!("removing {}: {e}", marker.display()),
            None,
        )),
    }
}

pub fn fetch(http: &dyn Http, keychain: &dyn Keychain, setup: &Setup) -> Outcome {
    let login = login(setup, keychain)?;
    let response = http
        .send(Request {
            method: Method::Get,
            url: USAGE_URL,
            headers: vec![
                ("authorization", format!("Bearer {}", login.token)),
                ("anthropic-beta", OAUTH_BETA.into()),
                ("user-agent", USER_AGENT.into()),
                ("accept", "application/json".into()),
                ("content-type", "application/json".into()),
            ],
            body: &[],
            limit: 1024 * 1024,
        })
        .map_err(super::fetch::transport)?;
    match response.status {
        200..=299 => {}
        401 => {
            return Err(Failure::definitive("login expired", Some(RENEW_REMEDY)));
        }
        status => return Err(super::fetch::http_status(status)),
    }
    let windows = serde_json::from_slice(&response.body)
        .ok()
        .map(|body: Value| windows(&body))
        .filter(|w| !w.is_empty())
        .ok_or_else(|| Failure::definitive("Claude returned no usage windows", None))?;
    Ok(Reading {
        plan: login.plan,
        windows,
    })
}

/// `limits[]` when present: it lists every window the account has, model ones included. Else
/// the fixed `five_hour`, `seven_day` and `seven_day_opus` fields.
pub fn windows(body: &Value) -> Vec<Window> {
    let limits: Vec<Window> = body
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(limit)
        .collect();
    if !limits.is_empty() {
        return limits;
    }
    [
        ("five_hour", "session", Role::Bound, FIVE_HOURS),
        ("seven_day", "week", Role::Bound, WEEK),
        ("seven_day_opus", "opus week", Role::Model, WEEK),
    ]
    .into_iter()
    .filter_map(|(id, label, role, seconds)| {
        let raw = body.get(id)?;
        Some(window(
            id,
            label,
            role,
            raw.get("utilization")?.as_f64()?,
            raw.get("resets_at"),
            Some(seconds),
        ))
    })
    .collect()
}

fn limit(entry: &Value) -> Option<Window> {
    let percent = entry.get("percent")?.as_f64()?;
    let resets = entry.get("resets_at");
    let model = entry.pointer("/scope/model");
    if let Some(name) = model
        .and_then(|m| m.get("display_name"))
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    {
        let key = model
            .and_then(|m| m.get("id"))
            .and_then(Value::as_str)
            .map_or_else(|| slug(name), str::to_string);
        return Some(window(
            &format!("model:{key}"),
            &format!("{} week", name.to_lowercase()),
            Role::Model,
            percent,
            resets,
            Some(WEEK),
        ));
    }
    match entry.get("group").and_then(Value::as_str) {
        Some("session") => Some(window(
            "five_hour",
            "session",
            Role::Bound,
            percent,
            resets,
            Some(FIVE_HOURS),
        )),
        Some("weekly") => Some(window(
            "seven_day",
            "week",
            Role::Bound,
            percent,
            resets,
            Some(WEEK),
        )),
        _ => {
            let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("limit");
            Some(window(kind, kind, Role::Unknown, percent, resets, None))
        }
    }
}

fn slug(name: &str) -> String {
    name.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn window(
    id: &str,
    label: &str,
    role: Role,
    used: f64,
    resets: Option<&Value>,
    seconds: Option<u64>,
) -> Window {
    Window {
        id: id.into(),
        label: label.into(),
        group: None,
        role,
        percent_used: Some(used.clamp(0.0, 100.0)),
        resets_at: resets.and_then(super::fetch::time),
        starts_at: None,
        window_seconds: seconds,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::http::Error;
    use crate::quota::http::fake::{Fake, response};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeKeychain {
        present: bool,
        value: Option<&'static str>,
        reads: Mutex<u32>,
    }

    impl Keychain for FakeKeychain {
        fn present(&self, _: &str, _: &str) -> Presence {
            if self.present {
                Presence::Present
            } else {
                Presence::NotReached
            }
        }
        fn read(&self, _: &str, _: &str) -> Result<String, ReadError> {
            *self.reads.lock().unwrap() += 1;
            self.value
                .map(str::to_string)
                .ok_or_else(|| ReadError::Refused("User canceled".into()))
        }
    }

    const LOGIN: &str = r#"{"claudeAiOauth":{"accessToken":"tok-123","refreshToken":"never-read","subscriptionType":"max"}}"#;
    const USAGE: &str = include_str!("fixtures/claude_usage_limits.json");

    fn setup(dir: &Path, allow: bool) -> Setup {
        Setup {
            macos: true,
            config_dir: None,
            home: dir.to_path_buf(),
            user: "alex".into(),
            marker: dir.join("quota-keychain-granted"),
            allow,
        }
    }

    fn temp() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tap-claude-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn without_consent_only_presence_is_checked() {
        let dir = temp();
        let keychain = FakeKeychain {
            present: true,
            value: Some(LOGIN),
            ..Default::default()
        };
        let http = Fake::with(vec![]);
        let failure = fetch(&http, &keychain, &setup(&dir, false)).unwrap_err();
        assert_eq!(failure.kind, FailureKind::NeedsKeychain);
        assert_eq!(*keychain.reads.lock().unwrap(), 0);
        assert!(http.asked.lock().unwrap().is_empty());
        let missing = FakeKeychain::default();
        let failure = fetch(&http, &missing, &setup(&dir, false)).unwrap_err();
        assert_eq!(
            failure.kind,
            FailureKind::Definitive,
            "exit 44 is not 'absent'"
        );
    }

    #[test]
    fn allowing_reads_once_grants_and_sends_the_token_only_to_anthropic() {
        let dir = temp();
        let keychain = FakeKeychain {
            present: true,
            value: Some(LOGIN),
            ..Default::default()
        };
        let http = Fake::with(vec![
            response(200, USAGE.as_bytes()),
            response(200, USAGE.as_bytes()),
        ]);
        let reading = fetch(&http, &keychain, &setup(&dir, true)).unwrap();
        assert_eq!(reading.plan.as_deref(), Some("max"));
        let marker = dir.join("quota-keychain-granted");
        assert!(marker.exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&marker).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&marker).unwrap(), b"");
        // Once granted, later refreshes read without being asked.
        fetch(&http, &keychain, &setup(&dir, false)).unwrap();
        assert_eq!(*keychain.reads.lock().unwrap(), 2);
        for (url, headers) in http.asked.lock().unwrap().iter() {
            assert_eq!(url, USAGE_URL);
            assert!(headers.contains(&("authorization", "Bearer tok-123".into())));
            assert!(headers.contains(&("anthropic-beta", OAUTH_BETA.into())));
            assert!(!headers.iter().any(|(_, v)| v.contains("never-read")));
        }
    }

    #[test]
    fn a_refused_read_withdraws_consent() {
        let dir = temp();
        grant(&dir.join("quota-keychain-granted")).unwrap();
        let keychain = FakeKeychain {
            present: true,
            ..Default::default()
        };
        let failure = fetch(&Fake::with(vec![]), &keychain, &setup(&dir, false)).unwrap_err();
        assert_eq!(failure.kind, FailureKind::NeedsKeychain);
        assert!(!dir.join("quota-keychain-granted").exists());
    }

    #[test]
    fn statuses_map_to_sign_out_or_transient() {
        let dir = temp();
        let keychain = FakeKeychain {
            present: true,
            value: Some(LOGIN),
            ..Default::default()
        };
        let cases = [
            (response(401, b""), FailureKind::Definitive),
            (response(429, b""), FailureKind::Transient),
            (response(503, b""), FailureKind::Transient),
            (response(403, b""), FailureKind::Definitive),
            (response(200, b"{}"), FailureKind::Definitive),
            (
                Err(Error::Transport("timed out".into())),
                FailureKind::Transient,
            ),
        ];
        for (answer, kind) in cases {
            let failure =
                fetch(&Fake::with(vec![answer]), &keychain, &setup(&dir, true)).unwrap_err();
            assert_eq!(failure.kind, kind, "{failure:?}");
        }
        let expired = fetch(
            &Fake::with(vec![response(401, b"")]),
            &keychain,
            &setup(&dir, true),
        );
        assert_eq!(expired.unwrap_err().remedy.as_deref(), Some(RENEW_REMEDY));
    }

    #[test]
    fn linux_reads_the_credentials_file() {
        let dir = temp();
        let mut linux = setup(&dir, false);
        linux.macos = false;
        let keychain = FakeKeychain::default();
        let failure = fetch(&Fake::with(vec![]), &keychain, &linux).unwrap_err();
        assert_eq!(failure.kind, FailureKind::NotSetUp);
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(".claude/.credentials.json"), LOGIN).unwrap();
        let reading = fetch(
            &Fake::with(vec![response(200, USAGE.as_bytes())]),
            &keychain,
            &linux,
        )
        .unwrap();
        assert_eq!(reading.plan.as_deref(), Some("max"));
        assert_eq!(*keychain.reads.lock().unwrap(), 0);
    }

    #[test]
    fn service_name_hashes_the_config_dir() {
        let dir = temp();
        let mut s = setup(&dir, false);
        assert_eq!(s.service(), "Claude Code-credentials");
        s.config_dir = Some("/Users/alex/.claude-work".into());
        let service = s.service();
        assert!(service.starts_with("Claude Code-credentials-"));
        assert_eq!(service.len(), "Claude Code-credentials-".len() + 8);
    }

    #[test]
    fn limits_array_maps_groups_and_models() {
        let body: Value = serde_json::from_str(USAGE).unwrap();
        let w = windows(&body);
        let ids: Vec<_> = w.iter().map(|w| (w.id.as_str(), w.role)).collect();
        assert_eq!(
            ids,
            [
                ("five_hour", Role::Bound),
                ("seven_day", Role::Bound),
                ("model:claude-opus-4", Role::Model),
                ("model:fable", Role::Model),
                ("mystery", Role::Unknown),
            ]
        );
        assert_eq!(w[0].percent_used, Some(3.0));
        assert_eq!(w[0].window_seconds, Some(18_000));
        assert_eq!(
            w[0].resets_at.unwrap().to_rfc3339(),
            "2026-11-08T18:40:00.512+00:00"
        );
        assert_eq!(w[1].resets_at, None);
        assert_eq!(w[3].label, "fable week");
    }

    #[test]
    fn fixed_fields_are_used_without_limits() {
        let body: Value =
            serde_json::from_str(include_str!("fixtures/claude_usage_fixed.json")).unwrap();
        let w = windows(&body);
        let ids: Vec<_> = w.iter().map(|w| (w.id.as_str(), w.percent_used)).collect();
        assert_eq!(
            ids,
            [
                ("five_hour", Some(28.0)),
                ("seven_day", Some(61.0)),
                ("seven_day_opus", Some(100.0)),
            ]
        );
        assert_eq!(w[2].role, Role::Model);
    }
}
