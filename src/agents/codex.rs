//! Codex publishes activity and approvals in its terminal title.
use super::{Launch, Observation, ProcessTable, RawState, ScanContext};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;

pub fn launch(task: Option<&str>, agent_args: &[String]) -> Result<Launch> {
    let mut args = agent_args.to_vec();
    args.extend(task.map(str::to_owned));
    Ok(Launch {
        command: "codex",
        args,
    })
}

pub fn resume(session_id: &str) -> Launch {
    Launch {
        command: "codex",
        args: vec!["resume".into(), session_id.into()],
    }
}

// Codex bounds title segments to 32 characters. Resolve its published UUID prefix against
// its native thread database; ambiguous prefixes must never reopen another conversation.
fn thread_id(title: &str, database: impl FnOnce() -> Result<Connection>) -> Result<Option<String>> {
    let token = title
        .split(" | ")
        .map(|part| part.trim_start_matches(|c: char| !c.is_ascii_hexdigit()))
        .find(|part| {
            uuid_prefix(part.trim_end_matches("...")) && (part.len() == 36 || part.ends_with("..."))
        });
    let Some(token) = token else { return Ok(None) };
    if token.len() == 36 && !token.ends_with("...") {
        return Ok(Some(token.to_owned()));
    }
    let prefix = token.trim_end_matches("...");
    let db = database()?;
    let mut statement = db.prepare("SELECT id FROM threads WHERE id GLOB ?1 LIMIT 2")?;
    let ids = statement
        .query_map([format!("{prefix}*")], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match ids.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(id.clone())),
        _ => bail!("ambiguous Codex terminal-title thread id {prefix}"),
    }
}

