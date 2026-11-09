//! Grok Build: grok.com's credits config, a grpc-web call decoded by hand, with the session token
//! the Grok CLI keeps in auth.json. API keys are never used. Stored expiry only orders tokens;
//! the server decides whether one still works.

use super::fetch::{http_status, transport};
use super::http::{Http, Method, Request};
use super::{Failure, FailureKind, Outcome, Reading, Role, Time, Window};
use chrono::{TimeZone, Utc};
use serde_json::Value;
use std::path::PathBuf;

const URL: &str = "https://grok.com/grok_api_v2.GrokBuildBilling/GetGrokCreditsConfig";
/// An empty grpc-web frame: no flags, zero length.
const EMPTY_REQUEST: [u8; 5] = [0, 0, 0, 0, 0];
const LIMIT: u64 = 64 * 1024;
const PRODUCT_GROK_BUILD: u64 = 2;
const RENEW_REMEDY: &str = "run grok to renew it";

pub fn fetch(http: &dyn Http) -> Outcome {
    let path = auth_path().ok_or_else(|| Failure::definitive("HOME is not set", None))?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Failure::new(
                FailureKind::NotSetUp,
                "not logged in",
                Some("install Grok Build and log in"),
            ));
        }
        Err(e) => {
            return Err(Failure::definitive(
                format!("reading {}: {e}", path.display()),
                None,
            ));
        }
    };
    let auth: Value = serde_json::from_str(&text)
        .map_err(|_| Failure::definitive(format!("{} is not JSON", path.display()), None))?;
    fetch_with(http, &auth, Utc::now())
}

fn auth_path() -> Option<PathBuf> {
    match std::env::var_os("GROK_HOME").filter(|v| !v.is_empty()) {
        Some(home) => Some(PathBuf::from(home).join("auth.json")),
        None => Some(PathBuf::from(std::env::var_os("HOME")?).join(".grok/auth.json")),
    }
}

/// Tries each session token, unexpired first, until one answers. A rejection moves on to the
/// next; anything else stops, so a network fault never reads as a sign-out.
pub fn fetch_with(http: &dyn Http, auth: &Value, now: Time) -> Outcome {
    let tokens = session_tokens(auth, now);
    if tokens.is_empty() {
        return Err(Failure::new(
            FailureKind::NotSetUp,
            "no Grok Build login in auth.json",
            Some("log in with grok"),
        ));
    }
    for token in tokens {
        let response = http
            .send(Request {
                method: Method::Post,
                url: URL,
                headers: vec![
                    ("authorization", format!("Bearer {token}")),
                    ("content-type", "application/grpc-web+proto".into()),
                    ("accept", "*/*".into()),
                    ("x-grpc-web", "1".into()),
                    ("x-user-agent", "connect-es/2.1.1".into()),
                    ("origin", "https://grok.com".into()),
                    ("referer", "https://grok.com/?_s=usage".into()),
                ],
                body: &EMPTY_REQUEST,
                limit: LIMIT,
            })
            .map_err(transport)?;
        match response.status {
            200..=299 => {}
            401 | 403 => continue,
            status => return Err(http_status(status)),
        }
        match grpc_status(
            response.header("grpc-status"),
            response.header("grpc-message"),
        ) {
            Grpc::Ok => {}
            Grpc::Rejected => continue,
            Grpc::Failed(failure) => return Err(failure),
        }
        let message = unframe(&response.body)?;
        return decode(&message);
    }
    Err(Failure::definitive("login expired", Some(RENEW_REMEDY)))
}

/// Session tokens in auth.json: Grok Build's OIDC login on auth.x.ai, an accounts.x.ai sign-in
/// or a grok.com session. Unexpired ones first; expiry is advisory.
fn session_tokens(auth: &Value, now: Time) -> Vec<String> {
    let Some(object) = auth.as_object() else {
        return Vec::new();
    };
    let entries: Vec<(Option<&str>, &Value)> = if object.get("key").is_some() {
        vec![(None, auth)]
    } else {
        object
            .iter()
            .map(|(scope, v)| (Some(scope.as_str()), v))
            .collect()
    };
    let mut tokens: Vec<(bool, String)> = entries
        .into_iter()
        .filter_map(|(scope, item)| {
            let text = |k: &str| {
                item.get(k)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
            };
            let key = text("key")?;
            let scope = text("scope")
                .or(text("url"))
                .or(text("audience"))
                .or(scope)?;
            let kind = text("type").or(text("kind")).map(str::to_lowercase);
            let oidc = text("auth_mode")
                .or(text("authMode"))
                .is_some_and(|m| m.eq_ignore_ascii_case("oidc"));
            if !is_session(scope, kind.as_deref(), oidc) {
                return None;
            }
            let expired = text("expires_at")
                .or(text("expiresAt"))
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_some_and(|t| t <= now);
            Some((expired, key.to_string()))
        })
        .collect();
    tokens.sort_by_key(|(expired, _)| *expired);
    tokens.into_iter().map(|(_, key)| key).collect()
}

