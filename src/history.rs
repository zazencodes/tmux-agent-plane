//! Recent conversations: the daemon alone records live observations and confirmed exits.
use crate::diagnostics::Issue;
use crate::index::{self, Agent};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

const CLOSED_LIMIT: usize = 100;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub id: u64,
    pub agent: Agent,
    pub closed_at: Option<u64>,
}

fn path() -> Result<std::path::PathBuf> {
    Ok(index::state_dir()?.join("history.json"))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
struct History {
    next_id: u64,
    entries: Vec<Entry>,
}

fn read() -> Result<History> {
    let path = path()?;
    match std::fs::read(&path) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(History::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn same(a: &Agent, b: &Agent) -> bool {
    a.kind == b.kind
        && match (&a.session_id, &b.session_id) {
            (Some(a), Some(b)) => a == b,
            (None, None) => a.process_id == b.process_id && a.pane == b.pane,
            _ => false,
        }
}

fn update(history: &mut History, agents: &[Agent], now: u64) -> Result<()> {
    let entries = &mut history.entries;
    for agent in agents {
        // A conversation id may arrive after the process's first observation.
        let existing = entries.iter_mut().find(|e| {
            same(&e.agent, agent)
                || (e.closed_at.is_none()
                    && e.agent.kind == agent.kind
                    && e.agent.session_id.is_none()
                    && e.agent.process_id == agent.process_id
                    && e.agent.pane == agent.pane)
        });
        match existing {
            Some(entry) => {
                entry.agent = agent.clone();
                entry.closed_at = None;
            }
            None => {
                history.next_id = history
                    .next_id
                    .checked_add(1)
                    .context("history id exhausted")?;
                entries.push(Entry {
                    id: history.next_id,
                    agent: agent.clone(),
                    closed_at: None,
                });
            }
        }
    }
    for entry in entries.iter_mut() {
        if entry.closed_at.is_none() && !agents.iter().any(|a| same(&entry.agent, a)) {
            entry.closed_at = Some(now);
        }
    }
    entries.sort_by_key(|e| {
        (
            std::cmp::Reverse(e.closed_at.unwrap_or(u64::MAX)),
            std::cmp::Reverse(e.id),
        )
    });
    let mut closed = 0;
    entries.retain(|entry| {
        if entry.closed_at.is_some() {
            closed += 1;
        }
        closed <= CLOSED_LIMIT
    });
    Ok(())
}

pub static RECORDER: std::sync::Mutex<Recorder> = std::sync::Mutex::new(Recorder { entries: None });

#[derive(Default)]
pub struct Recorder {
    entries: Option<History>,
}

impl Recorder {
    pub fn tick(&mut self, agents: &[Agent], clean: bool, now: u64) -> Vec<Issue> {
        if !clean {
            return Vec::new();
        }
        match self.keep(agents, now) {
            Ok(()) => Vec::new(),
            Err(error) => vec![Issue::new("history", error)],
        }
    }

    pub fn forget(&mut self, id: u64) -> Result<()> {
        let mut history = read()?;
        let i = history
            .entries
            .iter()
            .position(|e| e.id == id && e.closed_at.is_some())
            .context("closed conversation is no longer available")?;
        history.entries.remove(i);
        save(&history)?;
        self.entries = Some(history);
        Ok(())
    }

    fn keep(&mut self, agents: &[Agent], now: u64) -> Result<()> {
        let previous = match &self.entries {
            Some(entries) => entries.clone(),
            None => read()?,
        };
        let mut entries = previous.clone();
        update(&mut entries, agents, now)?;
        if entries != previous {
            save(&entries)?;
        }
        self.entries = Some(entries);
        Ok(())
    }
}

fn save(entries: &History) -> Result<()> {
    let path = path()?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(entries)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

pub fn forget(id: u64) -> Result<()> {
    crate::lifecycle::ask(&format!("forget {id}"), Duration::from_secs(5))
}

pub fn recent(workspace: Option<&str>) -> Result<Vec<Entry>> {
    closed_entries(workspace, &index::read()?.agents)
}

fn closed_entries(workspace: Option<&str>, live: &[Agent]) -> Result<Vec<Entry>> {
    Ok(read()?
        .entries
        .into_iter()
        .filter(|entry| {
            entry.closed_at.is_some()
                && workspace.is_none_or(|w| entry.agent.workspace == w)
                && !live.iter().any(|a| same(&entry.agent, a))
        })
        .collect())
}

pub fn reason(entry: &Entry) -> Option<String> {
    if entry.agent.session_id.is_none() {
        Some(if entry.agent.kind == index::Kind::Codex {
            "Codex closed without a session ID. For future chats, add Session ID in /terminal-title.".into()
        } else {
            "it published no session id".into()
        })
    } else if !std::path::Path::new(&entry.agent.cwd).is_dir() {
        Some("its folder is gone".into())
    } else {
        None
    }
}

/// Serialize revives through detection, so a second request sees the first running again.
pub fn revive(id: u64) -> Result<Agent> {
    let file = std::fs::File::create(index::state_dir()?.join("revive.lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error()).context("locking revive.lock");
    }
    let entry = recent(None)?
        .into_iter()
        .find(|e| e.id == id)
        .context("closed session is no longer available")?;
    if let Some(reason) = reason(&entry) {
        bail!("{reason}");
    }
    let pane = crate::restore::revive(&entry.agent)?;
    let started = Instant::now();
    loop {
        if let Some(agent) = index::read()?
            .agents
            .into_iter()
            .find(|a| a.pane == pane && same(a, &entry.agent))
        {
            return Ok(agent);
        }
        if started.elapsed() >= Duration::from_secs(15) {
            bail!(
                "conversation did not resume in pane {pane}; inspect it with `tmux-agent-plane read {pane}`"
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn matches(entry: &Entry, project: Option<&str>, query: &str) -> bool {
    let a = &entry.agent;
    if project.is_some_and(|p| p != a.session) {
        return false;
    }
    if query.is_empty() {
        return true;
    }
    let fields = format!("{} {} {} {}", a.title, a.name, a.session, a.kind.as_str()).to_lowercase();
    query
        .split_whitespace()
        .all(|word| fields.contains(&word.to_lowercase()))
}

pub fn list(json: bool, project: Option<&str>, query: &str) -> Result<()> {
    let agents = index::read()?.agents;
    let entries: Vec<_> = closed_entries(None, &agents)?
        .into_iter()
        .filter(|e| matches(e, project, query))
        .collect();
    if json {
        let mut rows = Vec::new();
        for entry in entries {
            let [r, g, b] =
                crate::ui::project_rgb(&agents, &entry.agent.workspace, &entry.agent.session);
            let mut row = serde_json::to_value(entry)?;
            row["project_color"] = format!("#{r:02x}{g:02x}{b:02x}").into();
            rows.push(row);
        }
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else if entries.is_empty() {
        println!("no recently closed sessions");
    } else {
        for entry in entries {
            println!(
                "{}  {}  {}  {}  {} ago  {}{}",
                entry.id,
                entry.agent.name,
                entry.agent.kind.as_str(),
                entry.agent.session,
                crate::ui::age(entry.closed_at.unwrap()),
                entry.agent.title,
                reason(&entry)
                    .map(|r| format!("  ({r})"))
                    .unwrap_or_default()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::index::{Kind, State};

    pub(crate) fn agent(id: Option<&str>) -> Agent {
        Agent {
            name: "worker".into(),
            pane: "%1".into(),
            kind: Kind::Pi,
            state: State::Idle,
            detail: None,
            title: "Fix the tests".into(),
            location: "pro:1.0".into(),
            session: "pro".into(),
            workspace: "main".into(),
            window: "@1".into(),
            cwd: "/tmp".into(),
            parent: None,
            parent_ended: false,
            orchestrator: false,
            process_id: 10,
            session_id: id.map(str::to_owned),
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            project_slot: None,
            visible: false,
        }
    }

    #[test]
    fn exits_reopens_thread_switches_and_late_ids_keep_one_conversation() {
        let mut history = History::default();
        update(&mut history, &[agent(None)], 1).unwrap();
        update(&mut history, &[agent(Some("one"))], 2).unwrap();
        assert_eq!(history.entries.len(), 1);
        let id = history.entries[0].id;
        update(&mut history, &[], 3).unwrap();
        assert_eq!(history.entries[0].closed_at, Some(3));
        // A daemon reload reads the same records, and a replacement process resumes the id.
        let mut history: History =
            serde_json::from_slice(&serde_json::to_vec(&history).unwrap()).unwrap();
        let mut resumed = agent(Some("one"));
        resumed.pane = "%2".into();
        resumed.process_id = 20;
        update(&mut history, &[resumed.clone()], 4).unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(
            (history.entries[0].id, history.entries[0].closed_at),
            (id, None)
        );
        resumed.session_id = Some("two".into());
        update(&mut history, &[resumed], 5).unwrap();
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[1].closed_at, Some(5));
        update(&mut history, &[], 6).unwrap();
        assert_eq!(history.entries[0].agent.session_id.as_deref(), Some("two"));
        assert_eq!(history.entries[0].closed_at, Some(6));
    }

    #[test]
    fn keeps_one_hundred_closed_conversations_and_all_live_ones() {
        let mut history = History::default();
        for i in 0..105 {
            update(&mut history, &[agent(Some(&i.to_string()))], i).unwrap();
        }
        assert_eq!(history.entries.len(), CLOSED_LIMIT + 1);
        assert!(history.entries[0].closed_at.is_none());
        assert_eq!(
            history.entries.last().unwrap().agent.session_id.as_deref(),
            Some("4")
        );
        assert!(reason(history.entries.last().unwrap()).is_none());
        let mut missing = history.entries[0].clone();
        missing.agent.cwd = "/no-such-plane-test-folder".into();
        assert_eq!(reason(&missing).as_deref(), Some("its folder is gone"));
        missing.agent.session_id = None;
        assert_eq!(
            reason(&missing).as_deref(),
            Some("it published no session id")
        );
    }

    #[test]
    fn evicted_ids_are_never_reused() {
        let mut history = History {
            next_id: 1000,
            entries: Vec::new(),
        };
        update(&mut history, &[agent(Some("new"))], 1).unwrap();
        assert_eq!(history.entries[0].id, 1001);
    }

    #[test]
    fn faulty_scans_do_not_mark_agents_closed() {
        let mut recorder = Recorder {
            entries: Some(History {
                next_id: 1,
                entries: vec![Entry {
                    id: 1,
                    agent: agent(Some("one")),
                    closed_at: None,
                }],
            }),
        };
        assert!(recorder.tick(&[], false, 10).is_empty());
        assert!(recorder.entries.unwrap().entries[0].closed_at.is_none());
    }
}
