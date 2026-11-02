//! Antigravity CLI's live presence descriptors and native SQLite state.
use super::{Launch, Observation, RawState, ScanContext};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn launch(task: Option<&str>) -> Result<Launch> {
    let task = task
        .filter(|task| !task.trim().is_empty())
        .context("agy publishes no state until its first turn; give spawn a task")?;
    Ok(Launch {
        command: "agy",
        args: vec!["--prompt-interactive".into(), task.into()],
    })
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let candidates: Vec<_> = context
        .panes
        .iter()
        .flat_map(|pane| {
            context
                .processes
                .descendants_named(pane.pid, "agy")
                .into_iter()
                .map(move |process| (pane, process))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(Vec::new());
    }
    let mut interactive = Vec::new();
    for (pane, process) in candidates {
        if let Some(argv) = process_argv(process.pid)?
            && interactive_argv(&argv)?
        {
            interactive.push((pane, process));
        }
    }
    if interactive.is_empty() {
        return Ok(Vec::new());
    }
    let data = PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
        .join(".gemini/antigravity-cli");
    let presence = data.join("presence");
    let summaries_path = data.join("conversation_summaries.db");
    match presence.try_exists().and_then(|exists| {
        if exists {
            summaries_path.try_exists()
        } else {
            Ok(false)
        }
    }) {
        Ok(false) => return Ok(Vec::new()),
        Ok(true) => {}
        Err(error) => return Err(error).context("checking Antigravity presence directory"),
    }
    let pids = interactive
        .iter()
        .map(|(_, process)| process.pid.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let output = Command::new("lsof")
        .args(["-nP", "-Fpfan", "-a", "-p", &pids, "+d"])
        .arg(&presence)
        .output()
        .context("running lsof for Antigravity presence (lsof is required)")?;
    // lsof exits 1 when a requested process has no matching descriptor (including exits
    // during the snapshot); records for the other selected processes are still valid.
    if !output.stderr.is_empty() || !(output.status.success() || output.status.code() == Some(1)) {
        bail!(
            "lsof Antigravity presence failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let sessions = parse_presence(
        &String::from_utf8(output.stdout).context("lsof output is not UTF-8")?,
        &presence,
    )?;
    if sessions.is_empty() {
        return Ok(Vec::new());
    }
    let summaries = open_database(&summaries_path)?;
    let mut found = Vec::new();
    for (pane, process) in interactive {
        let Some(ids) = sessions.get(&process.pid) else {
            continue;
        };
        ensure!(
            ids.len() == 1,
            "ambiguous Antigravity conversations for PID {} in pane {}",
            process.pid,
            pane.id
        );
        let id = &ids[0];
        let summary: Option<(String, String, i64)> = summaries
            .query_row(
                "SELECT status, title, not_fully_idle FROM conversation_summaries WHERE conversation_id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        // Presence precedes initialization and the workspace trust prompt. Like Claude's
        // pre-session trust dialog, this is addressable by pane id but has no agent state yet.
        let Some((status, title, not_fully_idle)) = summary else {
            continue;
        };
        ensure!(
            matches!(not_fully_idle, 0 | 1),
            "invalid Antigravity not_fully_idle flag {not_fully_idle}"
        );
        let steps = open_database(&data.join("conversations").join(format!("{id}.db")))?;
        let (state, detail) = session_state(&status, not_fully_idle == 1, &steps)?;
        ensure!(
            !found
                .iter()
                .any(|item: &Observation| item.pane_id == pane.id),
            "multiple interactive Antigravity sessions in pane {}",
            pane.id
        );
        found.push(Observation {
            pane_id: pane.id.clone(),
            state,
            detail,
            title,
            session_id: Some(id.clone()),
            preserve_title: false,
            completed_turns: None,
            completion_epoch: None,
        });
    }
    Ok(found)
}

fn open_database(path: &Path) -> Result<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening live Antigravity database {}", path.display()))
}

// ps flattens argument boundaries. Read the OS's actual argv so a prompt containing
// "--print" cannot be confused with the CLI's non-interactive mode flag.
#[cfg(target_os = "macos")]
fn process_argv(pid: i32) -> Result<Option<Vec<Vec<u8>>>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    let mut length = 0;
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            std::ptr::null_mut(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error).context("reading Antigravity argv size");
    }
    let mut buffer = vec![0u8; length];
    let result = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &mut length,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            return Ok(None);
        }
        return Err(error).context("reading Antigravity argv");
    }
    buffer.truncate(length);
    parse_procargs(&buffer).map(Some)
}