fn is_session(scope: &str, kind: Option<&str>, oidc: bool) -> bool {
    let lowered = scope.to_lowercase();
    // `https://auth.x.ai::<id>`: the part after `::` names the account.
    let scope = lowered.split("::").next().unwrap_or_default();
    let rest = scope.split_once("://").map_or(scope, |(_, rest)| rest);
    let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = host.split(':').next().unwrap_or_default();
    let api_key = matches!(kind, Some("api-key" | "api_key"))
        || lowered.contains("api-key")
        || lowered.contains("api_key")
        || matches!(host, "api.x.ai" | "api.grok.com");
    !api_key
        && ((host == "auth.x.ai" && oidc)
            || (host == "accounts.x.ai" && path.starts_with("/sign-in"))
            || matches!(host, "grok.com" | "www.grok.com"))
}

enum Grpc {
    Ok,
    Rejected,
    Failed(Failure),
}

/// 16 unauthenticated, or 7 permission denied with an auth message, is a rejected token;
/// 8 resource exhausted is transient.
fn grpc_status(status: Option<&str>, message: Option<&str>) -> Grpc {
    let Some(status) = status else {
        return Grpc::Ok;
    };
    match status.trim().parse::<u8>() {
        Ok(0) => Grpc::Ok,
        Ok(16) => Grpc::Rejected,
        Ok(7) if message.is_some_and(auth_message) => Grpc::Rejected,
        Ok(8) => Grpc::Failed(Failure::transient("rate limited (grpc 8)")),
        Ok(code) if code <= 16 => {
            Grpc::Failed(Failure::definitive(format!("grpc status {code}"), None))
        }
        _ => Grpc::Failed(protocol()),
    }
}

fn auth_message(message: &str) -> bool {
    let message = message.to_lowercase().replace("%20", " ");
    [
        "unauthenticated",
        "authentication required",
        "sign in required",
        "sign-in required",
        "bad credentials",
        "could not be validated",
        "invalid token",
        "expired token",
        "token expired",
        "token is expired",
        "token is invalid",
        "invalid credentials",
    ]
    .iter()
    .any(|needle| message.contains(needle))
}

fn protocol() -> Failure {
    Failure::definitive("unexpected response from grok.com", None)
}

/// The one data frame of a grpc-web body; a trailer frame's status is checked. A body that is
/// not framed is the message itself.
fn unframe(bytes: &[u8]) -> Result<Vec<u8>, Failure> {
    let first = *bytes.first().ok_or_else(protocol)?;
    if !(first == 0 || first == 1 || first & 0x80 != 0) {
        return Ok(bytes.to_vec());
    }
    let mut data: Option<Vec<u8>> = None;
    let mut trailer = false;
    let mut at = 0;
    while at < bytes.len() {
        let header = bytes.get(at..at + 5).ok_or_else(protocol)?;
        let flags = header[0];
        if flags != 0 && flags != 0x80 {
            return Err(protocol());
        }
        let length = u32::from_be_bytes(header[1..5].try_into().unwrap()) as usize;
        let frame = bytes.get(at + 5..at + 5 + length).ok_or_else(protocol)?;
        if flags == 0x80 {
            if trailer {
                return Err(protocol());
            }
            trailer = true;
            let text = String::from_utf8_lossy(frame);
            let field = |name: &str| {
                text.lines().find_map(|line| {
                    let (k, v) = line.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case(name)
                        .then(|| v.trim().to_string())
                })
            };
            match grpc_status(
                field("grpc-status").as_deref(),
                field("grpc-message").as_deref(),
            ) {
                Grpc::Ok => {}
                Grpc::Rejected => {
                    return Err(Failure::definitive("login expired", Some(RENEW_REMEDY)));
                }
                Grpc::Failed(failure) => return Err(failure),
            }
        } else {
            if trailer || data.is_some() {
                return Err(protocol());
            }
            data = Some(frame.to_vec());
        }
        at += 5 + length;
    }
    data.ok_or_else(protocol)
}

