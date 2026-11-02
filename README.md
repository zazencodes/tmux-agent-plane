# tmux-agent-plane

<p align="center">
  <a href="https://github.com/zazencodes/tmux-agent-plane/actions/workflows/ci.yml"><img src="https://github.com/zazencodes/tmux-agent-plane/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/zazencodes/tmux-agent-plane/releases/latest"><img src="https://img.shields.io/github/v/release/zazencodes/tmux-agent-plane" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPLv3-blue" alt="License: GPLv3"></a>
  <img src="https://img.shields.io/badge/tmux-3.3%2B-1BB91F?logo=tmux&logoColor=white" alt="tmux 3.3+">
  <img src="https://img.shields.io/badge/Rust-000000?logo=rust&logoColor=white" alt="Rust">
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Claude%20Code-ready-D97757?logo=claude&logoColor=white" alt="Claude Code ready">
  <img src="https://img.shields.io/badge/Codex-ready-000000?logo=data:image/svg%2bxml;base64,PHN2ZyBmaWxsPSJ3aGl0ZSIgcm9sZT0iaW1nIiB2aWV3Qm94PSIwIDAgMjQgMjQiIHhtbG5zPSJodHRwOi8vd3d3LnczLm9yZy8yMDAwL3N2ZyI%2BPHRpdGxlPk9wZW5BSTwvdGl0bGU%2BPHBhdGggZD0iTTIyLjI4MTkgOS44MjExYTUuOTg0NyA1Ljk4NDcgMCAwIDAtLjUxNTctNC45MTA4IDYuMDQ2MiA2LjA0NjIgMCAwIDAtNi41MDk4LTIuOUE2LjA2NTEgNi4wNjUxIDAgMCAwIDQuOTgwNyA0LjE4MThhNS45ODQ3IDUuOTg0NyAwIDAgMC0zLjk5NzcgMi45IDYuMDQ2MiA2LjA0NjIgMCAwIDAgLjc0MjcgNy4wOTY2IDUuOTggNS45OCAwIDAgMCAuNTExIDQuOTEwNyA2LjA1MSA2LjA1MSAwIDAgMCA2LjUxNDYgMi45MDAxQTUuOTg0NyA1Ljk4NDcgMCAwIDAgMTMuMjU5OSAyNGE2LjA1NTcgNi4wNTU3IDAgMCAwIDUuNzcxOC00LjIwNTggNS45ODk0IDUuOTg5NCAwIDAgMCAzLjk5NzctMi45MDAxIDYuMDU1NyA2LjA1NTcgMCAwIDAtLjc0NzUtNy4wNzI5em0tOS4wMjIgMTIuNjA4MWE0LjQ3NTUgNC40NzU1IDAgMCAxLTIuODc2NC0xLjA0MDhsLjE0MTktLjA4MDQgNC43NzgzLTIuNzU4MmEuNzk0OC43OTQ4IDAgMCAwIC4zOTI3LS42ODEzdi02LjczNjlsMi4wMiAxLjE2ODZhLjA3MS4wNzEgMCAwIDEgLjAzOC4wNTJ2NS41ODI2YTQuNTA0IDQuNTA0IDAgMCAxLTQuNDk0NSA0LjQ5NDR6bS05LjY2MDctNC4xMjU0YTQuNDcwOCA0LjQ3MDggMCAwIDEtLjUzNDYtMy4wMTM3bC4xNDIuMDg1MiA0Ljc4MyAyLjc1ODJhLjc3MTIuNzcxMiAwIDAgMCAuNzgwNiAwbDUuODQyOC0zLjM2ODV2Mi4zMzI0YS4wODA0LjA4MDQgMCAwIDEtLjAzMzIuMDYxNUw5Ljc0IDE5Ljk1MDJhNC40OTkyIDQuNDk5MiAwIDAgMS02LjE0MDgtMS42NDY0ek0yLjM0MDggNy44OTU2YTQuNDg1IDQuNDg1IDAgMCAxIDIuMzY1NS0xLjk3MjhWMTEuNmEuNzY2NC43NjY0IDAgMCAwIC4zODc5LjY3NjVsNS44MTQ0IDMuMzU0My0yLjAyMDEgMS4xNjg1YS4wNzU3LjA3NTcgMCAwIDEtLjA3MSAwbC00LjgzMDMtMi43ODY1QTQuNTA0IDQuNTA0IDAgMCAxIDIuMzQwOCA3Ljg3MnptMTYuNTk2MyAzLjg1NThMMTMuMTAzOCA4LjM2NCAxNS4xMTkyIDcuMmEuMDc1Ny4wNzU3IDAgMCAxIC4wNzEgMGw0LjgzMDMgMi43OTEzYTQuNDk0NCA0LjQ5NDQgMCAwIDEtLjY3NjUgOC4xMDQydi01LjY3NzJhLjc5Ljc5IDAgMCAwLS40MDctLjY2N3ptMi4wMTA3LTMuMDIzMWwtLjE0Mi0uMDg1Mi00Ljc3MzUtMi43ODE4YS43NzU5Ljc3NTkgMCAwIDAtLjc4NTQgMEw5LjQwOSA5LjIyOTdWNi44OTc0YS4wNjYyLjA2NjIgMCAwIDEgLjAyODQtLjA2MTVsNC44MzAzLTIuNzg2NmE0LjQ5OTIgNC40OTkyIDAgMCAxIDYuNjgwMiA0LjY2ek04LjMwNjUgMTIuODYzbC0yLjAyLTEuMTYzOGEuMDgwNC4wODA0IDAgMCAxLS4wMzgtLjA1NjdWNi4wNzQyYTQuNDk5MiA0LjQ5OTIgMCAwIDEgNy4zNzU3LTMuNDUzN2wtLjE0Mi4wODA1TDguNzA0IDUuNDU5YS43OTQ4Ljc5NDggMCAwIDAtLjM5MjcuNjgxM3ptMS4wOTc2LTIuMzY1NGwyLjYwMi0xLjQ5OTggMi42MDY5IDEuNDk5OHYyLjk5OTRsLTIuNTk3NCAxLjQ5OTctMi42MDY3LTEuNDk5N1oiLz48L3N2Zz4%3D" alt="Codex ready">
  <a href="https://zazencodes.com/"><img src="https://img.shields.io/badge/made%20by-ZazenCodes-black" alt="Made by ZazenCodes"></a>