#[cfg(target_os = "macos")]
fn parse_procargs(data: &[u8]) -> Result<Vec<Vec<u8>>> {
    let count = i32::from_ne_bytes(
        data.get(..4)
            .context("short KERN_PROCARGS2 header")?
            .try_into()?,
    );
    ensure!(count > 0, "empty Antigravity argv");
    let data = &data[4..];
    let end = data
        .iter()
        .position(|&byte| byte == 0)
        .context("unterminated executable path")?;
    let data = &data[end..];
    let start = data
        .iter()
        .position(|&byte| byte != 0)
        .context("missing KERN_PROCARGS2 argv")?;
    let args: Vec<_> = data[start..]
        .split(|&byte| byte == 0)
        .take(count as usize)
        .map(<[u8]>::to_vec)
        .collect();
    ensure!(
        args.len() == count as usize,
        "truncated KERN_PROCARGS2 argv"
    );
    Ok(args)
}

#[cfg(target_os = "linux")]
fn process_argv(pid: i32) -> Result<Option<Vec<Vec<u8>>>> {
    let data = match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("reading Antigravity argv"),
    };
    if data.is_empty() {
        return Ok(None); // Exited zombie.
    }
    ensure!(data.last() == Some(&0), "unterminated Antigravity argv");
    Ok(Some(
        data[..data.len() - 1]
            .split(|&byte| byte == 0)
            .map(<[u8]>::to_vec)
            .collect(),
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_argv(_pid: i32) -> Result<Option<Vec<Vec<u8>>>> {
    bail!("Antigravity tracking supports macOS and Linux")
}

fn interactive_argv(argv: &[Vec<u8>]) -> Result<bool> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg
            .split(|&byte| byte == b'=')
            .next()
            .expect("split has first item");
        match flag {
            b"--" => break,
            b"--print" | b"--prompt" | b"-p" | b"--help" | b"-h" | b"-help" | b"--version"
            | b"-version" | b"-v" | b"-V" => return Ok(false),
            b"--add-dir"
            | b"--agent"
            | b"--conversation"
            | b"--effort"
            | b"--input-format"
            | b"--json-schema"
            | b"--log-file"
            | b"--mode"
            | b"--model"
            | b"--output-format"
            | b"--print-timeout"
            | b"--project"
            | b"--prompt-interactive"
            | b"-i"
            | b"--settings" => {
                if !arg.contains(&b'=') {
                    args.next()
                        .context("Antigravity CLI flag missing its value")?;
                }
            }
            b"--continue"
            | b"-c"
            | b"--dangerously-skip-permissions"
            | b"--disable-slash-commands"
            | b"--new-project"
            | b"--remote-control"
            | b"--sandbox" => {}
            b"agent" | b"agents" | b"changelog" | b"help" | b"install" | b"mcp" | b"mic-serve"
            | b"models" | b"plugin" | b"plugins" | b"remote-control" | b"update" => {
                return Ok(false);
            }
            _ if arg.starts_with(b"-p") => return Ok(false),
            _ if arg.starts_with(b"-") => bail!(
                "unknown Antigravity interactive option {:?}",
                String::from_utf8_lossy(arg)
            ),
            _ => {} // Positional initial prompt, as one OS argument.
        }
    }
    Ok(true)
}

fn parse_presence(output: &str, directory: &Path) -> Result<HashMap<i32, Vec<String>>> {
    let mut sessions: HashMap<i32, Vec<String>> = HashMap::new();
    let mut pid = None;
    let mut access = "";
    for line in output.lines() {
        let (tag, value) = line.split_at_checked(1).context("empty lsof field")?;
        match tag {
            "p" => {
                pid = Some(value.parse::<i32>().context("invalid lsof PID")?);
                access = "";
            }
            "f" => access = "",
            "a" => access = value,
            "n" => {
                let path = Path::new(value);
                // Native directory watchers also open files read-only; only the session's
                // read/write presence descriptor identifies its selected conversation.
                if access != "u"
                    || path.parent() != Some(directory)
                    || path.extension().is_none_or(|ext| ext != "lock")
                {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|id| id.to_str())
                    .context("invalid Antigravity presence filename")?;
                ensure!(
                    id.len() == 36
                        && id.bytes().enumerate().all(|(i, byte)| {
                            if [8, 13, 18, 23].contains(&i) {
                                byte == b'-'
                            } else {
                                byte.is_ascii_hexdigit()
                            }
                        }),
                    "unknown Antigravity presence filename: {id:?}"
                );
                let ids = sessions
                    .entry(pid.context("lsof filename before PID")?)
                    .or_default();
                if !ids.iter().any(|existing| existing == id) {
                    ids.push(id.to_string());
                }
            }
            other => bail!("unknown lsof presence field {other:?}"),
        }
    }
    Ok(sessions)
}

