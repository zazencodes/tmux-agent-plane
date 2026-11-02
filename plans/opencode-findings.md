# OpenCode Phase 1 findings

Verified 2026-11-02, macOS arm64, installed `~/.opencode/bin/opencode` **1.19.3**.
`opencode --help` identifies the positional argument as the project path, and
`--prompt <text>` as the interactive initial prompt. The adapter launches literal
`opencode` with `--prompt` for a task; shell aliases continue to apply.

## Native signals first

Primary source: official release [v1.19.3](https://github.com/anomalyco/opencode/tree/v1.19.3),
commit `e3c4eb242585dcd77be1a867391b7c25fd47ce52`.

- [`cli/cmd/tui.ts`](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/cli/cmd/tui.ts)
  creates an in-process worker transport at `http://opencode.internal` by default.
  The HTTP listener starts only with explicit network flags. There is no default
  discoverable local server endpoint to query. Adding launch flags would change
  ordinary agent startup and still would not publish the active TUI route.
- [`session/status.ts`](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/opencode/src/session/status.ts)
  holds current statuses in memory, emits `session.status`, and deletes a map
  entry when it becomes idle. An absent entry in a **successful live status map**
  means idle according to this implementation. It does not mean missing history
  or an elapsed timeout.
- Native database/history/preferences and static terminal titles do not establish
  the active interactive PID/pane/session and current blocked state.

The single supported method is the authorized, opt-in **official TUI plugin**.
No state files means no OpenCode observation, even with running OpenCode processes.
There is no native-signal fallback.

## Opt-in install step for README More agents

Require OpenCode **1.19.3** (the version verified here). After the standard binary
and tmux plugin install, opt in by linking the publisher and adding one TUI plugin
entry. Replace `/absolute/path/to/tmux-agent-plane` with the installed checkout:

```sh
opencode_config="${XDG_CONFIG_HOME:-$HOME/.config}/opencode"
mkdir -p "$opencode_config"
ln -s /absolute/path/to/tmux-agent-plane/extensions/opencode/tmux-agent-plane.js \
  "$opencode_config/tmux-agent-plane.js"
```

Add `"./tmux-agent-plane.js"` to the `plugin` array in
`$opencode_config/tui.json` (or the existing `tui.jsonc`), retaining every other
setting and entry. If neither file exists, create `tui.json` with:

```json
{
  "$schema": "https://opencode.ai/tui.json",
  "plugin": ["./tmux-agent-plane.js"]
}
```

Restart the opted-in OpenCode TUI. Do not overwrite an existing config or use
`ln -f`. `extensions/opencode/tui.example.json` contains the example. Global
`plugins/*.js` autodiscovery is server-only in this release and cannot load this
TUI integration. `--pure` disables external plugins and therefore tracking.
The symlink plus array entry is the one integration step; standard installation
requires neither. No npm package, external dependency, wrapper or agent hook is
needed. The live tests loaded this `.js` symlink through the official loader from
an isolated config directory **without a config-local package.json/type:module**.

## Published contract

Source: `extensions/opencode/tmux-agent-plane.js`. It uses the official
[`TuiPluginApi`](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/plugin/src/tui.ts):
`route.current`, `state.ready`, `state.session.get`, `client.session.status`,
`client.permission.list`, `client.question.list`, `event.on`, and lifecycle disposal.

Every 250 ms, after synchronization, the plugin queries the native worker through
its existing SDK client. This works without an HTTP listener. It discards snapshots
superseded by status events or route changes. A per-PID JSON snapshot is atomically
renamed into `$XDG_STATE_HOME/tmux-agent-plane/opencode/<pid>.json`, defaulting to
`~/.local/state/tmux-agent-plane/opencode/<pid>.json`. It publishes:

- `version: 1`, native TUI `process.pid`, and inherited `TMUX_PANE`;
- native active `session_id` and title; home uses a launch-unique home identity;
- `working` for native `busy`/`retry`; `blocked` for pending permission/question
  requests with a useful detail; otherwise native live idle;
- permission/question membership for the selected root plus direct child sessions,
  matching OpenCode's own
  [session UI](https://github.com/anomalyco/opencode/blob/v1.19.3/packages/tui/src/routes/session/index.tsx);
- `updated_at`, time of the last **successful native API snapshot**. Completion
  events update only the internal counter; successful snapshots atomically publish
  current state, count and freshness. Events never republish cached state;
- launch-unique `completion_epoch` and monotonically increasing `completed_turns`.
  Only the ordered `session.status` event stream counts live busy/retry → idle
  transitions. API snapshots never mutate the completion lifecycle: native idle
  snapshots can precede queued busy/idle events. Duplicate idle events,
  initial idle/history, route/home/revisit changes and unrelated background
  subagent completions do not add completions. The counter survives route changes;
  epoch changes when the publisher starts again. Native session IDs remain native.

The Rust scan first checks the filename PID against the process snapshot and
requires native executable basename `opencode` descended from exactly one pane
shell. It then validates file PID/pane identity, version, nonempty session and epoch,
known state, and snapshot freshness (at most 5 s old, at most 1 s into the future).
PID files outside this server's pane ancestry, or whose PID no longer exists, are
ignored before JSON parsing. Live malformed, unknown, stale or mismatched data
errors loudly; an API failure publishes an error that the adapter rejects.
Unknown native statuses and unknown TUI routes also error. Time is used only to
reject stale publication, never to infer idle. The daemon alone derives done/seen.

## Registration owned by orchestrator

- `src/agents/mod.rs`: `pub mod opencode;`
- `src/index.rs`: `Kind::Opencode`, string `"opencode"`, and adapter dispatch
  `{ scan: opencode::scan, launch: opencode::launch }`.
- Shared completion contract from `145169d` and `862398a`: adapter observations
  carry `completed_turns: Some(n)` and `completion_epoch: Some(nonempty_launch_id)`.

These registration changes are temporary during worker verification and excluded
from the owned implementation commit. No changes to commands, sidebar, tmux plugin,
main checkout, installed binary or the user's daemon are needed from this worker.

## Reproducible tests and results

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --test extensions/opencode/plugin.test.mjs
cargo build
python3 extensions/opencode/test-live.py --binary target/debug/tmux-agent-plane
```

The live harness requires the merged/temporarily registered binary. It retains
artifacts under the printed `target/opencode-live-*` directory. It starts a unique
isolated tmux server with no user tmux config and unique XDG state/config/data/cache
roots, an isolated daemon and detached windows. The only model is its retained
local OpenAI-compatible HTTP fixture; it does not invoke a paid/external provider.
It suppresses desktop notifications and uses a control client to test visibility
without opening anything on the user's screen. Every agent operation uses the
branch plane CLI; raw tmux is limited to server/client infrastructure. Cleanup
kills test agents through plane and removes only the isolated server.

Passed against real OpenCode 1.19.3 with the completion-epoch shared implementation:

- Two same-cwd TUIs with distinct native PID → pane → session identity.
- Native working (delayed local model response), idle and unseen done.
- Bash permission blocked, acceptance and completion.
- Question tool blocked, answer and completion.
- `spawn --wait` and `send --wait` on fast first/subsequent turns, plus plain `wait`.
- Unseen done → viewed idle, with unchanged completion count.
- Graceful interactive exit removes the agent; remaining test agents killed.
- Official `.js` loading through symlink + config entry, no config-local module package.

Rust fixtures in `tests/fixtures/opencode/`: working, permission, question, idle,
unknown and malformed JSON. Five adapter tests cover state/detail/count/identity,
missing opt-in, ignored dead malformed PID files, stale/future/version/identity
failures, same-cwd panes, duplicate sessions and launch syntax. Publisher tests
cover fast events, duplicate idle, native retry, permission/question including child
requests, route/home/revisit epoch stability, ignored subagent completions, unknown
native status, API failure, disposal, and idle-snapshot/queued-event races.
The race regression was first run against the mixed event/snapshot implementation
and failed by counting a turn twice, then passed with event-only counting. Shared daemon tests cover durable first
turns, no double count, regression errors and native navigation.

## Explicit gaps and limits

- Verified on macOS arm64 and OpenCode 1.19.3. Linux and other OpenCode releases
  need a live rerun. Native SDK/TUI API drift is an error, not an idle fallback.
- The plugin supports the full TUI; `opencode run`, `serve`, `acp`, and `--mini`
  do not host it and are outside this interactive adapter.
- Remote attach, workspace switching, deeply nested/background subagent workflows
  and provider retry were not live-tested. Native retry and direct child blocked
  requests have publisher tests. History is never a source of current state.
- Provider login was unnecessary with the local fixture. External provider auth
  and network behavior are not claimed as tested.
- Generic menus/model pickers do not have native turn-blocking semantics here;
  only actual pending permission/question requests publish blocked.
- Mid-turn daemon startup cannot recover completions before publisher activation.
  It does preserve turns observed after activation, including sub-tick turns.

Follow-up publication regression: a native idle event between two API polls must
leave the previous busy snapshot unchanged. Only the next successful idle API
snapshot publishes the incremented completion counter, so the daemon cannot
consume the delta while still observing cached working state. The test failed
against the cached-event publication and passed after removing it. Error snapshots
include the required session/title fields so the adapter reports the original
source error detail through its unknown-state error.
