# tmux-agent-plane

<p align="center">
  <a href="https://github.com/zazencodes/tmux-agent-plane/actions/workflows/ci.yml"><img src="https://github.com/zazencodes/tmux-agent-plane/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://crates.io/crates/tmux-agent-plane"><img src="https://img.shields.io/crates/v/tmux-agent-plane" alt="crates.io"></a>
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

Start Claude Code, Codex or Antigravity (`agy`) in any tmux pane, the way you already do. Add Pi, OpenCode or Grok Build with the opt-in integrations below. tmux-agent-plane finds it, tracks whether it is working, waiting on you, or done, and shows that in your status bar and in an agent list. Your agents get the same view through a CLI and an agent skill, so any agent can check on, message, wait for or start other agents, and one agent can run the rest as a super agent.

Claude Code, Codex and Antigravity use native signals. Pi and OpenCode use publishers built on their official extension APIs; Grok Build uses its official leader mode. Each optional integration has one setup step under [More agents](#more-agents).

<p align="center">
  <img src="docs/assets/screenshot.png" alt="The agent sidebar on the left of a tmux window, next to a Codex agent" width="700">
</p>

## Features

- **Every agent, every session.** One daemon watches every pane in every tmux session and keeps a single index of the agents in them: name, state, task, location and working directory.
- **Know what needs you.** The status bar counts agents by state. An agent that finishes or blocks while you are looking elsewhere shows a five-second notice in every open sidebar footer and sends a desktop notification, and stays `done` until you look at it.
- **A sidebar of agents.** Your sidebar key (default `prefix + S`) opens a live list of every agent down the left of every window, in every session, and puts you in it: state, time in that state, task and location. `Enter` jumps to an agent, and the sidebar is there too. Close it once and it closes everywhere.
- **Flip through your agents.** Alt + K / J (Option + K / J on macOS) goes straight to the previous or next agent from anywhere, across sessions, with no prefix. Tap it repeatedly to scroll through them. See [Direct agent navigation](#direct-agent-navigation).
- **Search and peek before you jump.** `prefix + a` opens an agent picker with a large live preview and the sidebar's actions; `prefix + f` opens it ready to type a filter. Browse with `j`/`k`, press `/` then `n`/`N` to hop between matching rows, press `f` to fuzzy-filter names, projects and descriptions across every session, and press Enter to focus.
- **Agents that know about each other.** The `tmux-agent-plane` CLI and agent skill let any agent list, read, message, answer, wait for, show, close and spawn other agents.
- **Names you can read.** Agents name themselves after their task when they use the skill, name the agents they spawn, and you can rename any agent from the sidebar with `r`. Optional AI naming can name the remaining default-named agents; press `c` in the sidebar to set it up.
- **Super agents.** One agent can spawn helpers in new panes, windows or sessions, hand each a task, wait for the results and report back. A spawned agent records its parent when caller identity is known.
- **Sessions are projects.** Inspect and create named project sessions, start helpers directly in them, and move running agent panes between projects while their processes, names and parent links stay intact.
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
- At least one supported agent CLI: Claude Code, Codex, Antigravity, Pi, OpenCode or Grok Build
- `lsof` for Antigravity process/session association (included with macOS; install it on Linux when using agy)
- Rust and a C compiler, to build it (SQLite is bundled)
- macOS or Linux. Desktop notifications use `osascript` on macOS and `notify-send` on Linux.

## Install

**1. The binary.**

```sh
cargo install --locked tmux-agent-plane
```

**2. The tmux plugin.** With [TPM](https://github.com/tmux-plugins/tpm), add to `~/.tmux.conf` and press `prefix + I`:

```tmux
set -g @plugin 'zazencodes/tmux-agent-plane'
```

Without TPM, clone the repo and add `run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux` to `~/.tmux.conf`.

The plugin starts the daemon once the first session exists, binds `prefix + S` to the sidebar `prefix + a` to the agent list popup and `prefix + f` to the same popup already filtering (replacing tmux's window finder on that key), and sets a `session-created[90]` hook to start the watcher and a `window-linked[90]` hook that gives each new window the open sidebar. A `window-layout-changed[90]` hook closes a sidebar as soon as its last other pane disappears. A `pane-mode-changed[90]` hook takes a sidebar straight back out of copy mode, which would otherwise freeze its list. To use another sidebar key, set it before the plugin loads:

```tmux
set -g @agent-plane-sidebar-key Space   # replaces tmux's next-layout binding
```

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

## More agents

Tested with Pi 1.0.2, OpenCode 1.19.3 and Grok Build 1.0.70. Pi/OpenCode must inherit the daemon's `XDG_STATE_HOME`.

These integrations are independent. Install only the ones you use; absent publisher files or a Grok leader socket leave that kind untracked. The paths below assume a TPM checkout at `~/.tmux/plugins/tmux-agent-plane`.

- **Pi:** symlink its publisher into Pi's extension directory, then restart Pi:

  ```sh
  mkdir -p "${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/extensions"
  ln -s "$HOME/.tmux/plugins/tmux-agent-plane/extensions/pi" "${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/extensions/tmux-agent-plane"
  ```

- **OpenCode:** symlink the TUI publisher plugin and add `"./tmux-agent-plane.js"` to the `plugin` array in `~/.config/opencode/tui.json` (or your `$XDG_CONFIG_HOME/opencode/tui.json`), preserving existing entries. Restart OpenCode. [Example configuration](extensions/opencode/tui.example.json).

  ```sh
  mkdir -p "${XDG_CONFIG_HOME:-$HOME/.config}/opencode"
  ln -s "$HOME/.tmux/plugins/tmux-agent-plane/extensions/opencode/tmux-agent-plane.js" "${XDG_CONFIG_HOME:-$HOME/.config}/opencode/tmux-agent-plane.js"
  ```

- **Grok Build (macOS):** add `use_leader = true` to the existing `[cli]` table in `~/.grok/config.toml` (or `$GROK_HOME/config.toml`), then restart Grok sessions. [Example configuration](extensions/grok/config.toml.example). The daemon uses the same `GROK_HOME` and `GROK_LEADER_SOCKET` overrides as Grok; its default socket is `~/.grok/leader.sock`.

## Usage

Run your agents as usual. Then ask any of them things like:

- "What agents are running?"
- "How is codex-164 doing?"
- "Answer reviewer's question: yes, apply it."
- "Spawn a Codex agent in a new window to migrate the billing tests, and tell me when it's done."
- "You're the super agent: split this refactor across three agents, one per package, and review their work."

### Direct agent navigation

Press **Alt + K / J** (Option + K / J on macOS) to focus the previous or next agent directly, without entering the sidebar or pressing the tmux prefix. The keys work with the sidebar open or closed, follow its global order across sessions, and wrap at either end. Sidebar filters do not limit this navigation. From a shell or sidebar pane, J starts at the first agent and K at the last.

The binding targets the exact tmux client that received the key. It refuses a destination session or linked window another client is using. To choose other direct bindings, set these before the plugin loads, then reload the plugin:

```tmux
set -g @agent-plane-previous-key C-M-k
set -g @agent-plane-next-key C-M-j
```

These are tmux key names (`C` is Ctrl, `M` is Alt). The defaults leave tmux's prefix-then-Alt-arrow bindings available for resizing. Your window manager and terminal must pass the chosen keys through to tmux.

Press **Alt + H** to move into the sidebar, opening it if it is closed; from inside, it stays put. Press **Alt + L** to leave it for the selected agent, as `Enter` does, even halfway through typing a search. Everywhere else Alt + L goes to the pane as usual. To choose other keys:

```tmux
set -g @agent-plane-sidebar-in-key C-M-h
set -g @agent-plane-sidebar-out-key C-M-l
```

Inside the sidebar, Shift + Right always leaves it the same way, with nothing to configure. To go in with Shift + Left, add this after the plugin loads:

```tmux
bind -n S-Left run-shell "tmux-agent-plane enter-sidebar '#{window_id}'"
```

### Sidebar and agent list

| Key | Action |
| --- | --- |
| Sidebar key (default `prefix + S`) | Open the sidebar in every window and move into it; close it everywhere when it is open |
| `prefix + a` | Open the agent picker: the sidebar's list and actions, with a live screen preview |
| `prefix + f` | Open the agent picker already filtering, so you can start typing |
| `Alt + ↑` / `Alt + ↓` | Focus previous/next agent directly, across sessions; no prefix or sidebar focus needed |
| `Alt + H` / `Alt + L` | Move into the sidebar and stay / leave it for the selected agent; no prefix needed |
| `↓` `↑` | Move selection. In the sidebar, moving previews the selected agent's window/session while keyboard input stays in the sidebar; the popup previews inside itself. Both also accept `j` / `k` |
| `gg` / `G` | Move to the first / last agent in the list, previewing it like any other move |
| `s` | Re-sort agents within each session by attention, then longest in state; keep the selected pane |
| `/` | Search, as in Vim: type part of an agent's name or the task on its row, ignoring case. The selection moves to the next match as you type, every row stays listed, and every match is highlighted; a match past the end of a cut-off line lights up its `…`. `Enter` keeps the search and previews the match, `Esc` or `Ctrl-C` cancels it and returns to where you were, and clears a kept search, `Ctrl-W` deletes a word, `Ctrl-U` clears. Project names are not searched |
| `n` / `N` | With a search kept, go to the next / previous match, wrapping and previewing it like any other move. `n` is the new-agent key again once `Esc` clears the search or `Enter` opens an agent |
| `f` | Filter, in the sidebar and the popup alike: hide rows that do not match, fuzzy across agent name, project (tmux session), task and blocked detail, ignoring case and allowing gaps. Each word can match any field, in any order, and the best match comes first. `Enter` opens the selected match and ends the filter, `/` freezes the filter so keys are commands on its matches, `f` resumes editing it, `Esc` cancels the edit, `Ctrl-W` deletes a word, `Ctrl-U` clears; arrows and `Ctrl-J`/`Ctrl-K` move selection while editing |
| `d` | Cycle state filter: All → Blocked → Done → Working → Idle → All, skipping states with no filter matches when pressed |
| `Ctrl-J` / `Ctrl-K` | Move selection, including while editing a filter |
| `Esc` | Clear the search, then the filter, outside editing; then close the popup |
| `Enter` | Jump to the agent. The sidebar stays open; the popup closes. When the list says the daemon is not running, `Enter` restarts it and shows the reason if it cannot start |
| `x` | Ask to kill the selected agent’s pane. The footer pins its name/state; `Enter` or lowercase `y` confirms; `?` temporarily shows help; other keys cancel without replay. Refuses to end your attached session; warns if a remote session would end |
| `r` | Rename the agent (sidebar), starting from its current name. `Ctrl-W` deletes a word, `Ctrl-U` clears, `Enter` saves, `Esc` cancels |
| `c` | Configure, in the sidebar. AI naming: choose a CLI or custom command, review the data disclosure, then `Enter` enables it; `d` disables naming, `Esc` goes back or closes |
| `n` | With no search kept, start a new agent in the selected agent's project. Pick the kind with arrows or `j`/`k`, then `Enter`: it opens in a new window of that session, in that agent's directory, and takes you there. `Esc` or `n` cancels |
| `o` | Organize all agents across every tmux session. The selected ready agent coordinates the task; Enter, Space or `y` confirms the footer prompt; other keys cancel |
| `Tab` | In the popup, type to the selected agent inside the preview; `Tab` again returns to the menu |
| `?` | Show every sidebar shortcut. Any key closes help and is consumed. On short terminals, reopen `?` for the next page; `?` stays literal text during search, filter and rename |
| `q` | Close the sidebar everywhere; close the popup |

`n` offers Claude Code, Codex, Pi, OpenCode and Grok Build. Antigravity needs a first task to start, so use `spawn agy "<task>"` for it. Pi, OpenCode and Grok Build always appear in the picker, but are tracked only once their [optional integrations](#more-agents) are set up. Like `Enter`, `n` refuses a session another client is using.

An agent in a linked window has one index row, displayed under the first session by name. It counts as visible when its pane is visible through any link.

While the sidebar is open, every window has one, including windows and sessions you create later; each keeps its own selection. A window's sidebar closes with the window: when its other panes are gone, the layout hook closes it immediately. The sidebar and popup start on the first blocked agent, or else the first done one. The agent you are looking at is shown in bold in the sidebar.

Moving with arrows or `j`/`k` previews the selected agent's window/session and keeps keyboard input in its sidebar. The sidebar pane itself moves into the previewed window, so selection, filter, search and order stay exactly as they were, and the sidebar it swapped with takes its place in the window you left, which gets back the pane it had active before the sidebar: come back with plain tmux and you land on that agent, with no key or focus event sent to it while you were away. Preview keeps the sidebar active, so a done agent remains unseen; `Enter` focuses it and makes it seen, and the sidebar moves with you the same way. Navigation requires a unique client in the input sidebar and refuses a destination session/window used by another client.

Session headings stay in place, each in the same colour its session has in the picker. Within each session, agents start in blocked, done, working, idle order, longest in state first. The order is latched: rows never move on their own, so badges and ages update in place, new agents append in their group, and removed agents disappear. Only `s` re-sorts, preserving the selected pane. The daemon keeps one order for every sidebar: the sidebar you press `s` in shows it at once, the others within about a second and a half. Sessions are listed by name. The highlight follows the agent you are in; from a sidebar or a plain shell it stays on the last agent you used, until you move it.

An idle sidebar runs no tmux commands: it reads the daemon's index twice a second and writes to its pane only when something on screen changed. A window with an agent active is right the moment you arrive, however quickly you move, because its active pane cannot change while it is hidden. Everything else comes from the index and can be a second or so old: arriving in a window whose keyboard was left in its sidebar or a plain shell, moving between the sidebar and an agent within a window, and which agent counts as last used.

The state filter, text filter and search stay visible above the list. They combine, and a search only visits rows the filters keep. A state filter stays selected if its last match changes state or exits, until you press `d` again. Typing a search moves the selection but pauses previews, so nothing changes under you until `Enter`; from there `n` and `N` preview each match while the keyboard stays in the sidebar, and `Enter` goes to the agent. The text filter works as it does in the picker: the best match comes first, under its session heading, so a heading can repeat when matches from different sessions interleave. It updates as you type and pauses previews, including arrow navigation, so editing stays in the current sidebar. `Enter` jumps to the selected match and ends the filter; `/` freezes the filter and previews its selection. `Esc` while editing restores the previous text and selection. While editing either one, letters such as `f`, `r` and `q` are text.

`x` closes the agent's pane using the same operation as `tmux-agent-plane kill`. It does not issue `kill-session`. The confirmation takes priority over notices and event footers, with separate name/state lines in narrow sidebars. It is pinned to the target's native identity and state; an exited, changed or renamed agent requires a new confirmation. The selection moves to the next visible agent (or the previous one at the end); a sidebar goes there first, then kills, so your keys stay in it when the agent's window closes. Killing a final pane may naturally remove its window; if that would also end a session, the prompt names it. Ending the session you would still be in is refused: the picker's client, or a sidebar with no other row to move to.

### tmux fuzzy picker

Press `prefix + a` from any pane. It opens over a dimmed copy of your screen, on the list, grouped by session: each row names its session in a colour of its own, next to the window and pane. The agent you are looking at is selected, including from a sidebar beside it, so you browse from where you are; with no agent in view, the top row is. `j`/`k`, Up/Down or Ctrl-J/Ctrl-K browse, and the screen preview updates inside the popup. Periodic state updates preserve the selected pane. Enter focuses that exact pane in the client that opened the picker, across sessions; `q`, Esc or Ctrl-C closes and leaves your original pane and session selected. Browsing never switches sessions.

Press `/` to search as in the sidebar: the selection and preview move to the next row whose name or task contains your text, and after `Enter`, `n` and `N` go through the matches. Press `f` to filter, or open the picker with `prefix + f` to start there. The filter box appears and letters become text: a case-insensitive fuzzy query across agent names, projects, task titles and blocked details, where each word can match any of them. Stronger matches appear first; Ctrl-J/Ctrl-K or Up/Down browse them. Backspace edits, Ctrl-W deletes a word and Ctrl-U clears. Press `/` to freeze the filter: its matches and selection stay, and your keys are commands again, so `n` or `x` act on the selected match. `f` resumes editing. Enter opens the selected match right away; Esc cancels the edit. A `/` cannot be part of a filter. On the list, Esc clears a kept search, then a kept filter, then closes.

The sidebar's actions work on the selected agent, with the same prompts and checks: `n` new agent, `x` kill, `r` rename, `o` organize, `d` state filter, `s` sort, `c` configure and `?` shortcuts. `n` closes the picker and takes you to the new agent.

Press `Tab` to type to the selected agent without leaving the picker. The preview gets a coloured border and follows the agent live, with its cursor, and every key goes to that agent: Enter, Esc, Ctrl-C and letters included, so you can give it a task or answer a prompt. `Tab` again returns to the menu or search you came from. The agent stays pinned: if it exits or leaves the list, the picker returns to the menu and sends nothing elsewhere. `Tab` itself never reaches the agent, and Shift-Tab does. The preview shows the agent's screen at its own size, so a pane wider than the popup is cut off on the right. Ctrl-J sends a newline. Each newline in a multi-line paste arrives as Enter, so it submits; to paste several lines, press `Enter` from the menu to jump to the agent's pane.

The popup inherits the terminal background and uses its palette for the selected row, even when a tmux theme configures other popup colors. The plugin supplies the originating client to `tmux-agent-plane ui --client <client-name>`.

### Neovim Telescope picker

With Neovim 0.10+ and [Telescope](https://github.com/nvim-telescope/telescope.nvim), add this local plugin to your lazy.nvim spec (adjust the checkout path):

```lua
{
  dir = vim.fn.expand("~/.tmux/plugins/tmux-agent-plane/extensions/nvim"),
  name = "tmux-agent-plane.nvim",
  dependencies = { "nvim-telescope/telescope.nvim" },
}
```

After your Telescope setup, load the extension and bind Space f a:

```lua
require("telescope").load_extension("tmux_agent_plane")
vim.keymap.set("n", "<leader>fa", function()
  require("telescope").extensions.tmux_agent_plane.agents()
end, { desc = "Find agent sessions" })
```

Run Neovim inside tmux with `tmux-agent-plane` on `PATH` and its daemon running. `:Telescope tmux_agent_plane agents` opens the same picker. It reads tracked agents from `tmux-agent-plane list --json`, with their existing names and task descriptions (blocked detail when present). Type a case-insensitive fuzzy search across name, project, title and blocked detail, including queries spanning those fields; use Up/Down in insert or normal mode to browse, then Enter to run `tmux-agent-plane focus <pane-id>` for that exact selection. Browsing does not change agent focus. An empty index opens an empty picker; command failures, including a stale index, appear as error notifications.

### Notifications

When an agent finishes or blocks off screen, every open sidebar shows the latest transition on its bottom line for five seconds: `✓ grok-support done` in green or `◆ grok-support blocked` in red. Sidebars in other windows and sessions receive the same event. Rename errors and active rename controls take priority; normal key hints return when the notice expires. A notice never opens a closed sidebar, changes focus or intercepts pane input. Expired notices are discarded, and restarting the daemon clears the previous notice.

Pi and OpenCode completion counters also report a new finished turn when it completes between polls while the agent still shows `done`. Each new completion follows the same footer and desktop policy.

Desktop notifications keep the task or blocked detail and work with the sidebar closed. Daemon faults and recoveries open in their own window a little below the top of the terminal, once you pause typing so it does not swallow keystrokes. A notice stays until you press a key or for 10 seconds, and it takes keyboard input while it is open.

### Project sessions

A tmux session is a project: its orchestrator and helpers live in the same session, and the sidebar groups them under that session's name. The CLI and shipped skill let an orchestrator organize existing work and place new work directly in a named project:

```bash
tmux-agent-plane sessions list --json
tmux-agent-plane list --json
tmux-agent-plane sessions create api --cwd /path/to/api
tmux-agent-plane move api-review --session api
tmux-agent-plane spawn codex "Review the API changes" --in window --session api --cwd /path/to/api --name api-tests
tmux-agent-plane spawn claude "Plan the website" --in session --session website --cwd /path/to/website --name website-lead
```

`sessions list` includes every session, even one with only shells: exact name and ID, attached-client count, window IDs, session group and agent names. Linked agents appear in each containing session. Existing destinations use exact names or IDs; new session names reject dots, colons, control characters and surrounding whitespace so tmux cannot silently rewrite them. `sessions create` opens a detached interactive shell. `spawn --in session --session NAME` creates the project with its first agent; `--name` names that agent separately. `spawn --in window --session NAME` uses an existing project's configured shell. An unknown destination fails before launching anything.

`move` uses a native detached pane move: the process keeps running, with the same pane ID, native identity, name, parent and completion history. No input is sent to the agent. Only the chosen pane moves, leaving other agents and shells in a mixed source window. The destination's current window and clients stay in place. The command prints the new location; the daemon publishes it on its next scan.

A source window or detached session may disappear when it has no remaining content; the normal sidebar hook closes a lone sidebar. Moves that would end an attached source session are refused. Linked source windows and grouped source/destination sessions are also refused because those share windows across projects. The CLI checks the live agent identity and rechecks topology in tmux's command queue before moving. Existing shell-only windows remain for the user. The skill's project organization workflow covers inspecting work, creating sessions, moving each team member and verifying the result.

An agent you start by hand joins a project by itself. Within a second of appearing it moves to the session named after its folder, which is created if needed, and any client viewing it moves too, so you keep working in it without interruption. The folder is the one the agent runs in, so a subfolder or worktree gets its own project; `.` and `:` in a folder name become `-`. This happens once per pane, so a later `move` sticks. Agents already running when the daemon starts, agents from `spawn` or the sidebar's `n`, and agents sharing a window with other panes stay put, as do moves into a session another client is using and any move involving grouped sessions or linked windows.

The sidebar's `o` action organizes **all agents across every tmux session**, including agents hidden by filters. Its compact footer asks `organize all agents? [Y/n]` and names the configured background CLI. Enter, Space or `y` accepts; `n` or Esc cancels. It uses the **same saved command, model and thinking settings as AI naming**, outside tmux; the selected row and its state have no role. The configured model receives the full agent/session inventory and returns JSON project assignments without tools. Native code validates the complete plan, creates needed sessions and moves misplaced agents through the guarded move path. No prompt or keys are sent to interactive agents. The footer shows progress/results; full errors and counts are in `daemon.log`. Closing the sidebar leaves the job running; daemon restart cancels it and reports interruption. Jobs never retry automatically.

Configure the background command with `c` first. Manual organization works even when automatic naming is disabled. The naming command must follow the supplied prompt: names for naming jobs, JSON assignments for organization. Malformed/incomplete output, changed configuration, changed agent identities or changed session topology reject the plan before application. Linked/grouped moves remain refused. An attached source losing its last agents gets a shell window so its clients stay connected; destination sessions scheduled to receive work are kept alive too. Native guards recheck each move. External changes during application can stop a partially applied job; its error reports completed move/project counts.

### Agent launch syntax

`spawn` starts a persistent interactive session in tmux’s configured `default-shell`. It passes the launch command directly to an interactive login shell, which loads your startup files before running the agent, so aliases and flags apply even with long or multi-line tasks. Each argument is quoted separately. When the agent exits, a fresh interactive login shell stays open. `spawn` supplies its own launch command, so tmux’s `default-command` is not used.

| Kind | Command sent to the shell with a task |
| --- | --- |
| `claude` | `claude '<task>'` |
| `codex` | `codex '<task>'` |
| `pi` | `pi -- '<task>'` |
| `opencode` | `opencode --prompt '<task>'` |
| `agy` | `agy --prompt-interactive '<task>'` |
| `grok` | `grok -- '<task>'` |

All kinds except agy can be spawned without a task. `spawn agy` requires a task and fails immediately if it is missing: agy publishes no state until its first turn. An agy started by hand appears once you send its first message; before then, use `read <pane-id>` and `keys <pane-id> ...`. Pi and OpenCode need their opt-in publisher installed before `spawn` can detect them; Grok needs leader mode.

### Optional AI naming

AI naming is **off by default**. With it off, no automatic naming command runs. Explicitly confirming `o` still runs the saved command for organization. The sidebar always shows `c configure` above its usual footer, so configuration is one key away.

Press `c` and choose `1` Claude Code, `2` Codex, `3` Pi, `4` OpenCode, `5` Antigravity (`agy`), `6` Grok Build, or `7` for a custom command. Setup checks the executable is installed without running it, shows the command and data disclosure, and requires `Enter` to enable. `Esc` goes back or closes; `c`, then `d` disables naming. For Antigravity, authenticate `agy` normally first, then use **`c`, `5`, `Enter`**. Setup itself never calls a model or changes folder trust. Use your configured sidebar key to open or reopen it (the plugin default is `prefix + S`).

The one configuration file is `${XDG_CONFIG_HOME:-$HOME/.config}/tmux-agent-plane/config.json`. Setup writes it; the daemon reads changes on its existing tick, without a restart. An Antigravity configuration looks like:

```json
{
  "auto_name": {
    "enabled": true,
    "command": [
      "agy", "--model", "gemini-3.8-flash", "--effort", "low",
      "--output-format", "text", "--mode", "plan", "--sandbox",
      "--print-timeout", "55s", "--print", "{prompt}"
    ],
    "output": "text"
  }
}
```

Presets select inexpensive models explicitly: Claude `haiku` with `--effort low`; Codex `gpt-6-luna` with `--config 'model_reasoning_effort="low"'` (the lowest advertised by the CLI cache); Pi `google/gemini-flash-lite-latest` with `--thinking minimal` (its catalog does not support `off` for this model); OpenCode `opencode/ling-2.6-flash-free`, a free, non-reasoning model with no thinking flag; Antigravity `gemini-3.8-flash` with `--effort low`; Grok `grok-4.5` with `--reasoning-effort low`, selecting the lowest cached-input rate among the CLI's locally advertised standard models (which tie on uncached input/output rates), and avoiding the twice-priced Fast variant. API-only cheaper models were not advertised by this CLI's cache. Claude and Pi use floating aliases; Codex, OpenCode, agy and Grok use catalog IDs because a suitable floating alias could not be confirmed. Flags were checked against CLI help and read-only catalogs; real model availability and execution were not tested. Existing saved commands stay as configured; choose the preset again to adopt these settings.

A custom `command` is a JSON array containing an executable on the daemon's `PATH` (or an absolute path) and literal arguments. Exactly one argument must be `"{prompt}"`; it is replaced with the job instruction and JSON inventory. No shell interprets the command. The custom command must run noninteractively, use no tools, and print only the requested result to stdout (a name or organization JSON); use `"output": "text"`. The OpenCode preset uses `"output": "opencode"` to extract text events from its JSON output. Unknown fields and malformed configuration produce a daemon diagnostic. With no file, naming stays off and the hint is visible.

**What is sent:** the eligible agent's task title, working-directory path (`cwd`), and blocked detail. These are passed as JSON in the prompt to your chosen CLI; its configured AI provider may receive them. Organization explicitly sends all tracked agents’ names, kinds, tasks, paths, states, parent/native identifiers and the session inventory. tmux-agent-plane sends no screen capture or file contents. The command uses your CLI's existing authentication. Presets explicitly select the model and thinking shown during setup; custom commands control their own model/provider and thinking. Review these before enabling. Custom commands must follow the same no-tools contract.

Naming waits for a title of at least eight bytes containing two words, excluding generic startup titles. It applies to every supported agent kind. Only agents with their generated `<kind>-<pane number>` name and no explicit pane name qualify. A name assigned with `--name`, `r` or `rename` stays yours, even if it resembles a generated name. Default names assigned by older versions already have an explicit pane option and are preserved; newly spawned agents leave that option unset. A live tmux condition checks the pane process, unassigned name and uniqueness across all sessions immediately before applying, so a racing manual rename wins.

The daemon runs one naming process at a time, outside tmux with closed stdin and a private working directory under its state directory. Each eligible pane/process lifetime gets **one attempt**, recorded before launch in `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-agent-plane/auto-name.json`, including across daemon restarts. Changing the command gives agents it failed to name a fresh attempt. An answer that fails the name rule gets one immediate retry, which tells the command what it answered and why that was rejected; a command that fails or times out is not retried. There is a 60-second timeout and a 16 KiB limit on each output stream, per run. A failed attempt pauses new naming for 60 seconds, leaves that agent's default name in place, and writes a deduplicated `auto-name:worker` diagnostic with degraded status to `daemon.log`, including the rejected answer. Tracking continues. The next agent named successfully clears the fault and is logged as recovery. A configuration change or daemon restart also clears the fault and the pause; neither repeats reserved attempts, and a command that is still broken raises the fault again on its next attempt.

AI output must be two or three lowercase words joined by hyphens, each starting with a letter, at most 20 ASCII characters and unique. A single terminal newline is allowed; quotes, markdown, whitespace, commentary and duplicate names are rejected. Names are never repaired. Built-in commands use their noninteractive modes and restrict tools where the CLI provides that control; they run away from project folders and never alter your trust settings. No extra tmux commands run on idle naming ticks or while naming is disabled.

### CLI

| Command | Action |
| --- | --- |
| `list [--json]` | All agents. `*` (or `"self": true`) marks a known caller |
| `sessions list [--json]` | Every project session: name/ID, windows, attached clients, group and agents |
| `sessions create <name> [--cwd DIR]` | Create a detached named project with an interactive shell |
| `move <agent> --session SESSION` | Move the running agent pane to an existing project, preserving its process and metadata |
| `read <agent> [--lines N]` | The agent's screen, plus N lines of scrollback |
| `send <agent> "<prompt>" [--answer] [--wait] [--timeout S]` | Paste a prompt and press Enter. Refuses blocked agents unless `--answer` deliberately replies to their question; `--answer` requires a blocked target. `--wait` follows the submitted turn until it ends or blocks |
| `keys <agent> <key>...` | Press keys, by tmux key name: `Enter`, `Escape`, `Down`, `2` |
| `wait <agent> [--timeout S]` | Wait until the agent is not working |
| `rename <agent> <name>` | Name an agent: one word, unique. `self` requires a known caller |
| `focus <agent>` | Show the agent on your screen |
| `cycle next\|previous --client CLIENT` | Direct key binding: focus the adjacent agent in global sidebar order for the exact invoking tmux client |
| `kill <agent>` | Close the agent's pane |
| `spawn claude\|codex\|pi\|opencode\|agy\|grok ["<task>"] [--in pane\|window\|session] [--session SESSION] [--cwd DIR] [--name NAME] [--wait] [--timeout S]` | Start an agent. `--in pane` (default) splits the caller's pane; `--session` selects an existing project with `--in window`, or names a new one with `--in session` |
| `toggle-sidebar <window>` | What the sidebar key does, from a window (a window or pane id) |
| `follow-sidebar <window>` | Add the sidebar to a window while it is open. The plugin runs it when a window is created |
| `sidebar` | The sidebar's agent list, run in the sidebar pane |
| `ui --client <client-name>` | The fuzzy agent picker popup (the plugin supplies the client) |
| `daemon start` | Start the detached watcher; the plugin runs this automatically |
| `daemon restart` | Gracefully restart the watcher using the current binary |
| `daemon stop` | Gracefully stop the watcher |

`<agent>` is an agent's name, tmux pane id, or `self` when the caller can be identified. An agent is `<kind>-<pane number>` (`claude-154`) until it is named with `spawn --name`, `rename`, or `r` in the sidebar. The name is stored on the agent's tmux pane. `read` and `keys` also take any pane id, so an agent stuck on a prompt before it starts can still be answered.

Read a blocked question before replying: `tmux-agent-plane read reviewer`, then `tmux-agent-plane send reviewer --answer "Use the existing naming convention"` when the user's instructions make that answer clear. Use `keys` for menus and permissions. Check `list` before status or escalation messages; defer them or report through an available lead if the target is blocked. Status and escalation messages must never use `--answer`, and a refusal must never automatically retry with that flag.

## How it works

- **Claude Code** keeps `~/.claude/sessions/<pid>.json` for each running session, with its tmux pane and a `status`: `busy`, `waiting` (with `waitingFor`, such as `input needed`), `idle`, or `shell` (the turn is over and a background shell runs on).
- **Codex** sets the terminal title: a braille spinner in front while a turn runs, `[ ! ] Action Required` while an approval or question waits. A pane is a Codex agent when exactly one top-level `codex` process runs under it, and no Claude Code process sits above it.
- **Antigravity** publishes its active conversation through the process's open presence file. Live SQLite summaries and their background-task flag distinguish working from fully idle; current-turn pending interactions take priority and distinguish permission and question waits. The reader includes SQLite so it reads active WAL data correctly.
- **Pi** publishes PID, pane, native session, live activity, UI prompts, and completed-turn counts through the opt-in extension in `extensions/pi/`.
- **OpenCode** publishes its active TUI session and native status, permission and question snapshots through the opt-in TUI publisher plugin in `extensions/opencode/`. Its default TUI has no external HTTP listener.
- **Grok Build** uses the official leader roster's `working`, `needs_input` and `idle` states. Native active-session records tie each local resident session to its live TUI process. The blinking terminal title is not used for state.
- **The daemon** scans all enabled kinds once a second for every pane on the tmux server. It marks a turn that ends off screen as `done` until the pane is on screen, writes the index to `~/.local/state/tmux-agent-plane/agents.json` along with each window's active pane, the sidebar row order and the last agent used, closes sidebars left alone in a window, sets the `@agent_plane_status` tmux option, and sends notifications. It also keeps hidden windows at the size they will be shown at, so moving between agents resizes nothing: about two seconds after your terminal changes size, each hidden window gets one resize to its final layout, and tmux's global `default-size` follows so new windows start right. Agents redraw once per size change, off screen; idle, this runs nothing. Windows on screen, zoomed windows and any `window-size` other than tmux's default `latest` are left alone, as is everything while attached clients show different sizes. It runs detached from tmux jobs, exits with the tmux server, and starting it again keeps the existing watcher. Lifecycle and daemon errors are appended to `daemon.log` alongside the index (`$XDG_STATE_HOME/tmux-agent-plane/daemon.log` when set).
- **The CLI, the sidebars and the agent list** read the index. They fail with a clear error when the daemon is not running.
- **`send`** pastes with bracketed paste, then presses Enter, so a multi-line prompt arrives as one message.

## Notes

- Pi and OpenCode are opt-in. Without their publisher status files, those kinds remain untracked and require no setup. A live opted-in publisher with malformed, unknown or stale status fails loudly; dead process files are ignored.
- Pi and OpenCode publish durable completion counters, so waits observe turns that finish between daemon polls. Claude Code, Codex, agy and Grok are sampled: a turn entirely between polls can be missed by `send --wait` or `spawn --wait`.
- Codex tool shells may inherit a stale `TMUX_PANE` from a shared app-server. `list` works and marks no caller; `spawn --in window` and `--in session` work with `parent: null`. Self references, inherited-pane targets and `spawn --in pane` fail with a clear error. Use explicit agent names and window/session spawning.

- Antigravity appears after its first message and native folder trust acceptance. Named projects (including resumed/forked projects) and `--settings` overlays excluding the cwd from `trustedWorkspaces` stay untracked because live trust cannot be proven.
- A Claude Code session is detected once it writes its session file, after the folder trust prompt. When `spawn` reports that an agent did not start, read its pane and answer the prompt with `keys`.
- Codex detection relies on its default terminal title. A `tui.terminal_title` setting that drops the activity spinner hides the `working` state.
- An unknown, malformed or stale live state withdraws the affected kind or agent from the index. Other kinds keep tracking; the status bar shows the fault and `daemon.log` records its full reason and recovery. Errors reading the process/pane snapshot or writing the index/log stop the watcher.
- After upgrading, run `tmux-agent-plane daemon restart`. This stops the managed watcher through its private control socket, waits for its index lock to release, then starts the current binary. `daemon stop` stops it without a replacement. Startup failures and SIGTERM exits are recorded in `~/.local/state/tmux-agent-plane/daemon.log` (under `$XDG_STATE_HOME` when set).
- Only the default tmux server is watched.

## Screen flicker in Ghostty

Ghostty supports synchronized output, which lets tmux finish a redraw before the
terminal displays it. Without it, pane and window changes can expose partially
redrawn frames, including sidebar view-mode navigation. See
[Ghostty's explanation](https://ghostty.org/docs/help/synchronized-output).

Check the attached clients with:

```sh
tmux list-clients -F '#{client_termname}: #{client_termtype}: #{client_termfeatures}'
```

If Ghostty uses `xterm-256color` and its features lack `sync`, add this to your
`tmux.conf` **only if every client using that TERM supports synchronized output**:

```tmux
set -as terminal-features ',xterm-256color:sync'
```

Reload your configuration, then detach and reattach the client. On tmux 3.7c,
changing this option or running `refresh-client` does not update the attached
client's features. Confirm `sync` appears in the command above. This requires
neither a daemon restart nor reopening sidebars.

The plugin leaves terminal capabilities to tmux: `terminal-features` matches TERM,
not the terminal's reported `client_termtype`, and a Ghostty client may share TERM
with a terminal lacking this feature. Automatically adding the option after
recognizing Ghostty would also affect those other clients on their next attach.

Repeat pane changes and sidebar view-mode navigation with the same window size.
If flicker persists with `sync` active, temporarily disable custom Ghostty shaders
and repeat to distinguish terminal rendering effects from application redraws.

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
