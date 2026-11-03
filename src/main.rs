mod agents;
mod commands;
mod daemon;
mod detect;
mod diagnostics;
mod index;
mod lifecycle;
mod popup;
mod sidebar;
mod tmux;
mod ui;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// See and steer every coding agent running in tmux.
#[derive(Parser)]
#[command(name = "tmux-agent-plane", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start, restart or stop the detached tmux watcher
    Daemon {
        #[command(subcommand)]
        action: lifecycle::Action,
    },
    /// Open the agent list with a live preview; Enter jumps to the agent
    Ui,
    /// Run the always-on agent list (shown in the sidebar pane)
    Sidebar,
    /// Open the sidebar in every window and move into it; close it everywhere if it is open
    ToggleSidebar {
        /// tmux window id or target, e.g. `@104` or `work:2`
        window: String,
    },
    /// Add the sidebar to a window while it is open (run by the plugin when a window comes on screen)
    FollowSidebar {
        /// tmux window id or target
        window: String,
    },
    /// List running agents (`*` marks the caller)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Print an agent's recent screen
    Read {
        /// Agent name or pane id
        target: String,
        /// Lines of scrollback to include above the visible screen
        #[arg(long, default_value_t = 0)]
        lines: u32,
    },
    /// Paste a prompt and press Enter; blocked agents require a deliberate --answer
    Send {
        /// Agent name or pane id
        target: String,
        text: String,
        /// Deliberately answer a blocked agent (fails if the target is not blocked)
        #[arg(long)]
        answer: bool,
        /// Wait until the agent finishes the turn or blocks
        #[arg(long)]
        wait: bool,
        /// Give up waiting after this many seconds
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
    },
    /// Press keys in an agent's pane (tmux key names: Enter, Escape, Up, Down, 1, y ...)
    Keys {
        /// Agent name or pane id
        target: String,
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Give an agent a name
    Rename {
        /// Agent name, pane id, or `self` when caller identity is available
        target: String,
        /// One word, e.g. `reviewer` or `auth-refactor`
        name: String,
    },
    /// Show an agent on the user's screen
    Focus {
        /// Agent name or pane id
        target: String,
    },
    /// Close an agent's pane, ending the agent
    Kill {
        /// Agent name or pane id
        target: String,
    },
    /// Wait until an agent is not working (done, idle or blocked)
    Wait {
        /// Agent name or pane id
        target: String,
        /// Give up after this many seconds
        #[arg(long)]
        timeout: Option<u64>,
    },
    /// Start an agent in a new pane, window or session
    Spawn {
        kind: index::Kind,
        /// The first prompt (required for agy)
        task: Option<String>,
        #[arg(long = "in", value_enum, default_value = "pane")]
        placement: commands::Placement,
        /// Working directory (default: current directory)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Agent name (default: <kind>-<pane number>)
        #[arg(long)]
        name: Option<String>,
        /// Wait until the agent finishes its first turn or blocks
        #[arg(long)]
        wait: bool,
        /// Give up waiting after this many seconds
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Cmd::Daemon { action } => lifecycle::run(action),
        Cmd::Ui => ui::run(),
        Cmd::Sidebar => sidebar::run(),
        Cmd::ToggleSidebar { window } => sidebar::toggle(&commands::target_pane(&window)?),
        Cmd::FollowSidebar { window } => sidebar::follow(&commands::target_pane(&window)?),
        Cmd::List { json } => commands::list(json),
        Cmd::Read { target, lines } => commands::read(&target, lines),
        Cmd::Send {
            target,
            text,
            answer,
            wait,
            timeout,
        } => commands::send(&target, &text, answer, wait, timeout),
        Cmd::Keys { target, keys } => commands::keys(&target, &keys),
        Cmd::Rename { target, name } => commands::rename(&target, &name),
        Cmd::Focus { target } => commands::focus(&target),
        Cmd::Kill { target } => commands::kill(&target),
        Cmd::Wait { target, timeout } => commands::wait(&target, timeout),
        Cmd::Spawn {
            kind,
            task,
            placement,
            cwd,
            name,
            wait,
            timeout,
        } => commands::spawn(commands::Spawn {
            kind,
            placement,
            cwd,
            name,
            task,
            wait,
            timeout,
        }),
    }
}
