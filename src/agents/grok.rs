//! Grok Build's opt-in leader roster, joined to its native active-session registry.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const RPC_ID: &str = "tmux-agent-plane-roster";
const MAX_FRAME: usize = 16 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(2);

pub fn launch(task: Option<&str>) -> Result<Launch> {
    Ok(Launch {
        command: "grok",
        args: task
            .map(|task| vec!["--".to_string(), task.to_string()])
            .unwrap_or_default(),
    })
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let home = grok_home()?;
    let socket = std::env::var_os("GROK_LEADER_SOCKET")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("leader.sock"));
    scan_paths(context, &home, &socket)
}

fn scan_paths(context: &ScanContext<'_>, home: &Path, socket: &Path) -> Result<Vec<Observation>> {
    // Ordinary, in-process Grok is untracked. Leader mode is an explicit opt-in.
    match std::fs::symlink_metadata(socket) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", socket.display())),
    }
    // Do not inspect a registry or contact a leader when this server has no Grok panes.
    if !context.panes.iter().any(|pane| {
        context
            .processes
            .processes
            .values()
            .any(|p| is_grok(&p.executable) && context.processes.is_descendant(p.pid, pane.pid))
    }) {
        return Ok(Vec::new());
    }
    let registry = home.join("active_sessions.json");
    let bytes = match std::fs::read(&registry) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", registry.display())),
    };
    let active = active_sessions(&bytes, context)
        .with_context(|| format!("reading {}", registry.display()))?;
    if active.is_empty() {
        return Ok(Vec::new());
    }
    let roster = request_roster(socket)
        .with_context(|| format!("querying Grok leader {}", socket.display()))?;
    observations(&active, &roster)
}

fn grok_home() -> Result<PathBuf> {
    match std::env::var_os("GROK_HOME").filter(|s| !s.is_empty()) {
        Some(home) => Ok(home.into()),
        None => {
            Ok(PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".grok"))
        }
    }
}

fn is_grok(executable: &str) -> bool {
    matches!(
        executable.rsplit('/').next(),
        Some(
            "grok"
                | "grok-macos-aarch64"
                | "grok-macos-x86_64"
                | "grok-linux-aarch64"
                | "grok-linux-x86_64"
        )
    )
}

#[derive(Deserialize)]
struct ActiveFile {
    pid: i32,
    session_id: String,
    pid_start: u64,
}
struct Active {
    pane_id: String,
    session_id: String,
}

fn active_sessions(bytes: &[u8], context: &ScanContext<'_>) -> Result<Vec<Active>> {
    active_sessions_with_start(bytes, context, process_start)
}

fn active_sessions_with_start(
    bytes: &[u8],
    context: &ScanContext<'_>,
    start: impl Fn(i32) -> Result<u64>,
) -> Result<Vec<Active>> {
    let rows: Vec<Value> =
        serde_json::from_slice(bytes).context("invalid Grok active-session registry")?;
    let mut active = Vec::new();
    let mut identities = HashSet::new();
    for row in rows {
        let pid = row
            .get("pid")
            .and_then(Value::as_i64)
            .and_then(|p| i32::try_from(p).ok())
            .filter(|p| *p > 0)
            .context("invalid Grok active-session PID")?;
        let Some(process) = context.processes.processes.get(&pid) else {
            continue;
        };
        if !is_grok(&process.executable) {
            continue;
        }
        let panes = context
            .panes
            .iter()
            .filter(|pane| context.processes.is_descendant(pid, pane.pid))
            .collect::<Vec<_>>();
        if panes.len() > 1 {
            bail!("ambiguous Grok PID {pid} belongs to multiple panes");
        }
        let Some(pane) = panes.first() else {
            continue;
        };
        let file: ActiveFile =
            serde_json::from_value(row).context("invalid live Grok active session")?;
        if file.pid != pid || file.session_id.is_empty() {
            bail!("invalid live Grok session identity for PID {pid}");
        }
        if start(pid)? != file.pid_start {
            // PID reuse: the published record belongs to a process that has exited.
            continue;
        }
        if !identities.insert(file.session_id.clone()) {
            bail!("duplicate live Grok session {}", file.session_id);
        }
        active.push(Active {
            pane_id: pane.id.clone(),
            session_id: file.session_id,
        });
    }
    Ok(active)
}