fn validate_summary(status: &str) -> Result<()> {
    match status {
        "CASCADE_RUN_STATUS_IDLE"
        | "CASCADE_RUN_STATUS_RUNNING"
        | "CASCADE_RUN_STATUS_BUSY"
        | "CASCADE_RUN_STATUS_CANCELING" => Ok(()),
        other => bail!("unknown Antigravity conversation status {other:?}"),
    }
}

fn current_state(steps: &Connection) -> Result<(RawState, Option<String>)> {
    let mut statement = steps.prepare(
        "SELECT status, step_payload FROM steps
         WHERE idx >= COALESCE((SELECT MAX(idx) FROM steps WHERE step_type = 14), 0)
         ORDER BY idx",
    )?;
    let current = statement
        .query_map([], |row| {
            Ok((row.get::<_, i32>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    active_state(&current)
}

fn session_state(
    status: &str,
    not_fully_idle: bool,
    steps: &Connection,
) -> Result<(RawState, Option<String>)> {
    validate_summary(status)?;
    let (mut state, detail) = current_state(steps)?;
    // Native summaries can settle to IDLE while an approval still waits. The
    // selected conversation's current-turn RequestedInteraction remains pending.
    // fullyIdle includes background tasks, as documented by the native Stop hook.
    if state != RawState::Blocked && status == "CASCADE_RUN_STATUS_IDLE" && !not_fully_idle {
        state = RawState::Idle;
    }
    Ok((state, detail))
}

fn active_state(steps: &[(i32, Vec<u8>)]) -> Result<(RawState, Option<String>)> {
    let mut waiting = None;
    for (status, payload) in steps {
        match status {
            // Native CortexStepStatus values from Antigravity CLI 1.4.12's descriptor.
            1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 11 | 12 => {}
            9 => waiting = Some(waiting_detail(payload)?),
            other => bail!("unknown Antigravity step status {other}"),
        }
    }
    Ok(match waiting {
        Some(detail) => (RawState::Blocked, Some(detail.to_string())),
        None => (RawState::Working, None),
    })
}

/// Step.requested_interaction is protobuf field 56. The database's permissions column
/// does not contain pending requests. Decode only this native field; skip other fields.
fn waiting_detail(payload: &[u8]) -> Result<&'static str> {
    let interaction = protobuf_bytes(payload, 56)?
        .context("Antigravity WAITING without requested interaction")?;
    let fields = protobuf_fields(interaction)?;
    ensure!(
        fields.len() == 1,
        "ambiguous Antigravity requested interaction"
    );
    ensure!(
        fields[0].1.is_some(),
        "Antigravity requested interaction is not a message"
    );
    match fields[0].0 {
        21 => Ok("permission required"),
        22 => Ok("question"),
        other => bail!("unknown Antigravity requested interaction {other}"),
    }
}

fn protobuf_bytes(data: &[u8], field: u64) -> Result<Option<&[u8]>> {
    let mut found = None;
    for (number, value) in protobuf_fields(data)? {
        if number == field {
            ensure!(
                found.is_none(),
                "duplicate Antigravity protobuf field {field}"
            );
            found = Some(value.context("Antigravity interaction is not a message")?);
        }
    }
    Ok(found)
}

type ProtobufField<'a> = (u64, Option<&'a [u8]>);

fn protobuf_fields(mut data: &[u8]) -> Result<Vec<ProtobufField<'_>>> {
    let mut result = Vec::new();
    while !data.is_empty() {
        let key = varint(&mut data)?;
        ensure!(key >> 3 != 0, "invalid Antigravity protobuf field zero");
        let mut value = None;
        let length = match key & 7 {
            0 => {
                varint(&mut data)?;
                0
            }
            1 => 8,
            2 => {
                let length = usize::try_from(varint(&mut data)?)?;
                value = Some(
                    data.get(..length)
                        .context("truncated Antigravity protobuf message")?,
                );
                length
            }
            5 => 4,
            wire => bail!("unsupported Antigravity protobuf wire type {wire}"),
        };
        data = data
            .get(length..)
            .context("truncated Antigravity protobuf value")?;
        result.push((key >> 3, value));
    }
    Ok(result)
}

fn varint(data: &mut &[u8]) -> Result<u64> {
    let mut value = 0;
    for shift in (0..70).step_by(7) {
        let (&byte, rest) = data
            .split_first()
            .context("truncated Antigravity protobuf varint")?;
        *data = rest;
        ensure!(
            shift != 63 || byte < 2,
            "overflowing Antigravity protobuf varint"
        );
        value |= u64::from(byte & 127) << shift;
        if byte < 128 {
            return Ok(value);
        }
    }
    bail!("overflowing Antigravity protobuf varint")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct WaitingFixture {
        name: String,
        summary: String,
        status: i32,
        interaction_hex: String,
    }

    fn fixtures() -> Vec<WaitingFixture> {
        serde_json::from_str(include_str!("../../tests/fixtures/agy/waiting.json")).unwrap()
    }

    fn unhex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn live_question_and_permission_requests_are_blocked() {
        for fixture in fixtures() {
            validate_summary(&fixture.summary).unwrap();
            let state = active_state(&[(fixture.status, unhex(&fixture.interaction_hex))]).unwrap();
            assert_eq!(state.0, RawState::Blocked, "{}", fixture.name);
            assert_eq!(
                state.1.as_deref(),
                Some(if fixture.name == "question" {
                    "question"
                } else {
                    "permission required"
                })
            );
        }
    }

    #[test]
    fn unsupported_or_broken_native_state_is_an_error() {
        assert!(validate_summary("CASCADE_RUN_STATUS_UNSPECIFIED").is_err());
        assert!(validate_summary("NEW_STATUS").is_err());
        assert!(active_state(&[(0, vec![])]).is_err());
        assert!(active_state(&[(99, vec![])]).is_err());
        assert!(active_state(&[(9, vec![])]).is_err());
        assert!(waiting_detail(&[0xc2, 3, 1]).is_err());
        assert!(waiting_detail(&[0xc2, 3, 3, 0xba, 1, 0]).is_err());
        assert!(waiting_detail(&[0xc2, 3, 3, 0xa8, 1, 0]).is_err());
        assert!(protobuf_fields(&[0xff; 11]).is_err());
    }

    #[test]
    fn live_presence_is_pid_scoped_and_read_only_watchers_are_ignored() {
        let directory = Path::new("/Users/alex/.gemini/antigravity-cli/presence");
        let sessions = parse_presence(
            include_str!("../../tests/fixtures/agy/presence.lsof"),
            directory,
        )
        .unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[&70892], ["26ea77b0-f31b-447b-ae72-d9495d596eea"]);
        assert_eq!(sessions[&49755], ["d0d83804-ad42-4bb7-840b-60618acdfdb1"]);
        let watched = "p99\nf8\nar\nn/Users/alex/.gemini/antigravity-cli/presence/26ea77b0-f31b-447b-ae72-d9495d596eea.lock\n";
        assert!(parse_presence(watched, directory).unwrap().is_empty());
        assert!(parse_presence("pwat\n", directory).is_err());
    }

    #[test]
    fn argv_boundaries_distinguish_print_mode_from_prompt_text() {
        let interactive = |args: &[&str]| {
            interactive_argv(
                &args
                    .iter()
                    .map(|s| s.as_bytes().to_vec())
                    .collect::<Vec<_>>(),
            )
            .unwrap()
        };
        assert!(interactive(&["agy"]));
        assert!(interactive(&[
            "agy",
            "--prompt-interactive",
            "explain --print"
        ]));
        assert!(interactive(&["agy", "--prompt-interactive", "--print"]));
        assert!(interactive(&["agy", "--", "--print"]));
        assert!(!interactive(&[
            "agy",
            "--model",
            "Gemini 3.8 Flash",
            "--print",
            "task"
        ]));
        assert!(!interactive(&["agy", "-p", "task"]));
        assert!(!interactive(&["agy", "--prompt=task"]));
        assert!(!interactive(&["agy", "remote-control", "start"]));
        for flag in ["--help", "-h", "-help", "--version", "-version", "-v", "-V"] {
            assert!(!interactive(&["agy", flag]), "{flag}");
        }
    }

    #[test]
    fn native_argv_can_read_the_current_process() {
        let args = process_argv(std::process::id() as i32).unwrap().unwrap();
        assert!(!args[0].is_empty());
    }

    #[test]
    fn native_background_work_remains_working_after_summary_becomes_idle() {
        #[derive(Deserialize)]
        struct Sample {
            name: String,
            summary: String,
            not_fully_idle: bool,
            steps: Vec<(i32, i32, i32)>,
            expected: String,
        }
        let samples: Vec<Sample> =
            serde_json::from_str(include_str!("../../tests/fixtures/agy/background.json")).unwrap();
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, step_payload BLOB)").unwrap();
        for sample in samples {
            db.execute("DELETE FROM steps", []).unwrap();
            for (idx, kind, status) in sample.steps {
                db.execute(
                    "INSERT INTO steps VALUES (?1,?2,?3,X'')",
                    [idx, kind, status],
                )
                .unwrap();
            }
            let state = session_state(&sample.summary, sample.not_fully_idle, &db)
                .unwrap()
                .0;
            assert_eq!(
                crate::index::State::from(state).as_str(),
                sample.expected,
                "{}",
                sample.name
            );
        }
        for status in [
            "CASCADE_RUN_STATUS_RUNNING",
            "CASCADE_RUN_STATUS_BUSY",
            "CASCADE_RUN_STATUS_CANCELING",
        ] {
            assert_eq!(
                session_state(status, false, &db).unwrap().0,
                RawState::Working
            );
        }
    }

    #[test]
    fn absence_of_agents_does_not_require_lsof_or_native_data() {
        assert!(
            scan(&ScanContext {
                panes: &[],
                processes: &super::super::ProcessTable {
                    processes: HashMap::new()
                },
            })
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn readonly_wal_queries_use_the_current_turn_and_survive_checkpoint() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("agy-wal-{}.db", std::process::id()));
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE steps (idx INTEGER PRIMARY KEY, step_type INTEGER, status INTEGER, step_payload BLOB);
            INSERT INTO steps VALUES (0,14,3,X''),(1,132,9,X''),(2,14,3,X''),(3,132,2,X'');").unwrap();
        let reader = open_database(&path).unwrap();
        assert_eq!(current_state(&reader).unwrap().0, RawState::Working);
        let payload = unhex(&fixtures()[0].interaction_hex);
        writer
            .execute(
                "UPDATE steps SET status=9, step_payload=?1 WHERE idx=3",
                [&payload],
            )
            .unwrap();
        assert_eq!(current_state(&reader).unwrap().0, RawState::Blocked);
        assert_eq!(
            session_state("CASCADE_RUN_STATUS_IDLE", false, &reader)
                .unwrap()
                .0,
            RawState::Blocked
        );
        writer
            .execute(
                "UPDATE steps SET status=3, step_payload=X'' WHERE idx=3",
                [],
            )
            .unwrap();
        assert_eq!(current_state(&reader).unwrap().0, RawState::Working);
        assert_eq!(
            session_state("CASCADE_RUN_STATUS_IDLE", false, &reader)
                .unwrap()
                .0,
            RawState::Idle
        );
        assert_eq!(
            session_state("CASCADE_RUN_STATUS_IDLE", true, &reader)
                .unwrap()
                .0,
            RawState::Working
        );
        drop(reader);
        writer
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
            .unwrap();
        drop(writer);
        let reader = open_database(&path).unwrap();
        assert_eq!(current_state(&reader).unwrap().0, RawState::Working);
        drop(reader);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn launch_stays_interactive() {
        assert_eq!(launch(Some("task")).unwrap().command, "agy");
        assert!(launch(None).is_err());
        assert!(launch(Some("")).is_err());
        assert!(launch(Some("  ")).is_err());
        assert_eq!(
            launch(Some("task")).unwrap().args,
            ["--prompt-interactive", "task"]
        );
    }
}
