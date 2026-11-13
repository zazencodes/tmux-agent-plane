# Changelog

All notable user-facing changes to `tmux-agent-plane`. Versions follow [Semantic Versioning](https://semver.org/);
before 1.0, a breaking change bumps the minor version.

## Unreleased

### Added

- Workspaces: named groups of tmux sessions, one level above tmux, so you can set a body of work aside the way you detach from tmux. The sidebar, the picker, Alt + k / j, sidebar notices and tasks, and each session's status bar show only the workspace you are in; tmux still shows every session. Every existing session starts in `main`, and nothing changes until you start a second workspace.
- `w` in the sidebar and picker lists your workspaces, marking the ones with blocked or done agents: `Enter` switches, `c` starts a new one, `r` renames, `m` moves the selected agent's project into the highlighted workspace.
- `tmux-agent-plane workspace list|switch|move|rename`, and `--workspace` on `sessions create` and `spawn --in session`, so you and your agents can start workspaces and break work off into them. The skill tells agents to do this only when you ask.
- New sessions join the workspace you are in, automatic folder projects stay inside their workspace, restore brings sessions back into theirs, and closing a session keeps you in its workspace.

### Fixed

- Ctrl+C behaves like Escape throughout the UI, backing out one screen at a time, including workspace menus, configuration editors and quota.

- Configuration shows whether automatic naming is on or off, labels the background command as saved, and selects the naming switch after you save a command.

### Changed

- Turning Agent Plane keys off preserves open sidebars and their normal navigation. Opening or closing a sidebar is blocked until keys are back on. Global navigation, sidebar and picker keys return to their original tmux bindings, including `prefix + Space` for layouts. The status bar keeps an enable reminder visible; `prefix + o` turns the keys back on.
- Use lowercase `prefix + o` to turn global navigation keys off or on, so you can pass them through to Codex and other programs.

## 0.7.0 - 2026-11-10

### Changed

- Restore `gg` to jump to the first agent and move the graph toggle back to `p` in the sidebar, picker and configuration.

### Added

- Jump to the first agent in the previous or next project with `{` and `}` in the sidebar and picker.
- Search project names with `/`: each matching project selects its first visible agent, alongside agent-name and task matches.

## 0.6.1 - 2026-11-09

### Fixed

- Jumping to the bottom of the sidebar stops at the last agent, even with tasks shown.
- Keep one blank line between sidebar content and the key hints, with quota shown or hidden.

### Changed

- `g` shows or hides the graph in the sidebar and agent picker, and toggles its configuration default. `Home` jumps to the first agent.
- Simplify the README, put Quick start first, update configuration and restore guidance, and credit quota-axi and T3 Code for their inspiration.

## 0.6.0 - 2026-11-09

### Added

- `spawn` passes everything after `--` verbatim to the agent CLI, ahead of the task, so you can pick a model or any other flag: `tmux-agent-plane spawn codex "<task>" -- -m <model>`
- Give agents your standing preferences in `~/.config/tmux-agent-plane/config.toml`, such as the arguments to start Codex with; the skill tells agents to read and follow it. See step 5 of Install in the README
- Background tasks: agents start dev servers, watchers and tunnels with `tmux-agent-plane task start <name> -- <command>`, so every agent sees one list (`task list`) with each task's state and listening ports, reuses a running one instead of starting a second copy, and can `task read`, `task restart` and `task stop` it. Tasks run in their own windows in an `agent-plane-tasks` session, outlive the agent that started them, and keep their output and exit status when their command ends. The skill tells agents to use them instead of backgrounding commands themselves
- Press `t` in the sidebar to list background tasks below the agents, with each one's ports or exit code, project and age. Move onto a task to preview its window, `Enter` to open it, `x` to stop it after confirming, and `R` to restart it
- Agents come back after tmux dies: when the tmux server stops (a crash, a reboot, `kill-server`) and starts again, the status bar shows `↺ N` and `tmux-agent-plane restore` lists the agents that can come back. `restore --tasks` or `--no-tasks` reopens each one's conversation in its session, window and layout, with its name and parent, with or without the background tasks that were running; `restore --discard` forgets them. Codex agents cannot be resumed and are listed with their folder
- While agents from before tmux restarted wait, the sidebar says so on its top line; press `b` to see who can come back, who cannot and why, and the tasks that were running, then `Enter` to bring back agents and tasks, `a` for agents only, `d` to forget them or `Esc` to decide later
- Install by asking your coding agent to "install and configure tmux-agent-plane with me"; the README walks it through onboarding you, one choice at a time, including which keys move you between agents
- Press `u` in the sidebar to pin how much of your Claude, Codex, Antigravity and Grok Build plans is left: each window's remaining percent and time to reset, a marker for even use, and whether it lasts until the reset. `u` hides the panel; `i` opens details, where `r` reads again
- `tmux-agent-plane quota [--json]` reads the same, fresh, so agents can check capacity before handing out work
- Claude's login is read from the macOS Keychain only after you allow it once: press `Enter` on its card in the quota view
- Keep quota in sight: press `o`, then `u`, and every sidebar shows a quota panel below the agents, one line per plan with what is left and when it resets. It is off by default, and `u` in a sidebar shows or hides it and `i` opens full details
- Choose whether sidebars open with background tasks shown: `o`, then `t`. They are shown by default, and `t` in a sidebar still shows or hides them there
- Press `prefix + g` to turn the global keys off when a program needs them, and again to turn them back on. While they are off, every one of them reaches the pane as usual and the status bar shows `keys off · prefix g`, as does the bottom left of the sidebar and the agent picker. Start with them off with `set -g @agent-plane-keys off`, and choose the toggle with `@agent-plane-keys-toggle-key`
- The global key options take several keys each, separated by spaces, such as `set -g @agent-plane-sidebar-in-key 'M-h S-Left'`; all of them turn off together
- `l` and → open the selected agent in the sidebar and the agent picker, like `Enter`
- Press `p` in the sidebar or the agent picker to see the selected agent's graph: its parent in a box on the left, the agents that parent spawned down the right, joined by lines that are dashed while a child works and red while it is blocked, with the selected agent lit. It follows the selection, in a panel below the sidebar's list and in place of the picker's preview. Sidebars show it by default, and only for an agent with a parent or children; `o`, then `p` turns that off
- Codex agents record themselves as the parent of the agents they spawn with `spawn ... --parent <their name>`, which `spawn` requires from Codex

### Changed

- Quota uses the same unboxed heading and rule as tasks, with `u` as its single expand/collapse control. Removed the duplicate `i` shortcut.

- Quota details expand upward from the bottom, keeping agents visible; `u` expands/collapses them. Expired logins disappear from quota.
- Configuration sits at the bottom left, with arrow/j/k navigation, Enter/Space choices, checkboxes, saved confirmations and Accept. Escape goes back; Ctrl+C closes. Choose a background agent separately from automatic naming, and set each agent’s default launch model and effort.

- `u` shows or hides quota pinned at the bottom of the sidebar, below the graph and tasks, while browsing stays active. `i` opens full quota details; `Esc` returns to the panel
- The parent/child diagram is called Graph in configuration, panel headings and help
- Closing a session switches you to the most recently active remaining session, with or without the sidebar; closing the final session exits tmux
- Shown background tasks stay at the bottom of the sidebar, below the graph, while the agent list scrolls

- A parent link now belongs to the agents, not their panes: it ends when the spawned agent exits, so an agent started later in the same pane no longer inherits it, and `list` shows `ended` once the parent has exited. `spawn` from a plain shell records no parent
- `?` in the sidebar and agent picker is shorter: your tmux keys for moving between agents and in and out of the sidebar, as you have them bound, then one line per key under Move, Find, Act and Show. Keys for confirming and editing text stay in the footer while you use them

## 0.5.0 - 2026-11-07

### Added

- `/` in the sidebar and agent picker now searches as in Vim: type part of an agent's name or task, ignoring case, and the selection moves there with every row still listed and every match highlighted. `Enter` keeps the search, `n`/`N` go through the matches, and `Esc` or `Ctrl-C` clears it.
- Alt+H moves into the sidebar, opening it if needed, and stays there when you are already in it; Alt+L or Shift+Right leaves it for the selected agent. Configure with `@agent-plane-sidebar-in-key` and `@agent-plane-sidebar-out-key`, or bind other keys to the new `enter-sidebar` command
- Agents you start by hand join a session named after their folder within a second, creating it if needed, and your screen moves with them; a later `move` sticks, and spawned agents stay where they were asked to go
- When the sidebar or agent picker says the daemon is not running, press `Enter` to restart it; if it cannot start, the reason stays on screen
- `prefix + f` opens the agent picker already filtering, ready to type; this replaces tmux's window finder on that key
- Sidebar session headings use the same per-session colours as the agent picker
- `gg` and `G` jump to the first and last agent in the sidebar and the agent picker
- The agent picker has the sidebar's actions: `c` new, `x` kill, `r` rename, `f` filter, `o` configure and `?` shortcuts
- Press `Tab` in the agent picker to type to the selected agent in its live preview, and `Tab` again to return to the menu
- The agent picker lists rows by session and shows each session name in its own colour
- Browse with Ctrl-J/Ctrl-K in the agent picker, and while editing a search in the sidebar
- Start a new agent from the sidebar: press `c`, pick the kind, and it opens in a new window of the selected agent's project and directory, with you in it
- Focus the previous or next agent directly with Alt+K/J (Option+K/J on macOS), across sessions and without entering the sidebar; configure the keys with `@agent-plane-previous-key` and `@agent-plane-next-key`
- Treat tmux sessions as projects: inspect/create named sessions, spawn directly into an explicit project, and move running agents between projects without restarting them or sending input; the agent skill now includes a project organization workflow
- A Neovim Telescope picker for tracked agent sessions: fuzzy-search names, task titles and blocked details, browse with arrows, and press Enter to focus the selected pane

### Changed

- Configure moves from `c` to `o`, because `c` now creates a new agent, as `prefix + c` creates a window
- The configure screen is easier to read: it shows at a glance which command is active and which CLIs are not installed, spells out what is sent to an AI provider, and appears as a centred card in the picker
- `f` now filters rows by text, the fuzzy match `/` used to do, and `f` resumes a frozen filter; the state filter moved to `d`, and `Esc` or `Ctrl-C` clears both filters back to the whole list
- The screen behind the agent picker is dimmed while it is open
- The sidebar and the agent picker no longer repeat agent counts in their headers; the status bar is the one place for them
- Moving the sidebar selection always previews the selected agent's window, with the keyboard staying in the sidebar; the `v` toggle is gone
- `x` in the sidebar moves to the next row first, then kills, so the keyboard stays in the sidebar and an agent alone in your current session can be killed
- `Enter` during a search opens the selected match right away, in the agent picker and the sidebar; `/` freezes the search when you want to keep it
- The agent picker opens on the agent you are looking at, including from a sidebar beside it, so you browse from where you are; with no agent in view it opens on the top row
- Moving between agents no longer makes them reflow on arrival: the daemon keeps hidden windows at your terminal's size, including after the terminal resizes, and new windows start at that size with their sidebar already in place. This sets tmux's global `default-size`
- Open the tmux agent picker with lowercase `prefix + a`; it opens on the list, where `j`/`k` browse and letters are commands; press `/` to fuzzy-search names and descriptions and `/` again to freeze the matches and use commands on them, Enter to focus the exact pane in the originating client, or `q`/Esc/Ctrl-C to close
- Let the agent picker inherit the terminal background, with selection using the terminal palette
- The sidebar and the agent picker share one search: fuzzy across agent name, project (tmux session), task and blocked detail, in any word order and ignoring case, with the best match first. Location and directory are no longer searched, and `/` during a search freezes its matches in both

### Fixed

- Sidebar selection no longer snaps back when an index refresh overlaps a preview switch. Global previous/next keys inside the sidebar move from its selected row rather than jumping to the ends of the agent list

- The sidebar and agent picker no longer stay scrolled with agents hidden off the top once the whole list fits, such as after the pane grows or agents close
- The agent you land on goes bold in the sidebar straight away, and the one you left stops being bold, with no half-second lag
- The agent picker shows a blocked agent's reason in red, like the sidebar
- An empty sidebar no longer implies only Claude Code and Codex are tracked
- A sidebar no longer freezes on an old list, showing closed agents and missing new ones, after `prefix + [` or a mouse drag put it in tmux copy mode: sidebars now leave copy mode at once
- Keeping hidden windows sized no longer fails with "command too long" when many windows need resizing, and a persistent fault no longer flips between error and "recovered" popups
- The sidebar and picker can kill a Codex agent while that agent is running helper processes
- Give an AI name that breaks the naming rule one corrected retry, and show the rejected answer in the error if the retry fails too
- Clear the `auto-name:worker` warning once a later agent is named, and stop repeating an old naming error after every daemon restart
- Show the default-yes kill confirmation as `[Y/n]`; Enter accepts
- Close a window's sidebar immediately when its last other pane disappears, removing the full-screen sidebar pause before the window closes

### Removed

- The `s` re-sort key and attention sorting: rows stay in a stable order, with new agents added at the end of their session

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
