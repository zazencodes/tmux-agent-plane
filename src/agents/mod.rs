//! Agent adapters own their published signals and interactive launch syntax.
pub mod claude;
pub mod codex;
pub mod opencode;
pub mod pi;

use crate::index::State;
use crate::tmux::Pane;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::process::Command;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawState {
    Working,
    Blocked,
    Idle,
}
impl From<RawState> for State {
    fn from(state: RawState) -> Self {
        match state {
            RawState::Working => Self::Working,
            RawState::Blocked => Self::Blocked,
            RawState::Idle => Self::Idle,
        }
    }
}

pub struct Observation {
    pub pane_id: String,
    pub state: RawState,
    pub detail: Option<String>,
    pub title: String,
    pub session_id: Option<String>,
    pub preserve_title: bool,
    /// Durable completed turns in the published session, when available.
    pub completed_turns: Option<u64>,
    /// Stable publisher lifetime for the monotonic completion counter.
    pub completion_epoch: Option<String>,
}

pub struct Launch {
    pub command: &'static str,
    pub args: Vec<String>,
}
pub struct Adapter {
    pub scan: fn(&ScanContext<'_>) -> Result<Vec<Observation>>,
    pub launch: fn(Option<&str>) -> Launch,
}
pub struct ScanContext<'a> {
    pub panes: &'a [Pane],
    pub processes: &'a ProcessTable,
}

#[derive(Debug)]
pub struct Process {
    pub pid: i32,
    pub parent: i32,
    pub executable: String,
    /// The complete argument string published by ps; it is not shell-parsed.
    pub argv: String,
}
pub struct ProcessTable {
    pub processes: HashMap<i32, Process>,
}
impl ProcessTable {
    pub fn load() -> Result<Self> {
        // A wide fixed executable column preserves paths containing spaces on BSD and GNU ps.
        let exe_header = "E".repeat(1024);
        let format = format!("pid=PID,ppid=PPID,comm={exe_header},args=ARGV");
        let out = Command::new("ps")
            .args(["-A", "-ww", "-o", &format])
            .output()
            .context("failed to execute ps")?;
        if !out.status.success() {
            bail!("ps failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Self::parse(&String::from_utf8(out.stdout).context("ps output is not UTF-8")?)
    }
    fn parse(output: &str) -> Result<Self> {
        let mut lines = output.lines();
        let header = lines.next().context("ps output has no header")?;
        let exe = header.find("EEE").context("ps executable header missing")?;
        let argv = header.find("ARGV").context("ps argument header missing")?;
        let mut processes = HashMap::new();
        for line in lines {
            let ids = line
                .get(..exe)
                .context("short ps row")?
                .split_whitespace()
                .collect::<Vec<_>>();
            if ids.len() != 2 {
                bail!("invalid ps identity columns");
            }
            let executable = line
                .get(exe..argv)
                .context("short ps executable column")?
                .trim_end()
                .to_string();
            if executable.len() >= argv - exe - 1 {
                bail!("ps executable column truncated");
            }
            let process = Process {
                pid: ids[0].parse()?,
                parent: ids[1].parse()?,
                executable,
                argv: line.get(argv..).unwrap_or("").to_string(),
            };
            processes.insert(process.pid, process);
        }
        Ok(Self { processes })
    }
    pub fn descendants_named(&self, root: i32, name: &str) -> Vec<&Process> {
        self.processes
            .values()
            .filter(|p| {
                !p.argv.is_empty()
                    && p.executable.rsplit('/').next() == Some(name)
                    && self.is_descendant(p.pid, root)
            })
            .collect()
    }
    pub fn is_descendant(&self, pid: i32, root: i32) -> bool {
        let mut current = pid;
        for _ in 0..=self.processes.len() {
            if current == root {
                return true;
            }
            let Some(process) = self.processes.get(&current) else {
                return false;
            };
            if process.parent == current {
                return false;
            }
            current = process.parent;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_columns_preserve_spaces_and_ancestry() {
        let header = format!("PID PPID {} ARGV\n", "E".repeat(64));
        let row = |pid, parent, exe, args| format!("{pid:<4}{parent:<5}{exe:<65}{args}\n");
        let output = header
            + &row(1, 0, "/bin/shell", "shell")
            + &row(
                2,
                1,
                "/Applications/Agent App/codex",
                "codex task with spaces",
            );
        let table = ProcessTable::parse(&output).unwrap();
        assert_eq!(table.processes[&2].argv, "codex task with spaces");
        assert_eq!(table.descendants_named(1, "codex").len(), 1);
        assert!(!table.is_descendant(1, 2));
    }
    #[test]
    fn live_process_snapshot_has_caller() {
        let table = ProcessTable::load().unwrap();
        assert!(table.processes.contains_key(&(std::process::id() as i32)));
    }
}