/// One protobuf field: number and value by wire type.
#[derive(Debug)]
enum Field<'a> {
    Varint(u64),
    Fixed32([u8; 4]),
    Fixed64,
    Bytes(&'a [u8]),
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, Failure> {
    let mut value = 0u64;
    for shift in 0..10 {
        let byte = *bytes.get(*at).ok_or_else(protocol)?;
        *at += 1;
        if shift == 9 && byte > 1 {
            return Err(protocol());
        }
        value |= u64::from(byte & 0x7f) << (shift * 7);
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err(protocol())
}

fn scan(bytes: &[u8]) -> Result<Vec<(u64, Field<'_>)>, Failure> {
    let mut fields = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        let key = varint(bytes, &mut at)?;
        let number = key >> 3;
        if number == 0 || number > 0x1fff_ffff {
            return Err(protocol());
        }
        let field = match key & 7 {
            0 => Field::Varint(varint(bytes, &mut at)?),
            1 => {
                bytes.get(at..at + 8).ok_or_else(protocol)?;
                at += 8;
                Field::Fixed64
            }
            5 => {
                let raw = bytes.get(at..at + 4).ok_or_else(protocol)?;
                at += 4;
                Field::Fixed32(raw.try_into().unwrap())
            }
            2 => {
                let length = usize::try_from(varint(bytes, &mut at)?).map_err(|_| protocol())?;
                let end = at.checked_add(length).ok_or_else(protocol)?;
                let raw = bytes.get(at..end).ok_or_else(protocol)?;
                at = end;
                Field::Bytes(raw)
            }
            _ => return Err(protocol()),
        };
        fields.push((number, field));
    }
    Ok(fields)
}

fn message<'a>(fields: &[(u64, Field<'a>)], number: u64) -> Option<&'a [u8]> {
    messages(fields, number).next()
}

fn messages<'a, 'b>(
    fields: &'b [(u64, Field<'a>)],
    number: u64,
) -> impl Iterator<Item = &'a [u8]> + 'b {
    fields.iter().filter_map(move |(n, f)| match f {
        Field::Bytes(b) if *n == number => Some(*b),
        _ => None,
    })
}

