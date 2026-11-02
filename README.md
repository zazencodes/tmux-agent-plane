# tmux-agent-plane

See and steer every coding agent running in your tmux.

Start Claude Code or Codex in any tmux pane as you always do. tmux-agent-plane finds it, tracks whether it is working, blocked on you, or done, and shows that in your status bar and in an agent list. Every agent gets the same view through a CLI and an agent skill, so any agent can check on, message, wait for or start other agents. One agent can run the rest as a super agent.

No hooks, no wrappers, no config in your agents.

```
┌ agents · 4 ───────────────────────────────────────────────────────────┐
│◆ reviewer    blocked   12s  work:2.2   Review the auth diff           │
│✓ claude-154  done       3m  work:1.1   Tmux agent monitoring extension │
│● codex-164   working   41s  work:3.1   Prepare and post approved shorts│
│○ claude-163  idle       9m  site:1.1   Landing page copy               │
└───────────────────────────────────────────────────────────────────────┘
┌ reviewer · input needed · ~/pro/app ──────────────────────────────────┐
│ live preview of the selected agent's pane                             │
```

## States

| State | Meaning |
|---|---|
| `working` ● | running a turn |
| `blocked` ◆ | waiting on you: a question, a permission prompt, a dialog |
| `done` ✓ | finished a turn you have not looked at yet |
| `idle` ○ | ready for input, already seen |

An agent that finishes while its pane is off screen is `done` until you look at it. Turning `done` or `blocked` off screen sends a tmux message and a desktop notification.

## How it detects agents

A daemon polls once a second and reads what the agents already publish:

- **Claude Code** keeps `~/.claude/sessions/<pid>.json` for each running session, with its tmux pane and a `status` of `busy`, `waiting` (with `waitingFor`), `idle` or `shell` (a background shell runs on after the turn).
- **Codex** sets the terminal title: a braille spinner in front while a turn runs, `[ ! ] Action Required` while an approval or question waits. A pane is a Codex agent when a `codex` process runs under it.

The daemon writes everything to one index, `~/.local/state/tmux-agent-plane/agents.json`, which the CLI and the agent list read.

## Install

Requires tmux 3.3+ and Rust.

```bash
cargo install --path .
```

Then in `~/.tmux.conf`, start the plugin and add the status segment:

```tmux
set -g status-right "#{@agent_plane_status}#{E:@catppuccin_status_directory}"
run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux
```

The plugin binds `prefix + A` to the agent list (Enter jumps to the agent) and starts the daemon.

Install the skill for your agents by copying or linking `skill/tmux-agent-plane` into their skills folder (`~/.claude/skills/`, `~/.agents/skills/`).

## CLI

```bash
tmux-agent-plane list [--json]
tmux-agent-plane read <agent> [--lines N]
tmux-agent-plane send <agent> "<prompt>" [--wait] [--timeout S]
tmux-agent-plane keys <agent> <key>...
tmux-agent-plane wait <agent> [--timeout S]
tmux-agent-plane spawn claude|codex ["<task>"] [--in pane|window|session] [--cwd DIR] [--name NAME] [--wait] [--timeout S]
tmux-agent-plane ui
tmux-agent-plane daemon
```

`<agent>` is an agent's name or tmux pane id. `spawn` types the agent command into a new shell, so your shell aliases apply, and records the caller as the new agent's parent.

## Limits

- A Claude Code session is not detected until it writes its session file, which happens after the folder trust prompt.
- Codex detection relies on its default terminal title. A `tui.terminal_title` setting that drops the activity spinner hides the `working` state.
- Claude Code's session file is internal to Claude Code and may change between releases. The daemon fails loudly on a status it does not know.
- One tmux server: the default socket.