</p>

See and steer every coding agent running in your tmux.

Start Claude Code or Codex in any tmux pane, the way you already do. tmux-agent-plane finds it, tracks whether it is working, waiting on you, or done, and shows that in your status bar and in an agent list. Your agents get the same view through a CLI and an agent skill, so any agent can check on, message, wait for or start other agents, and one agent can run the rest as a super agent.

No hooks, no wrappers, nothing to configure in your agents.

```text
 tmux status bar                                    ◆ 1 ✓ 1 ● 2 ○ 1   ~/pro/app

┌ agents · 5 ──────────────────────────────────────────────────────────────────┐
│◆ reviewer      blocked  12s  work:2.2   Review the auth diff                 │
│✓ claude-154    done      3m  work:1.1   Add rate limiting to the API         │
│● codex-164     working  41s  work:3.1   Migrate the billing tests | app      │
│● docs-writer   working   2m  docs:1.1   Document the new endpoints           │
│○ claude-163    idle      9m  site:1.1   Landing page copy                    │
└──────────────────────────────────────────────────────────────────────────────┘
┌ reviewer · input needed · /Users/you/pro/app ────────────────────────────────┐
│ Do you want to apply this change to src/auth.rs?                             │
│ ❯ 1. Yes                                                                     │
│   2. No, and tell Claude what to do differently                              │
└──────────────────────────────────────────────────────────────────────────────┘
enter jump · j/k move · q quit
```

## Features

- **Every agent, every session.** One daemon watches every pane in every tmux session and keeps a single index of the agents in them: name, state, task, location and working directory.
- **Know what needs you.** The status bar counts agents by state. An agent that finishes or blocks while you are looking elsewhere sends a tmux message and a desktop notification, and stays `done` until you look at it.
- **A sidebar of agents.** `prefix + S` opens a live list of every agent down the left of your window: state, time in that state, task and location. `Enter` jumps to an agent, and the sidebar stays.
- **Peek before you jump.** `prefix + A` opens the agent list in a popup with a large live preview of the selected agent's screen.
- **Agents that know about each other.** The `tmux-agent-plane` CLI and agent skill let any agent list, read, message, answer, wait for, show, close and spawn other agents.
- **Super agents.** One agent can spawn helpers in new panes, windows or sessions, hand each a task, wait for the results and report back. Each spawned agent records its parent.
- **Your agents, unchanged.** Spawned agents start in a normal shell, so your aliases and flags apply, and the shell stays when the agent exits.

## States

| State | Meaning |
| --- | --- |
| ● `working` | running a turn |
| ◆ `blocked` | waiting on you: a question, a permission prompt, a dialog |
| ✓ `done` | finished a turn you have not looked at yet |
| ○ `idle` | ready for input, already seen |

## Requirements

- tmux 3.3 or newer
- Claude Code and/or Codex CLI
- Rust, to build it
- macOS or Linux. Desktop notifications use `osascript` on macOS and `notify-send` on Linux.

## Install

**1. The binary.**

```sh
cargo install --locked --git https://github.com/zazencodes/tmux-agent-plane
```

