//! Agent adapters own their published signals and interactive launch syntax.
pub mod agy;
pub mod claude;
pub mod codex;
pub mod grok;
pub mod opencode;
pub mod pi;

use crate::index::State;
use crate::tmux::Pane;
use anyhow::{Context, Result, bail};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
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
    /// Monotonic completed turns within completion_epoch, when available.
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
    pub launch: fn(Option<&str>) -> Result<Launch>,
}
pub struct ScanContext<'a> {
    pub panes: &'a [Pane],
    pub processes: &'a ProcessTable,
}

#[derive(Debug)]
pub struct Process {
    pub pid: i32,
    pub parent: i32,
    /// BSD executable path on macOS; kernel comm name on Linux.
    pub executable: String,
    /// Published command line, flattened without shell parsing.
    pub argv: String,
}
/// Live processes only. An exited process awaiting its parent's wait has lost its command line.
pub struct ProcessTable {
    pub processes: HashMap<i32, Process>,
}
impl ProcessTable {
    pub fn load() -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            Self::load_proc(std::path::Path::new("/proc"))
        }
        #[cfg(target_os = "macos")]
        {
            Self::load_bsd_ps()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        bail!("process snapshots require macOS or Linux")
    }
    #[cfg(target_os = "macos")]
    fn load_bsd_ps() -> Result<Self> {
        // BSD ps pads comm to this header's width, preserving paths containing spaces.
        // procps does not: Linux reads /proc and never uses this layout.
        let exe_header = "E".repeat(1024);
        let format = format!("pid=PID,ppid=PPID,stat=STAT,comm={exe_header},args=ARGV");
        let out = Command::new("ps")
            .args(["-A", "-ww", "-o", &format])
            .output()
            .context("failed to execute ps")?;
        if !out.status.success() {
            bail!("ps failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        Self::parse_bsd_ps(&String::from_utf8(out.stdout).context("ps output is not UTF-8")?)
    }
    #[cfg(any(target_os = "macos", test))]
    fn parse_bsd_ps(output: &str) -> Result<Self> {
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
            if ids.len() != 3 {
                bail!("invalid ps identity columns");
            }
            // Z is a zombie; the E flag means the process is exiting.
            if ids[2].starts_with('Z') || ids[2].contains('E') {
                continue;
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
    #[cfg(any(target_os = "linux", test))]
    fn load_proc(root: &std::path::Path) -> Result<Self> {
        let mut processes = HashMap::new();
        for entry in std::fs::read_dir(root).context("listing proc process records")? {
            let entry = entry.context("reading proc process entry")?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<i32>().ok())
                .filter(|pid| *pid > 0)
            else {
                continue;
            };
            match read_proc_process(&entry.path(), pid) {
                Ok(Some(process)) => {
                    processes.insert(pid, process);
                }
                Ok(None) => {} // The process changed identity while its files were read.
                Err(error)
                    if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                        e.kind() == std::io::ErrorKind::NotFound
                            || e.raw_os_error() == Some(libc::ESRCH)
                    }) => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("reading proc process {pid}"));
                }
            }
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
    /// Whether a process named `name` sits between `pid` and `root`, exclusive of both.
    pub fn has_ancestor_named(&self, pid: i32, root: i32, name: &str) -> bool {
        let mut current = self.processes.get(&pid).map(|p| p.parent);
        for _ in 0..=self.processes.len() {
            let Some(id) = current else { return false };
            if id == root {
                return false;
            }
            let Some(process) = self.processes.get(&id) else {
                return false;
            };
            if process.executable.rsplit('/').next() == Some(name) {
                return true;
            }
            current = Some(process.parent);
        }
        false
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

/// Field 2 is parenthesized and may itself contain spaces, newlines and parentheses.
/// Fields after its final ')' are numeric except the single-character process state.
#[cfg(any(target_os = "linux", test))]
#[derive(PartialEq, Eq)]
struct ProcIdentity {
    pid: i32,
    parent: i32,
    name: String,
    started: u64,
    exited: bool,
}
#[cfg(any(target_os = "linux", test))]
fn parse_proc_stat(stat: &str, expected_pid: i32) -> Result<ProcIdentity> {
    let (pid, rest) = stat.split_once(" (").context("proc stat command missing")?;
    let (name, tail) = rest
        .rsplit_once(')')
        .context("proc stat command unterminated")?;
    let fields: Vec<_> = tail.split_whitespace().collect();
    let pid: i32 = pid.parse().context("invalid proc stat pid")?;
    if pid != expected_pid || fields.len() < 20 || fields[0].len() != 1 {
        bail!("invalid proc stat identity");
    }
    Ok(ProcIdentity {
        pid,
        parent: fields[1].parse().context("invalid proc stat parent")?,
        name: name.to_owned(),
        // starttime is field 22; the suffix starts at field 3 (state).
        started: fields[19].parse().context("invalid proc stat starttime")?,
        exited: matches!(fields[0], "Z" | "X" | "x"),
    })
}
#[cfg(any(target_os = "linux", test))]
fn read_proc_process(path: &std::path::Path, pid: i32) -> Result<Option<Process>> {
    let read_identity = || -> Result<ProcIdentity> {
        parse_proc_stat(
            &std::fs::read_to_string(path.join("stat")).context("reading proc stat")?,
            pid,
        )
    };
    let before = read_identity()?;
    let bytes = std::fs::read(path.join("cmdline")).context("reading proc cmdline")?;
    let after = read_identity()?;
    // Reject detected PID reuse, comm changes or reparenting between reads.
    // Command-line bytes may still change within one process lifetime, as with ps.
    if before != after || after.exited {
        return Ok(None);
    }
    let argv = std::str::from_utf8(&bytes)
        .context("proc cmdline is not UTF-8")?
        .trim_end_matches('\0')
        .replace('\0', " ");
    Ok(Some(Process {
        pid,
        parent: after.parent,
        executable: after.name,
        argv,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn process_columns_preserve_spaces_and_ancestry() {
        let header = format!("PID PPID STAT {} ARGV\n", "E".repeat(64));
        let row =
            |pid, parent, stat, exe, args| format!("{pid:<4}{parent:<5}{stat:<5}{exe:<65}{args}\n");
        let output = header
            + &row(1, 0, "Ss", "/bin/shell", "shell")
            + &row(
                2,
                1,
                "S+",
                "/Applications/Agent App/codex",
                "codex task with spaces",
            )
            + &row(3, 1, "Z", "(pi)", "<defunct>")
            + &row(4, 1, "SE", "pi", "(pi)");
        let table = ProcessTable::parse_bsd_ps(&output).unwrap();
        assert_eq!(table.processes[&2].argv, "codex task with spaces");
        assert_eq!(table.descendants_named(1, "codex").len(), 1);
        assert!(!table.is_descendant(1, 2));
        assert!(!table.processes.contains_key(&3));
        assert!(!table.processes.contains_key(&4));
    }
    struct ProcFixture(std::path::PathBuf);
    impl ProcFixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "plane-proc-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn process(&self, pid: i32, parent: i32, name: &str, cmdline: &[u8]) {
            let path = self.0.join(pid.to_string());
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("stat"), stat(pid, parent, name)).unwrap();
            std::fs::write(path.join("cmdline"), cmdline).unwrap();
        }
    }
    impl Drop for ProcFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn stat(pid: i32, parent: i32, name: &str) -> String {
        format!("{pid} ({name}) S {parent} {}12345 0\n", "0 ".repeat(17))
    }
    #[test]
    fn linux_proc_snapshot_preserves_names_arguments_and_ancestry() {
        let fixture = ProcFixture::new();
        fixture.process(100, 0, "shell", b"shell\0");
        fixture.process(
            101,
            100,
            "codex",
            b"/tmp/Agent App/codex\0task with spaces\0",
        );
        fixture.process(102, 100, "name )\n(with", b"changed title\0\0\0");
        fixture.process(103, 0, "kworker", b"");
        fixture.process(104, 100, "exited", b"exited\0");
        std::fs::remove_file(fixture.0.join("104/cmdline")).unwrap();
        fixture.process(105, 100, "pi", b"");
        let zombie = stat(105, 100, "pi").replace(") S ", ") Z ");
        std::fs::write(fixture.0.join("105/stat"), zombie).unwrap();
        // Reproduce the reported procps shape without copying real command lines.
        // It is not a Linux input: /proc is canonical even if procps is installed.
        let header = format!("    PID    PPID {} ARGV\n", "E".repeat(1024));
        let procps = header + "    101     100 codex           /tmp/Agent App/codex task\n";
        assert!(ProcessTable::parse_bsd_ps(&procps).is_err());
        std::fs::write(fixture.0.join("procps-output"), procps).unwrap();
        let table = ProcessTable::load_proc(&fixture.0).unwrap();
        assert_eq!(table.processes[&101].executable, "codex");
        assert_eq!(
            table.processes[&101].argv,
            "/tmp/Agent App/codex task with spaces"
        );
        assert_eq!(table.processes[&102].executable, "name )\n(with");
        assert_eq!(table.processes[&102].argv, "changed title");
        assert!(table.processes[&103].argv.is_empty());
        assert!(!table.processes.contains_key(&104));
        assert!(!table.processes.contains_key(&105));
        assert_eq!(table.descendants_named(100, "codex").len(), 1);
        assert!(!table.is_descendant(100, 101));
    }
    #[test]
    fn linux_proc_malformed_or_unreadable_records_fail_loudly() {
        assert!(parse_proc_stat("101 (codex) S 100", 101).is_err());
        assert!(parse_proc_stat(&stat(101, 100, "codex"), 102).is_err());
        let fixture = ProcFixture::new();
        fixture.process(101, 100, "codex", b"codex\0");
        // A directory is a deterministic read failure, even when tests run as root.
        std::fs::remove_file(fixture.0.join("101/cmdline")).unwrap();
        std::fs::create_dir(fixture.0.join("101/cmdline")).unwrap();
        let error = ProcessTable::load_proc(&fixture.0).err().unwrap();
        assert!(format!("{error:#}").contains("reading proc process 101: reading proc cmdline"));
    }
    #[test]
    fn live_process_snapshot_has_named_child_with_spaced_executable_path() {
        struct Child(std::process::Child);
        impl Drop for Child {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let fixture = ProcFixture::new();
        let directory = fixture.0.join("Agent App");
        std::fs::create_dir(&directory).unwrap();
        let executable = directory.join("codex");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        let child = Child(
            std::process::Command::new(&executable)
                .arg("30")
                .spawn()
                .unwrap(),
        );
        let table = ProcessTable::load().unwrap();
        let process = &table.processes[&(child.0.id() as i32)];
        assert_eq!(process.executable.rsplit('/').next(), Some("codex"));
        #[cfg(target_os = "macos")]
        assert_eq!(process.executable, executable.to_str().unwrap());
        #[cfg(target_os = "linux")]
        assert_eq!(process.executable, "codex");
        assert_eq!(process.argv, format!("{} 30", executable.display()));
        assert!(table.is_descendant(process.pid, std::process::id() as i32));
        assert_eq!(
            table
                .descendants_named(std::process::id() as i32, "codex")
                .len(),
            1
        );
        let panes = [Pane {
            id: "%fixture".into(),
            pid: std::process::id() as i32,
            session: "fixture".into(),
            window: "@fixture".into(),
            window_index: 0,
            pane_index: 0,
            cwd: directory.to_string_lossy().into_owned(),
            title: "⠼ synthetic task".into(),
            visible: false,
            name: "fixture".into(),
            parent: String::new(),
        }];
        // Exercise the daemon's real process-to-pane adapter with the native snapshot.
        let observations = codex::scan(&ScanContext {
            panes: &panes,
            processes: &table,
        })
        .unwrap();
        assert_eq!(observations.len(), 1);
        assert_eq!(observations[0].pane_id, "%fixture");
        assert_eq!(observations[0].state, RawState::Working);
    }
    #[test]
    fn live_process_snapshot_has_caller() {
        let table = ProcessTable::load().unwrap();
        assert!(table.processes.contains_key(&(std::process::id() as i32)));
    }
}
