# Changelog

All notable user-facing changes to `tmux-agent-plane`. Versions follow [Semantic Versioning](https://semver.org/);
before 1.0, a breaking change bumps the minor version.

## Unreleased

### Added

- `/` in the sidebar and agent picker now searches as in Vim: type part of an agent's name or task, ignoring case, and the selection moves there with every row still listed and every match highlighted. `Enter` keeps the search, `n`/`N` go through the matches, and `Esc` or `Ctrl-C` clears it. `n` starts a new agent again once the search is cleared
- Alt+H moves into the sidebar, opening it if needed, and stays there when you are already in it; Alt+L or Shift+Right leaves it for the selected agent. Configure with `@agent-plane-sidebar-in-key` and `@agent-plane-sidebar-out-key`, or bind other keys to the new `enter-sidebar` command
- Agents you start by hand join a session named after their folder within a second, creating it if needed, and your screen moves with them; a later `move` sticks, and spawned agents stay where they were asked to go
- When the sidebar or agent picker says the daemon is not running, press `Enter` to restart it; if it cannot start, the reason stays on screen
- `prefix + f` opens the agent picker already filtering, ready to type; this replaces tmux's window finder on that key
- Sidebar session headings use the same per-session colours as the agent picker
- `gg` and `G` jump to the first and last agent in the sidebar and the agent picker
- The agent picker has the sidebar's actions: `n` new, `x` kill, `r` rename, `o` organize, `f` filter, `s` sort, `c` configure and `?` shortcuts
- Press `Tab` in the agent picker to type to the selected agent in its live preview, and `Tab` again to return to the menu
- The agent picker lists rows by session and shows each session name in its own colour
- Browse with Ctrl-J/Ctrl-K in the agent picker, and while editing a search in the sidebar
- Start a new agent from the sidebar: press `n`, pick the kind, and it opens in a new window of the selected agent's project and directory, with you in it
- Organize project sessions from the sidebar: press `o`, then Enter or Space to group all agents across every session using the configured background AI command; opening or canceling runs nothing
- Focus the previous or next agent directly with Alt+K/J (Option+K/J on macOS), across sessions and without entering the sidebar; configure the keys with `@agent-plane-previous-key` and `@agent-plane-next-key`
- Treat tmux sessions as projects: inspect/create named sessions, spawn directly into an explicit project, and move running agents between projects without restarting them or sending input; the agent skill now includes a project organization workflow
- A Neovim Telescope picker for tracked agent sessions: fuzzy-search names, task titles and blocked details, browse with arrows, and press Enter to focus the selected pane

### Changed

- `f` now filters rows by text, the fuzzy match `/` used to do, and `f` resumes a frozen filter; the state filter moved to `t`
- The screen behind the agent picker is dimmed while it is open
- The sidebar and the agent picker no longer repeat agent counts in their headers; the status bar is the one place for them
- Moving the sidebar selection always previews the selected agent's window, with the keyboard staying in the sidebar; the `v` toggle is gone
- `x` in the sidebar moves to the next row first, then kills, so the keyboard stays in the sidebar and an agent alone in your current session can be killed
- `Enter` during a search opens the selected match right away, in the agent picker and the sidebar; `/` freezes the search when you want to keep it
- The agent picker opens on the agent you are looking at, including from a sidebar beside it, so you browse from where you are; with no agent in view it opens on the top row
- Moving between agents no longer makes them reflow on arrival: the daemon keeps hidden windows at your terminal's size, including after the terminal resizes, and new windows start at that size with their sidebar already in place. This sets tmux's global `default-size`
- Confirm global organization in a compact yes/no footer like kill: `o`, then Enter, Space or `y` to accept; `n` / Esc cancels while the agent list stays visible

- Clarify sidebar organization covers all tracked agents across every tmux session, including hidden sidebar rows
- Open sidebar organization with lowercase `o`
- Open the tmux agent picker with lowercase `prefix + a`; it opens on the list, where `j`/`k` browse and letters are commands; press `/` to fuzzy-search names and descriptions and `/` again to freeze the matches and use commands on them, Enter to focus the exact pane in the originating client, or `q`/Esc/Ctrl-C to close
- Let the agent picker inherit the terminal background, with selection using the terminal palette
- The sidebar and the agent picker share one search: fuzzy across agent name, project (tmux session), task and blocked detail, in any word order and ignoring case, with the best match first. Location and directory are no longer searched, and `/` during a search freezes its matches in both

### Fixed

- Organize no longer fails with a session name error when a project folder has a dot in its name, such as `my.app`; the background agent is told the naming rules, and a name it still gets wrong is quoted in the error
- The agent you land on goes bold in the sidebar straight away, and the one you left stops being bold, with no half-second lag
- The agent picker shows a blocked agent's reason in red, like the sidebar
- An empty sidebar no longer implies only Claude Code and Codex are tracked
- A sidebar no longer freezes on an old list, showing closed agents and missing new ones, after `prefix + [` or a mouse drag put it in tmux copy mode: sidebars now leave copy mode at once
- Keeping hidden windows sized no longer fails with "command too long" when many windows need resizing, and a persistent fault no longer flips between error and "recovered" popups
- Organize no longer fails with "Codex process missing or ambiguous", and the sidebar and picker can kill a Codex agent, while that agent is running helper processes
- Give an AI name that breaks the naming rule one corrected retry, and show the rejected answer in the error if the retry fails too
- Clear the `auto-name:worker` warning once a later agent is named, and stop repeating an old naming error after every daemon restart
- Run global organization through the same configured background command/model as AI naming, with progress in the sidebar and guarded native session moves; never send organization tasks into open agent panes
- Show the default-yes confirmation as `[Y/n]` for organization and kill; Enter accepts, and Space also accepts organization
- Close a window's sidebar immediately when its last other pane disappears, removing the full-screen sidebar pause before the window closes

## 0.4.3 - 2026-11-04

### Added

- Ghostty flicker troubleshooting with synchronized-output configuration, client reattachment requirements, and shared-TERM limitations

### Changed

- Restyle the configure screen to match the sidebar: rounded panel, section headings, an on/off badge, a tick on the active command and key hints in the footer
- Open configuration with `c`, always shown as `c configure` above the sidebar footer; the `a` key and the `AI names` hint are gone

### Fixed

- Fix AI naming with Antigravity: the preset passed its arguments in an order `agy` rejects, so no agent was ever named. Re-select it with `c`, `5`, `Enter`; changing the command retries agents that failed
- Make daemon notices readable: they open in their own window a little below the top of the terminal, leaving the status bar visible, with a clear heading, the source and the log path on separate lines, and they stay until you press a key or for 10 seconds

### Removed

- Remove the 90-day settings review reminder and the option to hide the setup hint. A config file written by 0.4.2 fails loudly on the old `reviews` and `auto_name.hide_hint` keys: delete them

## 0.4.2 - 2026-11-03

### Added

- Subtle periodic settings reviews for enabled AI naming: compare saved settings with the installed preset, keep or change them, and configure or permanently disable the 90-day reminders
- Press `?` in the sidebar for keyboard shortcuts, with pages on short terminals and closing keys consumed safely
- Optional AI names for default-named agents, off by default: press `a` in the sidebar to choose a CLI or custom command, review what is sent, and enable; the setup hint can be hidden permanently
- Serialized, time-limited naming attempts with strict output validation, persistent failure diagnostics, and protection for user-assigned names

### Changed

- Select low-cost models and the lowest verified thinking level explicitly in AI naming presets, and show those settings before enabling
- Stop sidebars polling tmux: an idle sidebar now runs no tmux commands and writes to its pane only when something on screen changed, so open sidebars cost next to nothing however many windows you have
- Show the agent state, the highlight and a re-sort from another sidebar up to about a second and a half after they change; the sidebar you press `s` in still re-sorts at once, and a window with an agent active is still right the moment you arrive
- Give a window back the pane it had active when view mode or `Enter` carries the sidebar out of it, so coming back with plain tmux lands on that agent, not on the sidebar

### Fixed

- Keep one agent order in every sidebar: rows no longer change as you move between windows and sessions, and `s` re-sorts them everywhere
- Highlight the agent you are in, or the last one you used when you are in a sidebar or a plain shell, in every sidebar

## 0.4.1 - 2026-11-03

### Fixed

- Make view mode previews and `Enter` jumps faster: a switch now runs two tmux commands where it ran five, and the selection moves together with the window
- Stop the sidebar flickering in view mode: previewing an agent no longer shows the wrong row selected for up to half a second after the window changes
- Keep the selected row highlighted after `Enter` jumps to another window: the sidebar now moves with you, so its selection, filter and search stay as they were
- Stop reporting `degraded: scan:pi` and briefly dropping every Pi agent from the list when one Pi agent exits

## 0.4.0 - 2026-11-03

### Changed

- Keep sidebar rows where they are: agents no longer reorder when state changes, when you focus the sidebar, change the filter or accept a search; press `s` to re-sort
- Use lowercase sidebar keys only: `v` for view mode, `f` for the state filter and `s` for re-sort (were `V`, `F`, `S`)
- Search the sidebar with `/` instead of `\`, on the same line as the `V` and `F` settings
- Show daemon fault and recovery announcements in a short popup under the status bar instead of replacing the status bar, opened only after you pause typing so it does not swallow keystrokes

### Fixed

- Keep every sidebar exactly 38 columns wide; resizing the window or attaching a differently sized client no longer leaves some at other widths
- Stop reporting `degraded: scan:codex` or `ambiguous agent identity` when a Claude Code session runs `codex` in its pane or several one-off `codex` processes run side by side; those are tools, not agents
- Stop reporting `degraded: scan:codex` when Codex starts helper `codex` processes under itself
- Start the daemon after tmux creates its first session, preventing a stopped watcher when opening a fresh tmux server

## 0.3.0 - 2026-11-02

### Added

- Add sidebar view mode (`V`), incremental search (`\`) that pauses preview until accepted, and state filters (`F`); carry navigation between sidebars and refuse ambiguous/shared-client switches
- Order agents within session groups by attention and time in state; keep rows latched during navigation, with `S` to re-sort explicitly
- Add sidebar `x` confirmation pinned to the agent’s name, state and native identity; Enter or `y` confirms, other keys cancel, remote session endings are named, and ending your attached session is refused

### Fixed

- Deliver long and multi-line spawn tasks intact by passing the launch command directly to the configured interactive login shell; startup files and aliases load before the agent runs, and a shell stays open after it exits
- Keep agents in linked windows tracked once under the first session by name, with visibility through any link; validate sidebar session-ending warnings against every linked session
- Adopt incoming sidebar navigation before consuming buffered keys on a return visit, preserving rapid movement across sidebars
- Cancel sidebar kill confirmation when its originating client's session or window changes

## 0.2.0 - 2026-11-02

### Added

- Track Antigravity CLI (`agy`) from its first turn, including permissions, questions and background work; spawning requires a task and rejects omission before creating a pane
- Track and spawn Grok Build on macOS through its opt-in official leader mode, including permissions and questions
- Track and spawn OpenCode through an opt-in TUI plugin, including permission/question waits and durable turn completion counters
- Track and spawn Pi through its opt-in official extension, including UI prompts and durable turn completion counters

### Fixed

- Point stale-index errors to `daemon start` and the persistent daemon log
- Record the original tmux pane-listing error when the watcher exits, including server shutdowns and transient failures; record why duplicate workers exit
- Refuse ordinary sends to agents waiting on a question or permission before touching buffers or keys; deliberate textual replies use `send --answer`, which requires a blocked target and follows the reply turn with `--wait`
- Notify for a new Pi or OpenCode completion even when the agent remains unseen and already shows done
- Start the watcher detached from tmux jobs so startup, duplicate starts, stops, failures and signals never open job output over a pane; use `daemon restart` for a targeted graceful restart, with lifecycle diagnostics in `daemon.log`
- Keep the index updating when tmux status writes or client refreshes fail; retry presentation and report the fault and recovery
- Keep healthy agents tracking through kind scan, completion counter and notification failures; withdraw faulty observations, show degraded/recovered status, and persist full errors and fatal exit reasons in the state directory’s `daemon.log`
- Keep Antigravity untracked until native folder trust is established; selected projects and session overrides that force trust stay untracked when live trust cannot be proven
- Keep tracking agents when a Grok process exits during detection
- Publish OpenCode completion counts together with fresh native state, preserving done notifications when a turn ends between polls
- Reject unsafe Codex self references and pane splits when a shared app-server supplies a stale `TMUX_PANE`. Listing works without a caller marker; window/session spawning works with no parent
- Use published completion counters when available so wait commands observe turns that finish between daemon polls, without recounting turns when navigating between sessions

### Changed

- Show off-screen done and blocked notifications for five seconds in every open sidebar footer, preserving pane input and focus; desktop notifications keep their existing delivery channel
- Reject ambiguous agent identities and match Claude session processes to their tmux panes, including when separate tmux servers reuse pane IDs

### Upgrade notes

- A running 0.1.x watcher must be stopped once during this upgrade; it has no lifecycle control socket. Start the new watcher with `tmux-agent-plane daemon start`. Future upgrades use `tmux-agent-plane daemon restart`.

## 0.1.1 - 2026-11-02

### Changed

- The sidebar is open in every window of every session at once: `prefix + S` opens it everywhere, windows and sessions you go to later get it too, and the key (from any pane) or `q` closes it everywhere. A sidebar closes with its window when the window's other panes are gone

## 0.1.0 - 2026-11-02

### Added

- A daemon that finds Claude Code and Codex agents in every tmux pane and tracks whether each is working, blocked, done or idle, without hooks
- The `@agent_plane_status` status bar segment, and tmux and desktop notifications when an agent finishes or blocks off screen
- The agent sidebar (`prefix + S`, set with `@agent-plane-sidebar-key`): a live list of every agent down the left of the window, grouped by tmux session. The key opens it and moves you into it, and closes it from inside; `Enter` jumps to an agent
- The agent list popup (`prefix + A`), with a live preview of the selected agent and `Enter` to jump to it
- The `list`, `read`, `send`, `keys`, `wait`, `focus`, `kill` and `spawn` commands, so agents can see and steer each other
- Agent names: `rename`, `r` in the sidebar, and agents that name themselves and the agents they spawn
- The `tmux-agent-plane` agent skill