fn uint(fields: &[(u64, Field<'_>)], number: u64) -> Option<u64> {
    fields.iter().find_map(|(n, f)| match f {
        Field::Varint(v) if *n == number => Some(*v),
        _ => None,
    })
}

fn float(fields: &[(u64, Field<'_>)], number: u64) -> Result<Option<f64>, Failure> {
    let raw = fields.iter().find_map(|(n, f)| match f {
        Field::Fixed32(raw) if *n == number => Some(*raw),
        _ => None,
    });
    match raw.map(f32::from_le_bytes) {
        Some(v) if !v.is_finite() => Err(protocol()),
        v => Ok(v.map(f64::from)),
    }
}

/// A `google.protobuf.Timestamp` at `number`: its seconds.
fn timestamp(fields: &[(u64, Field<'_>)], number: u64) -> Result<Option<Time>, Failure> {
    let Some(raw) = message(fields, number) else {
        return Ok(None);
    };
    let inner = scan(raw)?;
    Ok(uint(&inner, 1)
        .and_then(|s| i64::try_from(s).ok())
        .and_then(|s| Utc.timestamp_opt(s, 0).single()))
}

/// The credits config (field 1): shared percent used (1), the current period (8: type 1, start
/// 2, reset 3; 2 weekly, 1 monthly) and per-product percent used (7: product 1, percent 2).
/// A missing percent is zero only while the period is valid. Only Grok Build's product is kept.
fn decode(bytes: &[u8]) -> Outcome {
    let response = scan(bytes)?;
    let config = scan(message(&response, 1).ok_or_else(protocol)?)?;
    let period = message(&config, 8).map(scan).transpose()?;
    let kind = period.as_ref().and_then(|p| uint(p, 1));
    let label = match kind {
        Some(2) => "week",
        Some(1) => "month",
        _ => "credits",
    };
    let starts_at = match &period {
        Some(p) => timestamp(p, 2)?,
        None => None,
    };
    let resets_at = match &period {
        Some(p) => timestamp(p, 3)?,
        None => None,
    };
    let valid_period = matches!(kind, Some(1 | 2)) && starts_at.is_some() && resets_at.is_some();
    let window = |id: &str, label: &str, used: f64| Window {
        id: id.into(),
        label: label.into(),
        group: None,
        role: Role::Bound,
        percent_used: Some(used.clamp(0.0, 100.0)),
        resets_at,
        starts_at,
        window_seconds: None,
    };
    let mut windows = Vec::new();
    let shared = float(&config, 1)?;
    if shared.is_some() || valid_period {
        windows.push(window("credits", label, shared.unwrap_or(0.0)));
    }
    for raw in messages(&config, 7) {
        let product = scan(raw)?;
        let used = float(&product, 2)?;
        if uint(&product, 1).unwrap_or(0) != PRODUCT_GROK_BUILD || (used.is_none() && !valid_period)
        {
            continue;
        }
        windows.push(window("product:grok_build", "build", used.unwrap_or(0.0)));
    }
    if windows.is_empty() {
        return Err(protocol());
    }
    Ok(Reading {
        plan: None,
        windows,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::quota::http::fake::{Fake, response};
    use crate::quota::http::{Error, Response};
    use crate::quota::tests::at;
    use serde_json::json;

    fn key(number: u64, wire: u64) -> Vec<u8> {
        encode_varint((number << 3) | wire)
    }

    fn encode_varint(mut v: u64) -> Vec<u8> {
        let mut out = Vec::new();
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return out;
            }
            out.push(byte | 0x80);
        }
    }

    fn bytes(number: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = key(number, 2);
        out.extend(encode_varint(payload.len() as u64));
        out.extend(payload);
        out
    }

    fn varint_field(number: u64, value: u64) -> Vec<u8> {
        let mut out = key(number, 0);
        out.extend(encode_varint(value));
        out
    }

    fn float_field(number: u64, value: f32) -> Vec<u8> {
        let mut out = key(number, 5);
        out.extend(value.to_le_bytes());
        out
    }

    /// A GetGrokCreditsConfig response: weekly period, shared 23.5% used, Grok Build 40%, chat
    /// 90% (dropped), plus an unknown fixed64 field to skip.
    pub fn credits_response() -> Vec<u8> {
        let period = [
            varint_field(1, 2),
            bytes(2, &varint_field(1, 1_762_000_000)),
            bytes(3, &varint_field(1, 1_762_604_800)),
        ]
        .concat();
        let config = [
            float_field(1, 23.5),
            bytes(7, &[varint_field(1, 2), float_field(2, 40.0)].concat()),
            bytes(7, &[varint_field(1, 4), float_field(2, 90.0)].concat()),
            bytes(8, &period),
            [key(9, 1), vec![0; 8]].concat(),
        ]
        .concat();
        bytes(1, &config)
    }

    fn framed(message: &[u8], trailer: &str) -> Vec<u8> {
        let mut out = vec![0];
        out.extend((message.len() as u32).to_be_bytes());
        out.extend(message);
        out.push(0x80);
        out.extend((trailer.len() as u32).to_be_bytes());
        out.extend(trailer.as_bytes());
        out
    }

    #[test]
    fn decodes_the_credits_config() {
        let reading = decode(&credits_response()).unwrap();
        let w = &reading.windows;
        assert_eq!(w.len(), 2);
        assert_eq!((w[0].id.as_str(), w[0].label.as_str()), ("credits", "week"));
        assert_eq!(w[0].percent_used, Some(23.5));
        assert_eq!(w[0].starts_at, Some(at(1_762_000_000)));
        assert_eq!(w[0].resets_at, Some(at(1_762_604_800)));
        assert_eq!(
            (w[1].id.as_str(), w[1].percent_used),
            ("product:grok_build", Some(40.0))
        );
        assert!(w.iter().all(|w| w.window_seconds.is_none()));
    }

    #[test]
    fn missing_percent_is_zero_only_in_a_valid_period() {
        let period = [
            varint_field(1, 1),
            bytes(2, &varint_field(1, 100)),
            bytes(3, &varint_field(1, 200)),
        ]
        .concat();
        let config = [bytes(7, &varint_field(1, 2)), bytes(8, &period)].concat();
        let reading = decode(&bytes(1, &config)).unwrap();
        assert_eq!(reading.windows[0].label, "month");
        assert!(reading.windows.iter().all(|w| w.percent_used == Some(0.0)));
        let no_period = bytes(1, &bytes(7, &varint_field(1, 2)));
        assert!(decode(&no_period).is_err());
    }

    #[test]
    fn malformed_protobuf_fails() {
        assert!(decode(&[0x0a, 0x05, 0x01]).is_err(), "length past the end");
        assert!(decode(&[0x0b]).is_err(), "wire type 3");
        assert!(decode(&[]).is_err(), "no config");
        let nan = bytes(1, &float_field(1, f32::NAN));
        assert!(decode(&nan).is_err());
    }

    #[test]
    fn grpc_web_frames_and_trailers() {
        let message = credits_response();
        assert_eq!(
            unframe(&framed(&message, "grpc-status: 0\r\n")).unwrap(),
            message
        );
        assert_eq!(unframe(&message).unwrap(), message, "unframed body");
        let rejected = unframe(&framed(
            &message,
            "grpc-status: 16\r\ngrpc-message: bad\r\n",
        ));
        assert_eq!(rejected.unwrap_err().remedy.as_deref(), Some(RENEW_REMEDY));
        let two = [
            framed(&message, "grpc-status: 0"),
            framed(&message, "grpc-status: 0"),
        ]
        .concat();
        assert!(unframe(&two).is_err());
        assert!(unframe(&[0, 0, 0, 0, 9, 1]).is_err(), "truncated frame");
    }

    fn auth() -> Value {
        json!({
            "https://auth.x.ai::account-1": {"key": "expired-oidc", "auth_mode": "oidc", "expires_at": "2026-11-03T02:27:17.623142Z", "refresh_token": "never-sent"},
            "https://grok.com": {"key": "live-session", "expires_at": "2027-01-01T00:00:00Z"},
            "https://api.x.ai": {"key": "api-key-1"},
            "https://auth.x.ai::account-2": {"key": "not-oidc"}
        })
    }

    #[test]
    fn session_tokens_skip_api_keys_and_try_unexpired_first() {
        let tokens = session_tokens(&auth(), at(1_794_000_000));
        assert_eq!(tokens, ["live-session", "expired-oidc"]);
        assert!(session_tokens(&json!({"https://api.x.ai": {"key": "k"}}), at(0)).is_empty());
        assert_eq!(
            session_tokens(
                &json!({"key": "direct", "scope": "https://accounts.x.ai/sign-in"}),
                at(0)
            ),
            ["direct"]
        );
    }

    fn ok() -> Result<Response, Error> {
        let mut answer = response(200, &framed(&credits_response(), "grpc-status: 0")).unwrap();
        answer
            .headers
            .push(("content-type".into(), "application/grpc-web+proto".into()));
        Ok(answer)
    }

    #[test]
    fn a_rejected_token_moves_on_and_only_grok_com_gets_tokens() {
        let http = Fake::with(vec![response(401, b""), ok()]);
        let reading = fetch_with(&http, &auth(), at(1_794_000_000)).unwrap();
        assert_eq!(reading.windows.len(), 2);
        let asked = http.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        for (url, headers) in asked.iter() {
            assert_eq!(url, URL);
            assert!(
                !headers
                    .iter()
                    .any(|(_, v)| v.contains("never-sent") || v.contains("api-key"))
            );
        }
        assert!(
            asked[0]
                .1
                .contains(&("authorization", "Bearer live-session".into()))
        );
        assert!(
            asked[1]
                .1
                .contains(&("authorization", "Bearer expired-oidc".into()))
        );
    }

    #[test]
    fn all_rejected_is_a_sign_out_and_a_network_fault_stops() {
        let http = Fake::with(vec![response(403, b""), response(401, b"")]);
        let failure = fetch_with(&http, &auth(), at(1_794_000_000)).unwrap_err();
        assert_eq!(failure.kind, FailureKind::Definitive);
        assert_eq!(failure.remedy.as_deref(), Some(RENEW_REMEDY));
        let http = Fake::with(vec![Err(Error::Transport("timed out".into()))]);
        let failure = fetch_with(&http, &auth(), at(1_794_000_000)).unwrap_err();
        assert_eq!(failure.kind, FailureKind::Transient);
        assert_eq!(http.asked.lock().unwrap().len(), 1);
        let mut header_rejected = response(200, b"").unwrap();
        header_rejected
            .headers
            .push(("grpc-status".into(), "16".into()));
        let http = Fake::with(vec![Ok(header_rejected), ok()]);
        assert!(fetch_with(&http, &auth(), at(1_794_000_000)).is_ok());
    }
}
