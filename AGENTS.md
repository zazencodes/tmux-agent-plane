# AGENTS.md

tmux-agent-plane: a Rust CLI, tmux plugin and agent skill that track and steer Claude Code, Codex, Pi, OpenCode, Antigravity and Grok Build agents running in tmux. See README.md for behaviour.

## Layout

- `src/agents/`: one adapter per kind owns live state signals, title parsing and interactive launch syntax; `mod.rs` holds scan/process/observation types.
- `src/detect.rs`: takes one process-table snapshot, joins adapter observations to tmux panes and rejects ambiguous identities.
- `extensions/`: opt-in Pi/OpenCode publishers and Grok leader configuration. Keep install instructions in README More agents.
- `src/daemon.rs`: one-second loop. Folds detection into `Done`/seen state, writes the index, sets the `@agent_plane_status` tmux option, notifies.
- `src/index.rs`: the index file (`~/.local/state/tmux-agent-plane/agents.json`) and its types. The daemon is its only writer.
- `src/commands.rs`: `list`, `read`, `send`, `keys`, `focus`, `kill`, `wait`, `spawn`. Agents are told never to use raw tmux on agent panes, so every agent job needs a command here.
- `src/ui.rs`: the ratatui agent list shown in the `prefix + A` popup.
- `src/sidebar.rs`: the always-on agent list in a sidebar pane (`prefix + S`), and its toggle.
- `tmux-agent-plane.tmux`: plugin entry point. Binds the key and starts the daemon.
- `skill/tmux-agent-plane/SKILL.md`: the agent skill. Keep it in sync with the CLI.

## Rules

- Prefer reliable native live signals without configuration (session/state files, titles, process tree or local APIs). An official mode such as Grok leader may be opt-in. When native signals are insufficient, use the official hook/plugin/extension API to publish per-session state. Ship publishers/configuration examples under `extensions/<kind>/`; install publishers by symlink plus required host configuration, documented in README More agents. Missing optional publisher files or leader socket leaves that kind untracked without errors or extra setup. Use one canonical method per kind; no fallback paths or screen scraping.
- Fail loudly: unknown agent states, tmux errors and a stale index are errors.
- One canonical way per job; no fallbacks.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo install --path . --force               # reinstall the binary after a change so the user can test it
tmux-agent-plane daemon restart              # gracefully restart on the new build
```

Test changes against live agents: spawn throwaway agents with `tmux-agent-plane spawn ... --in window` in a scratch folder, and kill them with `tmux-agent-plane kill` afterwards.

## Changelog

Every commit with a user-facing change adds its entry under `## Unreleased` in that commit, grouped under `### Added`, `### Changed`, `### Fixed` or `### Removed`. Write for users, not as a commit list.

## Releases

A release is a `vX.Y.Z` tag on `main`; pushing it runs `.github/workflows/release.yml`, which checks the tag against `Cargo.toml`, then creates the GitHub release with the tag's `CHANGELOG.md` section (`scripts/release-notes.sh`) as its notes. Never create a release by hand.

1. Make sure `main` is clean and CI is green.
2. Pick the version from `## Unreleased` (SemVer; before 1.0 a breaking change bumps the minor, anything else the patch).
3. In one commit, `Release vX.Y.Z`:
   - rename `## Unreleased` to `## X.Y.Z - YYYY-MM-DD` and add a new empty `## Unreleased` above it;
   - set `version` in `Cargo.toml` and run `cargo build` to update `Cargo.lock`.
4. Check the notes with `scripts/release-notes.sh X.Y.Z`.
5. `git tag -a vX.Y.Z -m vX.Y.Z`, then `git push origin main vX.Y.Z`.
6. Publish to crates.io: `cargo publish`.
7. Watch the workflow (`gh run watch`), then check with `gh release view vX.Y.Z` that the release exists and carries the changelog section.

If the workflow fails, fix the cause on `main`, delete the tag locally and on `origin`, and tag again.
