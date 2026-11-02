# Pi support findings

Verified on 2026-11-02 with Pi 1.0.2 (`@earendil-works/pi-coding-agent`), tmux 3.7c, macOS. Support is opt-in. No installed binary, user daemon, Pi settings or global extensions were changed during implementation.

## Canonical integration

Pi's native JSONL sessions identify the session and cwd but contain no live PID, pane, status or heartbeat. The terminal title is static. Neither is used to infer activity or associate panes.

The official extension API supplies the missing signals. `extensions/pi/index.ts` publishes atomic snapshots at `${XDG_STATE_HOME:-$HOME/.local/state}/tmux-agent-plane/pi/<pid>.json` during interactive TUI sessions inside tmux. Print, JSON and RPC modes do not publish. The Rust adapter reads only these opt-in snapshots; no directory/files means no Pi detection or error, even with a running Pi process.

Primary sources inspected:

- [Official extension documentation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md): extension discovery, lifecycle and final settlement contracts.
- [Extension types](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/extensions/types.ts): `ctx.mode`, `ctx.isIdle()`, `agent_settled`, `ui_prompt_start/end`.
- [Agent session implementation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/agent-session.ts): `isIdle` includes agent work, compaction, retries and queued continuation; `agent_end` alone is insufficient.
- [Extension runner](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/src/core/extensions/runner.ts): prompt events wrap `select`, `confirm`, `input`, `editor`, `custom`, coalescing nested/overlapping prompts.
- Installed version's matching `dist/core/{agent-session.js,extensions/runner.js,extensions/types.d.ts}`, `docs/extensions.md`, `dist/cli/{setup.js,args.js}` and `package.json` under `~/.pi/agent/install/releases/1.0.2/node_modules/@earendil-works/pi-coding-agent/`.

## More agents: Pi opt-in step for README

Keep the standard binary/plugin install unchanged. After installing the repository/plugin, opt into Pi tracking with one extension symlink:

```sh
mkdir -p "${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/extensions"
ln -s "$HOME/.tmux/plugins/tmux-agent-plane/extensions/pi" "${PI_CODING_AGENT_DIR:-$HOME/.pi/agent}/extensions/tmux-agent-plane"
```

For a manual plugin checkout, substitute its absolute path for `$HOME/.tmux/plugins/tmux-agent-plane`. Start Pi again or run `/reload` in existing sessions. Pi 1.0.2 was tested; this integration requires its `agent_settled`, UI prompt lifecycle events and context mode/idle API. Pi must inherit the same `XDG_STATE_HOME` as the daemon. `--no-extensions` disables discovery and therefore tracking. Removing the symlink and reloading opts out.

The extension runs in Pi's process using its supported extension API; it creates no model calls, tools, dialogs, config or session transcript entries. Files contain PID, pane ID, session identity, completion epoch/count, raw state, detail, title and heartbeat. The title is the native session name or latest input's first line (cwd basename before the first input).

## Identity, states and freshness

- `process.pid` + `TMUX_PANE` are published directly by the live Pi extension. Scan requires that PID in the current process snapshot, published `ps` argv equal to `pi`, and ancestry under exactly the named tmux pane's shell. Pi's official CLI sets `process.title = "pi"`; live `ps` confirmed both executable/title and argv as `pi`. This excludes other servers with coincident pane IDs; cwd is never an identity signal.
- `session_id` is exactly `ctx.sessionManager.getSessionId()`. `completion_epoch` is process PID plus `performance.timeOrigin`, stable across extension reloads and native session changes.
- `working` comes from authoritative `!ctx.isIdle()`, with immediate publication at `agent_start` and heartbeat refreshes. It covers automatic continuation/retry/compaction. There is no elapsed-time idle inference.
- `blocked` comes from official `ui_prompt_start`. `detail` is the published prompt title, or prompt kind. `ui_prompt_end` clears that blocking span and reads current `ctx.isIdle()` again. Ordinary Pi has no default tool permission gate; extension confirmation/question dialogs are covered.
- `idle` comes from authoritative `ctx.isIdle()`. `agent_settled` increments a process-lifetime total and publishes synchronously. A process-global object preserves total/epoch across `/reload`, `/new`, and `/resume`, including revisiting an earlier session. Only a new process creates a new completion epoch.
- Atomic rename prevents partial reads. The heartbeat refreshes every 1 second. A live snapshot older than 5 seconds or timestamped in the future is an error, never a state transition. The adapter checks time after reading the snapshot to avoid a concurrent publisher creating a false future timestamp.
- Malformed data, unknown schema/fields/state, invalid live identity, empty session/epoch, negative/missing counters and ambiguous pane matches fail loudly. Dead PID files are ignored before parsing, including malformed dead files. Files under another server's process tree are ignored.
- Shutdown stops the heartbeat and removes its file. Abrupt exit may leave a dead PID file, which is ignored. The daemon alone derives done/seen/completion deltas from raw states and durable counters.

