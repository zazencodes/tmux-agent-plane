# AGENTS.md

tmux-agent-plane: a Rust CLI, tmux plugin and agent skill that track and steer Claude Code and Codex agents running in tmux. See README.md for behaviour.

## Layout

- `src/detect.rs`: finds agents and reads their raw state. Claude from `~/.claude/sessions/<pid>.json`, Codex from the pane title plus the process tree.
- `src/daemon.rs`: one-second loop. Folds detection into `Done`/seen state, writes the index, sets the `@agent_plane_status` tmux option, notifies.
- `src/index.rs`: the index file (`~/.local/state/tmux-agent-plane/agents.json`) and its types. The daemon is its only writer.
- `src/commands.rs`: `list`, `read`, `send`, `keys`, `focus`, `kill`, `wait`, `spawn`. Agents are told never to use raw tmux on agent panes, so every agent job needs a command here.
- `src/ui.rs`: the ratatui agent list shown in the `prefix + A` popup.
- `tmux-agent-plane.tmux`: plugin entry point. Binds the key and starts the daemon.
- `skill/tmux-agent-plane/SKILL.md`: the agent skill. Keep it in sync with the CLI.

## Rules

- Detection reads what agents publish (session files, titles). Do not add hooks or agent-side config.
- Fail loudly: unknown agent states, tmux errors and a stale index are errors.
- One canonical way per job; no fallbacks.

## Development

```bash
cargo test
cargo clippy --release
cargo build --release   # ~/.local/bin/tmux-agent-plane links to target/release on Alex's machine
pkill -f 'tmux-agent-plane daemon'; tmux source-file ~/.tmux.conf   # restart the daemon on the new build
```

Test changes against live agents: spawn throwaway agents with `tmux-agent-plane spawn ... --in window` in a scratch folder, and kill their windows afterwards.
