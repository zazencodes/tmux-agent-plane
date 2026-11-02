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

- Keep tracking agents when a Grok process exits during detection

- Publish OpenCode completion counts together with fresh native state, preserving done notifications when a turn ends between polls

- Reject unsafe Codex self references and pane splits when a shared app-server supplies a stale `TMUX_PANE`. Listing works without a caller marker; window/session spawning works with no parent

- Use published completion counters when available so wait commands observe turns that finish between daemon polls, without recounting turns when navigating between sessions

### Changed

- Reject ambiguous agent identities and match Claude session processes to their tmux panes, including when separate tmux servers reuse pane IDs

- The sidebar is open in every window of every session at once: `prefix + S` opens it everywhere, windows and sessions you go to later get it too, and closing it (the key from inside it, or `q`) closes it everywhere

## 0.1.0 - 2026-11-02

### Added

- A daemon that finds Claude Code and Codex agents in every tmux pane and tracks whether each is working, blocked, done or idle, without hooks
- The `@agent_plane_status` status bar segment, and tmux and desktop notifications when an agent finishes or blocks off screen
- The agent sidebar (`prefix + S`, set with `@agent-plane-sidebar-key`): a live list of every agent down the left of the window, grouped by tmux session. The key opens it and moves you into it, and closes it from inside; `Enter` jumps to an agent
- The agent list popup (`prefix + A`), with a live preview of the selected agent and `Enter` to jump to it
- The `list`, `read`, `send`, `keys`, `wait`, `focus`, `kill` and `spawn` commands, so agents can see and steer each other
- Agent names: `rename`, `r` in the sidebar, and agents that name themselves and the agents they spawn
- The `tmux-agent-plane` agent skill
