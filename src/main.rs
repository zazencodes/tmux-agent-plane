mod agents;
mod autoname;
mod autoname_config;
mod autoname_setup;
mod background;
mod bindings;
mod commands;
mod daemon;
mod detect;
mod diagnostics;
mod fit;
mod index;
mod lifecycle;
mod links;
mod navigation;
mod popup;
mod projects;
mod quota;
mod restore;
mod sessions;
mod sidebar;
mod sidebar_help;
mod tasks;
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
    /// Fuzzy-search agents with a live preview; Enter focuses the selected pane
    Ui {
        /// tmux client that opened the popup (supplied by the plugin)
        #[arg(long)]
        client: String,
        /// Open with the filter being typed
        #[arg(long)]
        search: bool,
    },
    /// Install the plugin's tmux bindings, or toggle them off and on
    #[command(hide = true)]
    Bindings {
        #[arg(long)]
        toggle: bool,
    },
    /// Run the always-on agent list (shown in the sidebar pane)
    Sidebar,
    /// Show a daemon message until a key press or 10 seconds (shown in a popup)
    #[command(hide = true)]
    Notice,
    /// Open the sidebar in every window and move into it; close it everywhere if it is open
    ToggleSidebar {
        /// tmux window id or target, e.g. `@104` or `work:2`
        window: String,
    },
    /// Move into the window's sidebar, opening it everywhere if it is closed
    EnterSidebar {
        /// tmux window id or target, e.g. `@104` or `work:2`
        window: String,
    },
    /// Add the sidebar to a window while it is open (run by the plugin when a window is created)
    FollowSidebar {
        /// tmux window id or target
        window: String,
    },
    /// List running agents (`*` marks the caller)
    List {
        #[arg(long)]
        json: bool,
    },
    /// Read how much of each AI subscription is left (Claude, Codex, Antigravity, Grok Build)
    Quota {
        #[arg(long)]
        json: bool,
    },
    /// Inspect or create tmux sessions (each session is a project)
    Sessions {
        #[command(subcommand)]
        action: sessions::Action,
    },
    /// Move a running agent into an existing project without restarting it
    Move {
        /// Agent name, pane id, or `self` when caller identity is available
        target: String,
        /// Exact destination session name or id
        #[arg(long)]
        session: String,
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
    /// Focus the next or previous agent without entering the sidebar
    Cycle {
        #[arg(value_enum)]
        direction: navigation::Direction,
        /// Exact tmux client name, supplied by the invoking key binding
        #[arg(long)]
        client: String,
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
    /// Run long-lived commands (dev servers, watchers, tunnels) in the background, shared by every agent
    Task {
        #[command(subcommand)]
        action: tasks::Action,
    },
    /// Bring back agents from before tmux restarted: lists them; --tasks or --no-tasks restores
    Restore {
        /// Restore the agents and the tasks that were running
        #[arg(long, group = "restore")]
        tasks: bool,
        /// Restore the agents only
        #[arg(long, group = "restore")]
        no_tasks: bool,
        /// Forget them without restoring
        #[arg(long, group = "restore")]
        discard: bool,
    },
    /// Start an agent in a new pane, window or session
    Spawn {
        kind: index::Kind,
        /// The first prompt (required for agy)
        task: Option<String>,
        #[arg(long = "in", value_enum, default_value = "pane")]
        placement: commands::Placement,
        /// Exact existing session for --in window; new session name for --in session
        #[arg(long)]
        session: Option<String>,
        /// Working directory (default: current directory)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// Agent name (default: <kind>-<pane number>)
        #[arg(long)]
        name: Option<String>,
        /// The calling Codex agent's own name, recorded as the parent (required from Codex)
        #[arg(long)]
        parent: Option<String>,
        /// Wait until the agent finishes its first turn or blocks
        #[arg(long)]
        wait: bool,
        /// Give up waiting after this many seconds
        #[arg(long, requires = "wait")]
        timeout: Option<u64>,
        /// Arguments after `--`, passed verbatim to the agent CLI
        #[arg(last = true, value_name = "AGENT_ARGS")]
        agent_args: Vec<String>,
    },
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Cmd::Daemon { action } => lifecycle::run(action),
        Cmd::Ui { client, search } => ui::run(&client, search),
        Cmd::Bindings { toggle } => bindings::apply(toggle),
        Cmd::Sidebar => sidebar::run(),
        Cmd::Notice => popup::notice(),
        Cmd::ToggleSidebar { window } => sidebar::toggle(&commands::target_pane(&window)?),
        Cmd::EnterSidebar { window } => sidebar::enter(&commands::target_pane(&window)?),
        Cmd::FollowSidebar { window } => sidebar::follow(&commands::target_pane(&window)?),
        Cmd::List { json } => commands::list(json),
        Cmd::Quota { json } => quota::command(json),
        Cmd::Sessions { action } => sessions::run(action),
        Cmd::Move { target, session } => commands::move_agent(&target, &session),
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
        Cmd::Cycle { direction, client } => navigation::cycle(direction, &client),
        Cmd::Kill { target } => commands::kill(&target),
        Cmd::Wait { target, timeout } => commands::wait(&target, timeout),
        Cmd::Task { action } => tasks::run(action),
        Cmd::Restore {
            tasks,
            no_tasks,
            discard,
        } => restore::run_cli(tasks, no_tasks, discard),
        Cmd::Spawn {
            kind,
            task,
            placement,
            session,
            cwd,
            name,
            parent,
            wait,
            timeout,
            agent_args,
        } => commands::spawn(commands::Spawn {
            kind,
            placement,
            session,
            cwd,
            name,
            parent,
            task,
            agent_args,
            wait,
            timeout,
        }),
    }
}
