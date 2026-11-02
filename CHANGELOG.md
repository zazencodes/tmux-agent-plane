# Changelog

All notable user-facing changes to `tmux-agent-plane`. Versions follow [Semantic Versioning](https://semver.org/);
before 1.0, a breaking change bumps the minor version.

## Unreleased

### Added

- A daemon that finds Claude Code and Codex agents in every tmux pane and tracks whether each is working, blocked, done or idle, without hooks
- The `@agent_plane_status` status bar segment, and tmux and desktop notifications when an agent finishes or blocks off screen
- The agent sidebar (`prefix + S`, set with `@agent-plane-sidebar-key`): a live list of every agent down the left of the window. The key opens it and moves you into it, and closes it from inside; `Enter` jumps to an agent
- The agent list popup (`prefix + A`), with a live preview of the selected agent and `Enter` to jump to it
- The `list`, `read`, `send`, `keys`, `wait`, `focus`, `kill` and `spawn` commands, so agents can see and steer each other
- Agent names: `rename`, `r` in the sidebar, and agents that name themselves and the agents they spawn
- The `tmux-agent-plane` agent skill