**2. The tmux plugin.** With [TPM](https://github.com/tmux-plugins/tpm), add to `~/.tmux.conf` and press `prefix + I`:

```tmux
set -g @plugin 'zazencodes/tmux-agent-plane'
```

Without TPM, clone the repo and add `run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux` to `~/.tmux.conf`.

The plugin starts the daemon, binds `prefix + S` to the sidebar and `prefix + A` to the agent list popup.

**3. The status bar.** Add `#{@agent_plane_status}` to `status-right` (or `status-left`):

```tmux
set -g status-right "#{@agent_plane_status}%H:%M"
```

**4. The skill.** Link it into each agent's skills folder. With TPM the repo is in `~/.tmux/plugins/tmux-agent-plane`:

```sh
ln -s ~/.tmux/plugins/tmux-agent-plane/skill/tmux-agent-plane ~/.claude/skills/tmux-agent-plane   # Claude Code
ln -s ~/.tmux/plugins/tmux-agent-plane/skill/tmux-agent-plane ~/.agents/skills/tmux-agent-plane   # Codex
```

**5. Tell your agents.** Agents reach for raw tmux commands out of habit, even with the skill installed. One line in your global `~/.claude/CLAUDE.md` and `~/.codex/AGENTS.md` fixes that:

```markdown
- Coding agents in my tmux are indexed by `tmux-agent-plane`. Whenever a task touches another agent (checking on, reading, messaging, waiting for, showing, closing or spawning one), load the `tmux-agent-plane` skill first and use its CLI; never drive agent panes with raw tmux commands.
```

## Usage

Run your agents as usual. Then ask any of them things like:

- "What agents are running?"
- "How is codex-164 doing?"
- "Answer reviewer's question: yes, apply it."
- "Spawn a Codex agent in a new window to migrate the billing tests, and tell me when it's done."
- "You're the super agent: split this refactor across three agents, one per package, and review their work."

### Sidebar and agent list

| Key | Action |
| --- | --- |
| `prefix + S` | Open or close the sidebar in the current window |
| `prefix + A` | Open the agent list popup, with a preview |
| `j` `k` / `↓` `↑` | Move |
| `Enter` | Jump to the agent. The sidebar stays open; the popup closes |
| `q` | Close (also `Esc` in the popup) |

The sidebar belongs to the window it was opened in; open one in each window you work from. The popup opens on the first blocked agent, or else the first done one. The agent you are looking at is shown in bold in the sidebar.

### CLI

| Command | Action |
| --- | --- |
| `list [--json]` | All agents. `*` (or `"self": true`) marks the caller |
| `read <agent> [--lines N]` | The agent's screen, plus N lines of scrollback |
| `send <agent> "<prompt>" [--wait] [--timeout S]` | Paste a prompt and press Enter. `--wait` returns when that turn ends or the agent blocks |
| `keys <agent> <key>...` | Press keys, by tmux key name: `Enter`, `Escape`, `Down`, `2` |
| `wait <agent> [--timeout S]` | Wait until the agent is not working |
| `focus <agent>` | Show the agent on your screen |
| `kill <agent>` | Close the agent's pane |
| `spawn claude\|codex ["<task>"] [--in pane\|window\|session] [--cwd DIR] [--name NAME] [--wait] [--timeout S]` | Start an agent. `--in pane` (default) splits the caller's pane |
| `toggle-sidebar <window>` | Open or close the sidebar in a window (a window or pane id) |
| `sidebar` | The sidebar's agent list, run in the sidebar pane |
| `ui` | The agent list popup |
| `daemon` | The watcher, started by the plugin |

`<agent>` is an agent's name or tmux pane id. Agents are named `<kind>-<pane number>` (`claude-154`) unless spawned with `--name`. `read` and `keys` also take any pane id, so an agent stuck on a prompt before it starts can still be answered.

## How it works

- **Claude Code** keeps `~/.claude/sessions/<pid>.json` for each running session, with its tmux pane and a `status`: `busy`, `waiting` (with `waitingFor`, such as `input needed`), `idle`, or `shell` (the turn is over and a background shell runs on).
- **Codex** sets the terminal title: a braille spinner in front while a turn runs, `[ ! ] Action Required` while an approval or question waits. A pane is a Codex agent when a `codex` process runs under it.
- **The daemon** reads both once a second for every pane on the tmux server. It marks a turn that ends off screen as `done` until the pane is on screen, writes the index to `~/.local/state/tmux-agent-plane/agents.json`, sets the `@agent_plane_status` tmux option, and sends notifications. It exits with the tmux server, and a second copy exits at once.
- **The CLI and the agent list** read the index. They fail with a clear error when the daemon is not running.
- **`send`** pastes with bracketed paste, then presses Enter, so a multi-line prompt arrives as one message.

## Notes

- A Claude Code session is detected once it writes its session file, after the folder trust prompt. When `spawn` reports that an agent did not start, read its pane and answer the prompt with `keys`.
- Codex detection relies on its default terminal title. A `tui.terminal_title` setting that drops the activity spinner hides the `working` state.
- Claude Code's session file is internal to Claude Code and can change between releases. The daemon stops with an error on a status it does not know.
- After upgrading, restart the daemon: `pkill -f 'tmux-agent-plane daemon'`, then reload your tmux config.
- Only the default tmux server is watched.

## Changelog

See [CHANGELOG.md](CHANGELOG.md). Releases are tagged `vX.Y.Z`.

## License

[GPLv3](LICENSE). You can use, modify and share this code, and any version you distribute must also be released under GPLv3.

<p align="center">
  <a href="https://zazencodes.com/?utm_source=github&utm_medium=referral&utm_campaign=tmux-agent-plane">
    <img
      src="docs/assets/zazencodes-banner.png"
      alt="ZazenCodes — Engineering for the Agentic Era"
      width="100%"
    >
  </a>
  <br>
  Created by <a href="https://zazencodes.com/">ZazenCodes</a>
</p>