#[cfg(target_os = "macos")]
fn process_start(pid: i32) -> Result<u64> {
    // SAFETY: proc_bsdinfo is a kernel output struct; zero is valid and the buffer is sized exactly.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let filled =
        unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
    if filled != size {
        bail!(
            "cannot read live Grok PID {pid} start time: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec)
}

#[cfg(not(target_os = "macos"))]
fn process_start(_pid: i32) -> Result<u64> {
    bail!(
        "Grok leader integration currently requires macOS: Linux pid_start token semantics have not been verified"
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RosterEntry {
    session_id: String,
    title: Option<String>,
    activity: String,
    resident: bool,
    origin: Origin,
}
#[derive(Deserialize)]
struct Origin {
    kind: String,
}
#[derive(Deserialize)]
struct Roster {
    sessions: Vec<Value>,
}

fn observations(active: &[Active], roster: &Roster) -> Result<Vec<Observation>> {
    let mut found = Vec::new();
    let mut panes = HashSet::new();
    let mut sessions = HashSet::new();
    for row in &roster.sessions {
        let Some(id) = row.get("sessionId").and_then(Value::as_str) else {
            bail!("Grok roster entry has no sessionId");
        };
        let Some(session) = active.iter().find(|s| s.session_id == id) else {
            continue;
        };
        let entry: RosterEntry =
            serde_json::from_value(row.clone()).context("invalid live Grok roster entry")?;
        // The leader lists historical sessions too, including sessions in ordinary in-process TUIs.
        // Only its resident, local actors publish live state through this opt-in integration.
        if !entry.resident {
            continue;
        }
        if entry.origin.kind != "local" {
            bail!(
                "Grok session {id} has unsupported origin {:?}",
                entry.origin.kind
            );
        }
        let (state, detail) = match entry.activity.as_str() {
            "working" => (RawState::Working, None),
            "needs_input" => (RawState::Blocked, Some("input needed".to_string())),
            "idle" => (RawState::Idle, None),
            other => bail!("unknown live Grok activity {other:?} for session {id}"),
        };
        if !sessions.insert(entry.session_id.clone()) {
            bail!("duplicate Grok roster session {id}");
        }
        if !panes.insert(session.pane_id.clone()) {
            bail!("multiple live Grok sessions in pane {}", session.pane_id);
        }
        found.push(Observation {
            pane_id: session.pane_id.clone(),
            state,
            detail,
            title: entry.title.unwrap_or_default(),
            session_id: Some(entry.session_id),
            preserve_title: false,
            completed_turns: None,
            completion_epoch: None,
        });
    }
    Ok(found)
}

fn write_message(stream: &mut impl Write, message: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_FRAME {
        bail!("Grok IPC frame is too large");
    }
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}
fn read_message(stream: &mut impl Read) -> Result<Value> {
    let mut header = [0; 4];
    stream
        .read_exact(&mut header)
        .context("reading Grok IPC frame header")?;
    let len = u32::from_be_bytes(header) as usize;
    if len > MAX_FRAME {
        bail!("Grok IPC frame is too large: {len}");
    }
    let mut bytes = vec![0; len];
    stream
        .read_exact(&mut bytes)
        .context("reading Grok IPC frame body")?;
    serde_json::from_slice(&bytes).context("invalid Grok IPC JSON")
}

struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}
impl DeadlineStream {
    fn remaining(&self) -> std::io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Grok roster request exceeded its overall deadline",
                )
            })
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.remaining()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.remaining()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}
fn request_roster(socket: &Path) -> Result<Roster> {
    request_roster_with_timeout(socket, IO_TIMEOUT)
}
fn request_roster_with_timeout(socket: &Path, timeout: Duration) -> Result<Roster> {
    let deadline = Instant::now() + timeout;
    let stream = UnixStream::connect(socket).context("connecting to Grok leader")?;
    let mut stream = DeadlineStream { stream, deadline };
    write_message(
        &mut stream,
        &json!({"type":"register", "client_type":"tmux-agent-plane", "mode":"stdio", "capabilities":{}}),
    )?;
    let registered = read_message(&mut stream)?;
    if registered.get("type").and_then(Value::as_str) != Some("registered")
        || registered.get("ready").and_then(Value::as_bool) != Some(true)
        || registered
            .get("leader_protocol_version")
            .and_then(Value::as_u64)
            != Some(1)
    {
        bail!("Grok leader is not ready or has an unsupported registration: {registered}");
    }
    let request =
        json!({"jsonrpc":"2.0", "id":RPC_ID, "method":"_x.ai/sessions/list", "params":{}});
    write_message(
        &mut stream,
        &json!({"type":"acp", "payload":serde_json::to_string(&request)?}),
    )?;
    for _ in 0..64 {
        let envelope = read_message(&mut stream)?;
        match envelope.get("type").and_then(Value::as_str) {
            Some("acp") => {
                let payload = envelope
                    .get("payload")
                    .and_then(Value::as_str)
                    .context("Grok ACP envelope has no payload")?;
                let payload: Value =
                    serde_json::from_str(payload).context("invalid Grok ACP payload")?;
                if payload.get("id").and_then(Value::as_str) == Some(RPC_ID) {
                    let roster = parse_response(&payload)?;
                    write_message(&mut stream, &json!({"type":"disconnect"}))?;
                    return Ok(roster);
                }
            }
            Some("pong" | "leader_ready") => {}
            _ => bail!("unexpected Grok IPC message: {envelope}"),
        }
    }
    bail!("Grok roster response missing after 64 messages")
}
fn parse_response(payload: &Value) -> Result<Roster> {
    if payload.get("error").is_some_and(|e| !e.is_null())
        || payload
            .pointer("/result/error")
            .is_some_and(|e| !e.is_null())
    {
        bail!("Grok roster request failed: {payload}");
    }
    let result = payload
        .pointer("/result/result")
        .context("Grok roster response has no result")?;
    serde_json::from_value(result.clone()).context("invalid Grok roster response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::{Process, ProcessTable};
    use crate::tmux::Pane;
    use std::collections::HashMap;
    use std::io::Cursor;
    use std::os::unix::net::UnixListener;

    fn pane(id: &str, pid: i32) -> Pane {
        Pane {
            id: id.into(),
            pid,
            session: "test".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 0,
            cwd: "/scratch/shared".into(),
            title: "unrelated title".into(),
            visible: false,
            name: String::new(),
            parent: String::new(),
        }
    }
    fn table() -> ProcessTable {
        ProcessTable {
            processes: [
                (10, 1, "zsh"),
                (11, 10, "grok"),
                (20, 1, "zsh"),
                (21, 20, "grok"),
            ]
            .into_iter()
            .map(|(pid, parent, executable)| {
                (
                    pid,
                    Process {
                        pid,
                        parent,
                        executable: executable.into(),
                        argv: executable.into(),
                    },
                )
            })
            .collect::<HashMap<_, _>>(),
        }
    }
    fn active() -> Vec<Active> {
        vec![Active {
            pane_id: "%1".into(),
            session_id: "grok-live".into(),
        }]
    }
    fn fixture(name: &str) -> Value {
        serde_json::from_str(match name {
            "working" => include_str!("../../tests/fixtures/grok/working.json"),
            "idle" => include_str!("../../tests/fixtures/grok/idle.json"),
            "question" => include_str!("../../tests/fixtures/grok/question.json"),
            "permission" => include_str!("../../tests/fixtures/grok/permission.json"),
            _ => unreachable!(),
        })
        .unwrap()
    }
    #[test]
    fn native_roster_states() {
        for (name, state) in [
            ("working", RawState::Working),
            ("idle", RawState::Idle),
            ("question", RawState::Blocked),
            ("permission", RawState::Blocked),
        ] {
            let observations =
                observations(&active(), &parse_response(&fixture(name)).unwrap()).unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].state, state);
            assert_eq!(observations[0].session_id.as_deref(), Some("grok-live"));
            assert!(observations[0].completed_turns.is_none());
            assert!(observations[0].completion_epoch.is_none());
        }
    }
    #[test]
    fn exact_pid_identity_separates_same_cwd_and_ignores_dead_files() {
        let panes = [pane("%1", 10), pane("%2", 20)];
        let processes = table();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let bytes = br#"[{"pid":11,"pid_start":123,"session_id":"grok-live"},{"pid":21,"pid_start":123,"session_id":"second"},{"pid":999,"session_id":false}]"#;
        let active = active_sessions_with_start(bytes, &context, |_| Ok(123)).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].pane_id, "%1");
        assert_eq!(active[1].pane_id, "%2");
        let mut response = fixture("idle");
        let mut second = response["result"]["result"]["sessions"][0].clone();
        second["sessionId"] = "second".into();
        response["result"]["result"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(second);
        assert_eq!(
            observations(&active, &parse_response(&response).unwrap())
                .unwrap()
                .len(),
            2
        );
    }
    #[test]
    fn malformed_live_registry_and_duplicate_identity_fail() {
        let panes = [pane("%1", 10)];
        let processes = table();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        for bytes in [
            b"{".as_slice(),
            br#"[{"pid":11,"session_id":false}]"#,
            br#"[{"pid":11,"session_id":""}]"#,
            br#"[{"pid":11,"session_id":"s"},{"pid":11,"session_id":"s"}]"#,
        ] {
            assert!(active_sessions_with_start(bytes, &context, |_| Ok(123)).is_err());
        }
    }
    #[test]
    fn nonresident_history_is_untracked_and_unknown_live_state_fails() {
        let mut response = fixture("idle");
        response["result"]["result"]["sessions"][0]["activity"] = "future_state".into();
        assert!(observations(&active(), &parse_response(&response).unwrap()).is_err());
        response["result"]["result"]["sessions"][0]["resident"] = false.into();
        assert!(
            observations(&active(), &parse_response(&response).unwrap())
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn ambiguous_live_sessions_in_one_pane_fail() {
        let mut response = fixture("idle");
        let mut second = response["result"]["result"]["sessions"][0].clone();
        second["sessionId"] = "second".into();
        response["result"]["result"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(second);
        let mut active = active();
        active.push(Active {
            pane_id: "%1".into(),
            session_id: "second".into(),
        });
        assert!(observations(&active, &parse_response(&response).unwrap()).is_err());
    }
    #[test]
    fn malformed_response_and_server_errors_fail() {
        for response in [
            json!({}),
            json!({"error":{"code":-1}}),
            json!({"result":{"error":"unavailable","result":null}}),
            json!({"result":{"result":{"sessions":false}}}),
        ] {
            assert!(parse_response(&response).is_err());
        }
        let mut response = fixture("idle");
        response["result"]["result"]["sessions"][0]["resident"] = "yes".into();
        assert!(observations(&active(), &parse_response(&response).unwrap()).is_err());
    }
    #[test]
    fn ipc_rejects_oversized_truncated_and_invalid_json_frames() {
        for data in [
            ((MAX_FRAME as u32) + 1).to_be_bytes().to_vec(),
            vec![0, 0, 0, 4, b'{'],
            vec![0, 0, 0, 1, b'{'],
        ] {
            assert!(read_message(&mut Cursor::new(data)).is_err());
        }
    }
    #[test]
    fn ipc_is_read_only_and_matches_response_id() {
        let socket =
            std::env::temp_dir().join(format!("grok-plane-test-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(IO_TIMEOUT)).unwrap();
            let register = read_message(&mut stream).unwrap();
            assert_eq!(register["type"], "register");
            write_message(
                &mut stream,
                &json!({"type":"registered","ready":true,"leader_protocol_version":1}),
            )
            .unwrap();
            let request = read_message(&mut stream).unwrap();
            let request: Value =
                serde_json::from_str(request["payload"].as_str().unwrap()).unwrap();
            assert_eq!(request["method"], "_x.ai/sessions/list");
            assert_eq!(request["params"], json!({}));
            write_message(&mut stream,&json!({"type":"acp","payload":"{\"jsonrpc\":\"2.0\",\"method\":\"notification\"}"})).unwrap();
            write_message(
                &mut stream,
                &json!({"type":"acp","payload":serde_json::to_string(&fixture("idle")).unwrap()}),
            )
            .unwrap();
            assert_eq!(read_message(&mut stream).unwrap()["type"], "disconnect");
        });
        assert_eq!(
            observations(&active(), &request_roster(&socket).unwrap()).unwrap()[0].state,
            RawState::Idle
        );
        server.join().unwrap();
        std::fs::remove_file(&socket).unwrap();
        assert!(request_roster(&socket).is_err());
    }
    #[test]
    fn one_deadline_bounds_partial_frames_and_unsolicited_messages() {
        let socket =
            std::env::temp_dir().join(format!("grok-plane-deadline-{}.sock", std::process::id()));
        let listener = UnixListener::bind(&socket).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_message(&mut stream).unwrap();
            write_message(
                &mut stream,
                &json!({"type":"registered","ready":true,"leader_protocol_version":1}),
            )
            .unwrap();
            read_message(&mut stream).unwrap();
            write_message(&mut stream, &json!({"type":"pong"})).unwrap();
            // Drip a frame header slowly enough to keep individual reads alive.
            for byte in [0, 0, 0, 20] {
                std::thread::sleep(Duration::from_millis(50));
                if stream.write_all(&[byte]).is_err() {
                    break;
                }
            }
        });
        let started = Instant::now();
        let error = request_roster_with_timeout(&socket, Duration::from_millis(120))
            .err()
            .unwrap();
        assert!(started.elapsed() < Duration::from_millis(400));
        assert!(
            error
                .chain()
                .filter_map(|e| e.downcast_ref::<std::io::Error>())
                .any(|e| matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                )),
            "{error:#}"
        );
        server.join().unwrap();
        std::fs::remove_file(socket).unwrap();
    }
    #[test]
    fn absent_opt_in_and_absent_grok_do_not_read_malformed_registry() {
        let home = std::env::temp_dir().join(format!("grok-plane-absent-{}", std::process::id()));
        std::fs::create_dir(&home).unwrap();
        std::fs::write(home.join("active_sessions.json"), "broken JSON").unwrap();
        let panes = [pane("%1", 10)];
        let processes = table();
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let socket = home.join("leader.sock");
        assert!(scan_paths(&context, &home, &socket).unwrap().is_empty());
        std::fs::write(&socket, "not a server").unwrap();
        let empty = ProcessTable {
            processes: HashMap::new(),
        };
        let empty_context = ScanContext {
            panes: &panes,
            processes: &empty,
        };
        assert!(
            scan_paths(&empty_context, &home, &socket)
                .unwrap()
                .is_empty()
        );
        assert!(scan_paths(&context, &home, &socket).is_err());
        std::fs::remove_dir_all(home).unwrap();
    }
    #[test]
    fn interactive_prompt_stays_one_argument() {
        assert_eq!(launch(None).unwrap().command, "grok");
        assert!(launch(None).unwrap().args.is_empty());
        assert_eq!(
            launch(Some("quote ' and\nnewline")).unwrap().args,
            ["--", "quote ' and\nnewline"]
        );
    }
    #[cfg(target_os = "macos")]
    #[test]
    fn native_process_start_token_is_checked() {
        let pid = std::process::id() as i32;
        let start = process_start(pid).unwrap();
        let processes = ProcessTable {
            processes: HashMap::from([(
                pid,
                Process {
                    pid,
                    parent: 1,
                    executable: "grok".into(),
                    argv: "grok".into(),
                },
            )]),
        };
        let panes = [pane("%1", pid)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        let bytes =
            serde_json::to_vec(&json!([{"pid":pid,"pid_start":start,"session_id":"s"}])).unwrap();
        assert_eq!(active_sessions(&bytes, &context).unwrap().len(), 1);
        let bytes =
            serde_json::to_vec(&json!([{"pid":pid,"pid_start":start+1,"session_id":"s"}])).unwrap();
        assert!(active_sessions(&bytes, &context).unwrap().is_empty());
    }
}