fn uuid_prefix(value: &str) -> bool {
    (29..=36).contains(&value.len())
        && value.bytes().enumerate().all(|(i, c)| {
            if [8, 13, 18, 23].contains(&i) {
                c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn database() -> Result<Connection> {
    let home = match std::env::var_os("CODEX_HOME") {
        Some(home) => PathBuf::from(home),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".codex"),
    };
    let path = std::fs::read_dir(&home)
        .with_context(|| format!("reading {}", home.display()))?
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter_map(|path| {
            let version = path
                .file_name()?
                .to_str()?
                .strip_prefix("state_")?
                .strip_suffix(".sqlite")?
                .parse::<u32>()
                .ok()?;
            Some((version, path))
        })
        .max_by_key(|(version, _)| *version)
        .context("Codex thread database is missing")?
        .1;
    Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {}", path.display()))
}

pub fn scan(context: &ScanContext<'_>) -> Result<Vec<Observation>> {
    let mut found = Vec::new();
    for pane in context.panes {
        let Some(pid) = agent_pid(context.processes, pane.pid) else {
            continue;
        };
        let (state, detail) = codex_state(&pane.title);
        found.push(Observation {
            pane_id: pane.id.clone(),
            pid,
            state,
            detail,
            title: clean_title(&pane.title)
                .split(" | ")
                .filter(|part| !uuid_prefix(part.trim_end_matches("...")))
                .collect::<Vec<_>>()
                .join(" | "),
            session_id: thread_id(&pane.title, database)?,
            preserve_title: state == RawState::Blocked,
            completed_turns: None,
            completion_epoch: None,
        });
    }
    Ok(found)
}

/// The pane's Codex agent process. Codex starts helper `codex` children; only the outermost
/// process is the agent. A `codex` run by a Claude Code session in the same pane is that
/// session's tool. Several top-level `codex` processes (parallel one-off runs) identify no agent.
pub fn agent_pid(processes: &ProcessTable, pane_pid: i32) -> Option<i32> {
    let all: Vec<_> = processes
        .descendants_named(pane_pid, "codex")
        .into_iter()
        .filter(|p| !processes.has_ancestor_named(p.pid, pane_pid, "claude"))
        .collect();
    let outermost: Vec<_> = all
        .iter()
        .filter(|p| {
            !all.iter()
                .any(|a| a.pid != p.pid && processes.is_descendant(p.pid, a.pid))
        })
        .collect();
    match outermost.as_slice() {
        [agent] => Some(agent.pid),
        _ => None,
    }
}

fn clean_title(title: &str) -> String {
    title
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim()
        .to_string()
}

fn codex_state(title: &str) -> (RawState, Option<String>) {
    if title.contains("Action Required") {
        return (RawState::Blocked, Some("action required".to_string()));
    }
    let spinning = title
        .chars()
        .next()
        .is_some_and(|c| ('\u{2800}'..='\u{28FF}').contains(&c));
    if spinning {
        (RawState::Working, None)
    } else {
        (RawState::Idle, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::Process;
    use crate::tmux::Pane;
    use std::collections::HashMap;

    fn process(pid: i32, parent: i32, executable: &str) -> (i32, Process) {
        let process = Process {
            pid,
            parent,
            executable: executable.into(),
            argv: executable.into(),
        };
        (pid, process)
    }

    fn pane(id: &str, pid: i32) -> Pane {
        Pane {
            id: id.into(),
            pid,
            session: "s".into(),
            session_id: "$1".into(),
            workspace: "main".into(),
            window: "@1".into(),
            window_index: 0,
            pane_index: 0,
            cwd: "/".into(),
            title: "Fix tests | repo".into(),
            active: false,
            viewed: false,
            sidebar: false,
            name: String::new(),
            parent: String::new(),
            orchestrator_role: String::new(),
            placed: false,
            task: false,
        }
    }

    #[test]
    fn nested_codex_helpers_belong_to_one_agent() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/codex"),
                process(12, 11, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert_eq!(scan(&context).unwrap().len(), 1);
    }

    #[test]
    fn agent_pid_is_the_outermost_codex_past_helpers_under_other_processes() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/codex"),
                process(12, 11, "/bin/node_repl"),
                process(13, 12, "/bin/codex"),
                process(14, 12, "/bin/codex"),
            ]),
        };
        assert_eq!(agent_pid(&processes, 10), Some(11));
    }

    #[test]
    fn codex_run_by_claude_in_the_pane_is_not_an_agent() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/claude"),
                process(12, 11, "/bin/codex"),
                process(13, 11, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(scan(&context).unwrap().is_empty());
    }

    #[test]
    fn sibling_codex_processes_are_not_tracked_and_raise_no_fault() {
        let processes = ProcessTable {
            processes: HashMap::from([
                process(10, 1, "/bin/zsh"),
                process(11, 10, "/bin/codex"),
                process(12, 10, "/bin/codex"),
            ]),
        };
        let panes = [pane("%1", 10)];
        let context = ScanContext {
            panes: &panes,
            processes: &processes,
        };
        assert!(scan(&context).unwrap().is_empty());
    }

    #[test]
    fn published_thread_ids_resolve_exactly_and_reject_ambiguous_prefixes() {
        let id = "019af123-4567-7890-abcd-123456789abc";
        let db = || {
            let db = Connection::open_in_memory()?;
            db.execute("CREATE TABLE threads (id TEXT)", [])?;
            db.execute("INSERT INTO threads VALUES (?1)", [id])?;
            Ok(db)
        };
        assert_eq!(
            thread_id(&format!("⠼ {id} | Fix tests | repo"), || panic!(
                "full id needs no database"
            ))
            .unwrap()
            .as_deref(),
            Some(id)
        );
        let title = format!("[ ! ] Action Required | {}... | Fix tests", &id[..29]);
        assert_eq!(thread_id(&title, db).unwrap().as_deref(), Some(id));
        assert!(
            thread_id(&title, || {
                let db = db()?;
                db.execute(
                    "INSERT INTO threads VALUES (?1)",
                    [format!("{}0000000", &id[..29])],
                )?;
                Ok(db)
            })
            .unwrap_err()
            .to_string()
            .contains("ambiguous")
        );
        assert_eq!(
            thread_id("Fix tests | repo", || panic!(
                "no title id needs no database"
            ))
            .unwrap(),
            None
        );
        assert!(!uuid_prefix("019af123-4567-7890-abcd-1234567_"));
    }

    #[test]
    fn codex_titles() {
        assert_eq!(codex_state("⠼ Fix tests | repo").0, RawState::Working);
        assert_eq!(codex_state("[ ! ] Action Required").0, RawState::Blocked);
        assert_eq!(codex_state("[ . ] Action Required").0, RawState::Blocked);
        assert_eq!(codex_state("Fix tests | repo").0, RawState::Idle);
    }

    #[test]
    fn titles_lose_status_glyphs() {
        assert_eq!(
            clean_title("✳ Tmux agent monitoring"),
            "Tmux agent monitoring"
        );
        assert_eq!(clean_title("⠼ Fix tests | repo"), "Fix tests | repo");
        assert_eq!(clean_title("[ ! ] Action Required"), "Action Required");
    }

    #[test]
    fn launch_puts_agent_args_before_task() {
        let args = ["-m".to_string(), "gpt".to_string()];
        let launch_with = launch(Some("task"), &args).unwrap();
        assert_eq!(launch_with.command, "codex");
        assert_eq!(launch_with.args, ["-m", "gpt", "task"]);
        assert_eq!(launch(None, &args).unwrap().args, ["-m", "gpt"]);
        assert!(launch(None, &[]).unwrap().args.is_empty());
    }
}