The launch is literal `pi`, with `["--", task]` when supplied, preserving shell aliases and interactive mode while protecting option-like prompts. Pi itself retains its documented `@file` attachment syntax even after `--`.

## Fixtures and reproducible validation

`tests/fixtures/pi/{working,blocked,idle}.json` exercise the actual snapshot schema. Six Rust tests cover all raw states, durable counters/epochs, missing opt-in directory/files, dead malformed files, live malformed/unknown/invalid/stale/future data, same-cwd panes, cross-server ancestry, ambiguous identities and launch syntax.

`tests/fixtures/pi/live-provider.ts` registers a deterministic **local test-only provider**. It returns through the real Pi agent loop after 1 ms or 3.5 seconds and supplies real confirmation/input dialogs, a session switch command and a brief publisher freeze. It performs no network request, credential access or billable model usage. It is never installed with the shipped publisher.

`tests/pi_live.py` is the retained integration harness. With Pi registered in the built branch binary:

```sh
cargo build
python3 tests/pi_live.py
```

It creates a unique ignored `target/pi-live-*/` config, state home, scratch cwd and isolated tmux socket. Only that config receives extension/fixture symlinks. All agent operations go through the locally built plane CLI; raw tmux handles isolated server/client infrastructure only. A private PTY client proves visibility without showing a user terminal. Notification executables are stubbed inside the test environment. The harness kills its test agents through the plane CLI, stops its private daemon/server and retains JSON results/logs in its printed target directory.

Latest full harness: **PASS** at `target/pi-live-397ehd7m/results.json` with exact shared sources from `862398a` temporarily present and Pi temporarily registered:

- Interactive idle startup; live working → unseen done; plain `wait`, slow `spawn --wait` and `send --wait`.
- Three live Pi processes in distinct panes sharing one cwd, each with distinct session/PID identity.
- Confirmation and question blocked details, dismissal and return to idle.
- 1 ms first/follow-up turns settled before a working poll, with durable completion counts and no missed wait.
- `/reload` preserves source count/epoch. `/new` changes native session only; a completed new-session turn adds exactly one. Revisiting the original native session preserves the total and adds no old completions. The next fast turn adds exactly one.
- Private viewer + plane `focus`: done → visible idle.
- Native `/quit`: process disappears from index.
- Frozen live publisher: daemon exits loudly with a stale status error; heartbeat recovery permits a fresh private daemon.

Earlier direct identity proof: Pi PID `99497`, parent shell PID `99474`, pane `%1`, native session `01a19df3-3cfa-76ed-bed2-111f107744d4`, with the same native ID verified in the isolated session header. A second same-cwd Pi ran under `%2` independently.

`cargo fmt --check`, `cargo test`, and `cargo clippy --all-targets -- -D warnings` pass with temporary Pi registration and shared epoch sources. Registration/shared files are reverted before the owned commit.

## Required shared registration and remaining gaps

Orchestrator owns these changes:

1. `src/agents/mod.rs`: `pub mod pi;`.
2. `Kind`: add `Pi`; `as_str()` returns `"pi"`; dispatch `scan: pi::scan, launch: pi::launch`.
3. Shared `Observation.completed_turns` and `.completion_epoch` plus daemon epoch-based delta handling from `862398a` are required. Shared navigation regression and parser/live revisit checks pass together.
4. Update shared README/AGENTS/skill with this opt-in method and interactive launch syntax.

No external provider turn was run: the machine defaults to `openai-codex`, whose usage is prohibited for this task. The deterministic provider exercises real Pi agent/session/UI events; provider-specific login or errors remain untested. Startup trust prompts before `session_start` and Pi-owned dialogs that bypass the extension UI API are not claimed as blocked signals. Unknown older/newer API behavior requires version-specific verification; no compatibility fallback is implemented. Automatic retry/compaction semantics are verified in installed primary code; a live provider-driven retry/compaction was not induced.
