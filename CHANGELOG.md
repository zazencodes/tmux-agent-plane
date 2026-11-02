# Changelog

All notable user-facing changes to `tmux-agent-plane`. Versions follow [Semantic Versioning](https://semver.org/);
before 1.0, a breaking change bumps the minor version.

## Unreleased

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
