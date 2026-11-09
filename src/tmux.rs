use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

/// Field separator for `list-panes -F` output. Pane titles never contain it.
const SEP: char = '\x1f';

pub fn run(args: &[&str]) -> Result<String> {
    let out = Command::new("tmux")
        .args(args)
        .output()
        .context("failed to execute tmux")?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Match the session's configured shell, including a session-local override.
pub fn default_shell(pane: Option<&str>) -> Result<String> {
    let mut args = vec!["display-message", "-p"];
    if let Some(pane) = pane {
        args.extend(["-t", pane]);
    }
    args.push("#{default-shell}");
    let shell = run(&args)?.trim().to_owned();
    if shell.is_empty() {
        bail!("tmux default-shell is empty; cannot launch an interactive shell");
    }
    Ok(shell)
}

pub fn run_with_stdin(args: &[&str], input: &str) -> Result<()> {
    let mut child = Command::new("tmux")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to execute tmux")?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "tmux {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct Pane {
    pub id: String,
    pub pid: i32,
    pub session: String,
    pub window: String,
    pub window_index: u32,
    pub pane_index: u32,
    pub cwd: String,
    pub title: String,
    /// The active pane of its window.
    pub active: bool,
    /// Its window is on screen: the current window of an attached session.
    pub viewed: bool,
    /// This pane is a sidebar.
    pub sidebar: bool,
    pub name: String,
    pub parent: String,
    /// The pane has its project and is never placed automatically again.
    pub placed: bool,
    /// The pane runs a background task, which is never an agent.
    pub task: bool,
}

impl Pane {
    /// The pane is on screen and takes the keyboard.
    pub fn visible(&self) -> bool {
        self.active && self.viewed
    }

    pub fn location(&self) -> String {
        format!("{}:{}.{}", self.session, self.window_index, self.pane_index)
    }
}

const PANE_FIELDS: [&str; 15] = [
    "#{pane_id}",
    "#{pane_pid}",
    "#{session_name}",
    "#{window_id}",
    "#{window_index}",
    "#{pane_index}",
    "#{pane_current_path}",
    "#{pane_title}",
    "#{pane_active}",
    "#{&&:#{window_active},#{session_attached}}",
    "#{@agent-plane-sidebar}",
    "#{@agent-plane-name}",
    "#{@agent-plane-parent}",
    "#{@agent-plane-placed}",
    "#{?@agent-plane-task,1,0}",
];

fn parse_pane(line: &str) -> Result<Pane> {
    let f: Vec<&str> = line.split(SEP).collect();
    if f.len() != PANE_FIELDS.len() {
        bail!("unexpected list-panes line: {line:?}");
    }
    Ok(Pane {
        id: f[0].to_string(),
        pid: f[1].parse().context("pane_pid")?,
        session: f[2].to_string(),
        window: f[3].to_string(),
        window_index: f[4].parse().context("window_index")?,
        pane_index: f[5].parse().context("pane_index")?,
        cwd: f[6].to_string(),
        title: f[7].to_string(),
        active: f[8] == "1",
        viewed: f[9] == "1",
        sidebar: f[10] == "1",
        name: f[11].to_string(),
        parent: f[12].to_string(),
        placed: f[13] == "1",
        task: f[14] == "1",
    })
}

pub fn list_panes() -> Result<Vec<Pane>> {
    let format = PANE_FIELDS.join(&SEP.to_string());
    let panes = run(&["list-panes", "-a", "-F", &format])?
        .lines()
        .map(parse_pane)
        .collect::<Result<Vec<_>>>()?;
    Ok(physical_panes(panes))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What an attached client is showing.
pub struct ClientSize {
    pub size: Size,
    /// The `window-size` option of the window on its screen.
    pub sizing: String,
}

pub struct WindowSize {
    pub id: String,
    pub size: Size,
    pub zoomed: bool,
    /// The window's `window-size` option.
    pub sizing: String,
    pub name: String,
    /// `#{window_layout}`: the unzoomed layout, which `select-layout` takes back.
    pub layout: String,
}

#[derive(Default)]
pub struct Geometry {
    pub clients: Vec<ClientSize>,
    pub windows: Vec<WindowSize>,
}

const CLIENT_ROW: &str = "client\x1f#{window_width}\x1f#{window_height}\x1f#{window-size}";
const WINDOW_ROW: &str = "window\x1f#{window_id}\x1f#{window_width}\x1f#{window_height}\x1f#{window_zoomed_flag}\x1f#{window-size}\x1f#{window_layout}\x1f#{window_name}";

fn parse_size(width: &str, height: &str) -> Result<Size> {
    Ok(Size {
        width: width.parse().context("window_width")?,
        height: height.parse().context("window_height")?,
    })
}

/// The daemon's view of tmux each tick: panes and sizes from one tmux process, so reading the
/// sizes costs no extra process.
/// The plugin's global keys, from `@agent-plane-keys` and the prefix key bound to toggle them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GlobalKeys {
    pub off: bool,
    pub toggle: Option<String>,
}

const KEYS_OPTION: &str = "@agent-plane-keys";
const TOGGLE_OPTION: &str = "@agent-plane-bound-keys-toggle-key";

pub fn snapshot() -> Result<(Vec<Pane>, Geometry, GlobalKeys)> {
    let format = PANE_FIELDS.join(&SEP.to_string());
    let out = run(&[
        "list-panes",
        "-a",
        "-F",
        &format,
        ";",
        "list-clients",
        "-F",
        CLIENT_ROW,
        ";",
        "list-windows",
        "-a",
        "-F",
        WINDOW_ROW,
        ";",
        "show-options",
        "-gq",
        KEYS_OPTION,
        ";",
        "show-options",
        "-gq",
        TOGGLE_OPTION,
    ])?;
    let mut panes = Vec::new();
    let mut geometry = Geometry::default();
    let mut keys = GlobalKeys::default();
    for line in out.lines() {
        // show-options prints `name value`, quoting a value tmux would otherwise split.
        let option = |name: &str| {
            line.strip_prefix(name)?
                .strip_prefix(' ')
                .map(|value| value.trim_matches('"').to_string())
        };
        if let Some(value) = option(KEYS_OPTION) {
            keys.off = value == "off";
            continue;
        }
        if let Some(value) = option(TOGGLE_OPTION) {
            keys.toggle = Some(value);
            continue;
        }
        match line.split(SEP).collect::<Vec<_>>().as_slice() {
            ["client", width, height, sizing] => geometry.clients.push(ClientSize {
                size: parse_size(width, height)?,
                sizing: sizing.to_string(),
            }),
            ["window", id, width, height, zoomed, sizing, layout, name] => {
                geometry.windows.push(WindowSize {
                    id: id.to_string(),
                    size: parse_size(width, height)?,
                    zoomed: *zoomed == "1",
                    sizing: sizing.to_string(),
                    name: name.to_string(),
                    layout: layout.to_string(),
                })
            }
            _ => panes.push(parse_pane(line)?),
        }
    }
    Ok((physical_panes(panes), geometry, keys))
}

/// A linked window is listed once per session, but its processes belong to one
/// physical pane. Choose the first session by name and retain viewing through any link.
fn physical_panes(mut panes: Vec<Pane>) -> Vec<Pane> {
    panes.sort_by(|a, b| a.session.cmp(&b.session));
    let mut unique: Vec<Pane> = Vec::new();
    for pane in panes {
        if let Some(existing) = unique.iter_mut().find(|p| p.id == pane.id) {
            existing.viewed |= pane.viewed;
        } else {
            unique.push(pane);
        }
    }
    unique
}

pub struct Client {
    pub name: String,
    /// Unix seconds of the client's last keyboard or mouse input.
    pub activity: u64,
}

pub fn clients() -> Result<Vec<Client>> {
    run(&["list-clients", "-F", "#{client_name}\x1f#{client_activity}"])?
        .lines()
        .map(|line| {
            let (name, activity) = line
                .split_once(SEP)
                .with_context(|| format!("invalid list-clients row {line:?}"))?;
            Ok(Client {
                name: name.to_string(),
                activity: activity.parse().context("invalid client activity")?,
            })
        })
        .collect()
}

/// Spawned and moved agents chose their project; a marked pane is never placed automatically.
pub const PLACED_OPTION: &str = "@agent-plane-placed";

pub fn mark_placed(pane: &str) -> Result<()> {
    run(&["set-option", "-p", "-t", pane, PLACED_OPTION, "1"]).map(|_| ())
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The visible screen of a pane, with or without colour escapes.
pub fn capture(pane: &str, ansi: bool, lines: Option<u32>) -> Result<String> {
    let start = lines.map(|n| format!("-{n}"));
    let mut args = vec!["capture-pane", "-p", "-t", pane];
    if ansi {
        args.push("-e");
    } else {
        args.push("-J");
    }
    if let Some(start) = &start {
        args.extend(["-S", start]);
    }
    run(&args)
}

/// Make the most recently used client show this pane.
pub fn jump(session: &str, window: &str, pane: &str) -> Result<()> {
    run(&["switch-client", "-t", session])?;
    run(&["select-window", "-t", window])?;
    run(&["select-pane", "-t", pane])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_windows_share_one_process_identity_and_any_link_can_be_visible() {
        let first = Pane {
            id: "%1".into(),
            pid: 10,
            session: "alpha".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 0,
            cwd: "/tmp".into(),
            title: "agent".into(),
            active: true,
            viewed: false,
            sidebar: false,
            name: "agent".into(),
            parent: String::new(),
            placed: false,
            task: false,
        };
        let mut linked = first.clone();
        linked.session = "zeta".into();
        linked.window_index = 9;
        linked.viewed = true;
        let panes = physical_panes(vec![linked, first]);
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].session, "alpha");
        assert_eq!(panes[0].window_index, 1);
        assert!(panes[0].visible());
    }
}
