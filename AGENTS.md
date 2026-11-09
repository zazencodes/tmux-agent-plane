# AGENTS.md

tmux-agent-plane: a Rust CLI, tmux plugin and agent skill that track and steer Claude Code, Codex, Pi, OpenCode, Antigravity and Grok Build agents running in tmux. See README.md for behaviour.

## Layout

- `src/agents/`: one adapter per kind owns live state signals, title parsing and interactive launch syntax; `mod.rs` holds scan/process/observation types.
- `src/detect.rs`: takes one process-table snapshot, joins adapter observations to tmux panes and rejects ambiguous identities.
- `extensions/`: opt-in Pi/OpenCode publishers and Grok leader configuration. Keep install instructions in README More agents.
- `src/daemon.rs`: one-second loop. Folds detection into `Done`/seen state, writes the index, sets the `@agent_plane_status` tmux option, notifies.
- `src/fit.rs`: keeps hidden windows at the size they will be shown at, so arriving at one resizes nothing. Runs from the daemon's tick on its existing snapshot.
- `src/diagnostics.rs`: persistent full-chain errors, deduplication, fault/recovery status and partial scan results.
- `src/lifecycle.rs`: detached watcher start/restart/stop and private control socket.
- `src/links.rs`: who spawned whom. `spawn`/`restore` write the parent's pane on the child's pane; the daemon binds the link to both agent processes, clears it when the child is gone and reports an ended parent.
- `src/index.rs`: the index file (`~/.local/state/tmux-agent-plane/agents.json`) and its types. The daemon is its only writer.
- `src/restore.rs`: the daemon's roster of agent windows and running tasks, the pending restore a new tmux server inherits, and `restore`'s rebuild. `src/restore/layout.rs` removes sidebars from recorded layouts.
- `src/commands.rs`: `list`, `read`, `send`, `keys`, `focus`, `kill`, `wait`, `spawn`. Agents are told never to use raw tmux on agent panes, so every agent job needs a command here.
- `src/sessions.rs`: session/project inventory and creation, exact spawn destinations and guarded native agent-pane moves.
- `src/projects.rs`: a hand-started agent joins the session named after its folder, once per pane, from the daemon's tick.
- `src/ui.rs`: the picker popup (`prefix + a`, or `prefix + f` to start searching): the sidebar's list and actions, drawn with a large preview and ranked search.
- `src/ui/tree.rs`: `p`, the list as a tree of who spawned whom: tree order, connectors and edge styles shared by the sidebar and the picker.
- `src/sidebar.rs`: the always-on agent list in a sidebar pane (`prefix + S`), and its toggle. Idle, a sidebar runs no tmux commands: it learns everything from the index, and only keys run tmux. The plugin’s layout hook closes lone sidebars.
- `tmux-agent-plane.tmux`: plugin entry point. Binds the key and starts the daemon.
- `skill/tmux-agent-plane/SKILL.md`: the agent skill. Keep it in sync with the CLI.

## Rules

- Create worktrees under the primary checkout's `.worktrees/<branch>`, including when working from a linked worktree. Keep all project checkouts inside the primary checkout; remove clean, merged worktrees when no live agent uses them.
- Always stage and commit completed work in reasonable, coherent chunks as part of the task, following the `git-commit-conventions` skill, without waiting for the user to ask. Keep unrelated work out of those commits.
- Prefer reliable native live signals without configuration (session/state files, titles, process tree or local APIs). An official mode such as Grok leader may be opt-in. When native signals are insufficient, use the official hook/plugin/extension API to publish per-session state. Ship publishers/configuration examples under `extensions/<kind>/`; install publishers by symlink plus required host configuration, documented in README More agents. Missing optional publisher files or leader socket leaves that kind untracked without errors or extra setup. Use one canonical method per kind; no fallback paths or screen scraping.
- Fail loudly: withdraw faulty kinds/observations, keep healthy tracking, and persist errors/recovery in daemon.log with degraded status. Process/pane snapshot, index and log failures remain fatal; stale index reads fail.
- One canonical way per job; no fallbacks.
- New shortcuts use lowercase keys. Never propose or add a capital-letter (Shift) key.
- The sidebar and the picker stay separate programs with their own layouts and previews, but look and read as one product: session colours, state and blocked styles, and shared messages stay in sync, using shared components in `src/ui.rs`. A change to one that shows in both goes in both.
- Agent counts live only in the status bar, which stays visible whatever else is open and also shows faults. Lists do not repeat them.
- After every fix or change lands, without waiting to be asked, run `cargo install --path . --force` and `tmux-agent-plane daemon restart` so the user can test it on their system. There are no exceptions: do this for plugin-only or docs-adjacent changes, and when other agents have uncommitted work in the checkout. Open sidebars keep the code they started with, so tell the user to close and reopen them with the sidebar key.

## Next steps

If `.agents/TODO.md` exists, read it before starting work: it is the maintainer's local list of what to do next. It is not in git. Keep it current: tick or remove items as they land, and add follow-ups as they come up.

## Development

```bash
cargo test
cargo clippy --all-targets -- -D warnings
cargo install --path . --force               # reinstall the binary after a change so the user can test it
tmux-agent-plane daemon restart              # gracefully restart on the new build
```

Use isolated tmux servers and short `/tmp` state roots for live tests. Antigravity agents and daemons must both use a private HOME; check the real settings.json hash in finally, and never change user trust/settings during tests.

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
