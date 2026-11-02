# Grok Build findings — 2026-11-02

## Shipped method and scope

The single method is Grok's official opt-in **leader roster API**, joined to
its native active-session registry. Supported/tested: macOS arm64, official
Grok **1.0.70 (f50904eaee6c)**, installed using the inspected official
`https://x.ai/cli/install.sh` script. Device-code login completed successfully;
all live probes below were authenticated interactive TUI sessions. Installation
created Grok/agent links, completions and the installer's shell PATH block.
The plane binary and user daemon were never replaced or restarted.

Exact opt-in example: `extensions/grok/config.toml.example`. Merge
`use_leader = true` into the existing `[cli]` table in `~/.grok/config.toml`
(or `$GROK_HOME/config.toml`) and restart Grok sessions. This is user-level
configuration; project `.grok/config.toml` is not the place for `[cli]`.
No symlink/publisher/plugin is needed. Run `grok` normally thereafter.

Home discovery: nonempty `GROK_HOME`, otherwise `$HOME/.grok`.
Socket discovery: nonempty `GROK_LEADER_SOCKET`, otherwise `<home>/leader.sock`.
The plane daemon must share Grok's home/socket environment. Hash-suffixed
nonproduction relay sockets are not scanned; use the explicit override.
A missing socket/registry or no live Grok descendant returns an empty scan.
Ordinary in-process Grok stays untracked. There is no native-title fallback.

Linux **has not been live-tested**. The public source snapshot does not
contain the stable release's `pid_start` semantics. An opted-in live Linux
session errors explicitly rather than accepting possible PID reuse. Users
without the Grok leader opt-in retain existing behavior.

## Identity, state and freshness

`<home>/active_sessions.json` publishes `{session_id,pid,pid_start,cwd,opened_at}`.
The adapter requires the exact PID to be a Grok process beneath a pane's
process root, then joins the **exact session ID** to a local resident roster
entry. Cwd and history never identify a pane. On macOS, the published start
token equals `proc_pidinfo(PROC_PIDTBSDINFO).pbi_start_tvsec * 1_000_000 +
pbi_start_tvusec`; verified against live processes. A different token is a
dead process incarnation and is ignored. Dead PIDs are ignored before parsing
the rest of their record. Multiple resident sessions in one pane are an error.

The native protocol uses four-byte big-endian length-prefixed JSON over a Unix
socket. The adapter registers as `client_type=tmux-agent-plane`, `mode=stdio`,
empty capabilities, requires ready protocol version 1, and issues only the
read-only `_x.ai/sessions/list` ACP request. It does not initialize, load,
subscribe to, mutate, or answer a Grok session. The response is
`result.result.sessions[]`; exact mappings:

- `working` → `RawState::Working`
- `needs_input` → `RawState::Blocked`, detail `input needed`
- `idle` → `RawState::Idle`

Nonresident historical entries are ignored. Unknown live states, malformed
live registry/roster data, duplicate identity, unexpected protocol, unavailable
present server and failed RPCs are errors. A single two-second overall protocol
deadline covers all writes, frame headers, partial bodies and unsolicited
messages; a slow-drip test proves that each read does not restart the deadline.
Timeout is an error, never an agent state. IPC responses provide current state,
so no inactivity window or stale history timestamp supplies freshness.

Both `completed_turns` and `completion_epoch` are `None`. The roster publishes
no safe monotonic completion counter. `updates.jsonl` has durable
`turn_completed` records, but using a history count would need a proven epoch,
rewind and first-attach contract. A turn completed entirely between one-second
samples can be missed; `send --wait` / first-turn `spawn --wait` can time out.
This accepted gap is not reported as a passing fast-turn test. The daemon
alone derives `done` from observed working → idle transitions.

Launch: literal command `grok`, optional task arguments `["--", task]`.
The installed parser accepts the standard positional terminator, so option-like
tasks are protected and quotes/newlines remain one task argument.

## Evidence and tests

Initial no-config probe: session `01a19df2-5ee7-79d1-b0ca-2fbf85d1d97c`,
TUI PID `85028`, pane `%3` rooted at shell PID `84723`, start token
`1793651258535333`. A no-tool HELLO turn emitted native `turn_started`,
`turn_ended` and `turn_completed`. Default `grok leader list --json` was `[]`
and `lsof` showed no named listening socket. The no-config TUI has no safe
external full-state interface: questions retain a spinner/Running title,
`events.jsonl` reports permission allowed immediately while the question still
waits, and pending interactions are not persisted. Permission title sampling
also cannot resolve the documented unfocused blink. These observations explain
why leader mode is required; none drive the adapter.

Leader test: custom socket `.research/grok-leader.sock`, session
`01a19df5-e1a6-7272-85f3-5f6dd2931035`, TUI PID `37992`, pane `%5` rooted at
`37703`. Both an `ask_user_question` choice and a foreground Python command's
permission prompt returned native `needs_input`. Approving the harmless Python
sleep command produced `working`, then `idle`; plane reported unseen `done`.
A private PTY client and `plane focus` changed that record to visible `idle`.
No desktop window was opened.

Retained rerun harness: `extensions/grok/check-live.py`. Run after shared
registration with `cargo build`, then
`python3 extensions/grok/check-live.py "$PWD/target/debug/tmux-agent-plane"`.
It creates a unique detached tmux server, XDG state directory and Grok home,
symlinks existing auth, uses the example leader setting, and keeps command,
index and daemon evidence. It only drives agents through the branch plane CLI.
The private PTY attaches infrastructure; plane performs agent focus. Backend
cleanup uses `grok leader kill` scoped to the dedicated test home.

First complete automated run evidence:
`/var/folders/ll/6ymmghv964ddzxgzk0g867_w0000gn/T/grok-plane-live-1mn2c4x2`.
Passed question blocked via `spawn --wait`, permission blocked via `send --wait`,
working during foreground sleep, `wait` → unseen done, two panes in exactly the
same cwd with distinct native session IDs, idle `spawn --wait`, and exit removal.
Separate private-PTY viewed-idle check also passed. Final complete harness run also passed viewed idle and a literal `--help`
interactive task (asserted from the exact session's native user-message update).
Final evidence: `/var/folders/ll/6ymmghv964ddzxgzk0g867_w0000gn/T/grok-plane-live-i00xb9up`.
`cargo fmt --check`, all **25 tests**, and `cargo clippy --all-targets -- -D warnings`
passed. Fast-turn wait behavior remains the explicit counter gap above.

Parser fixtures under `tests/fixtures/grok/` are captured native protocol
responses for working, idle, question and permission, with only IDs/cwd/title
sanitized. Tests cover their mappings, exact PID association in same-cwd panes,
dead/malformed records, duplicate/ambiguous identity, PID reuse on macOS,
nonresident history, unknown live state, protocol/API errors, bounded frames,
read-only request shape, a single total deadline, absent integration and task
argument preservation. Validation uses temporary shared files from `862398a`
and temporary Grok enum/module/dispatch registration; these shared edits are
restored before commit.

## Primary sources

- Official installation / usage: https://docs.x.ai/build/overview and https://docs.x.ai/build/cli/reference
- Bundled installed docs: `~/.grok/docs/user-guide/26-config-reference.md` documents `cli.use_leader`; `~/.grok/README.md` describes leader mode as opt-in.
- Public source snapshot `xai-org/grok-build` at `2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8`:
  - `crates/codegen/xai-grok-active-sessions/src/lib.rs`: PID/session registry and atomic replacement
  - `crates/codegen/xai-grok-shell/src/agent/roster.rs`: roster API and activity enum
  - `crates/codegen/xai-grok-shell/src/agent/mvp_agent/session_lifecycle.rs`: current pending-interaction/turn state
  - `crates/codegen/xai-grok-shell/src/session/pending_interaction.rs`: question/permission/plan requests are not persisted
  - `crates/codegen/xai-grok-shell/src/leader/{protocol,server}.rs`: native framing and registration
  - `crates/codegen/xai-grok-pager/src/app/cli.rs`: leader opt-in and interactive positional prompt
  - `crates/codegen/xai-grok-pager/src/notifications/title.rs`: unfocused permission-title blinking

The installed stable build commit is absent from that public repository; live
1.0.70 responses are the authority for fixture wire shapes and `pid_start`.
An upstream default read-only per-PID status endpoint (or atomic snapshot with
PID start identity, selected session, explicit state, stable completion epoch
and monotonic completed-turn count) would remove leader opt-in and the fast-turn
sampling gap. Until then the single supported method is leader mode.

## Shared registration needed

Orchestrator: `pub mod grok;` in `src/agents/mod.rs`; `Kind::Grok` with
clap/serde `grok`, `as_str() = "grok"`, and dispatch to
`grok::{scan,launch}`. Shared `Observation` must contain both optional completion
fields from `862398a`. No other shared orchestration changes are required.

## Process-exit race follow-up

A native macOS `PROC_PIDTBSDINFO` probe against absent PIDs `2147483647`
and `2000000000` returned `filled=0`, `errno=3` (`ESRCH`, "No such process").
An active-registry candidate can exit between the scan's process-table
snapshot and this lookup. `process_start` now returns `Result<Option<u64>>`:
only the native zero-result/ESRCH combination returns `None`, and the adapter
skips that vanished process. A mismatched start token remains a stale record
and is ignored. Permission errors and every other lookup failure remain errors.

Regression coverage includes an injected vanished result for a candidate
still present in the process snapshot, propagation of EPERM/EACCES/EIO,
and a real nonexistent macOS PID lookup. This follow-up requires no Grok
model turn, lifecycle harness, launch change, or shared orchestration change.

Validation: `rustfmt --check` and diff checks passed. A separate temporary
validation copy based on shared `862398a`, with this Grok module/fixtures and
registration, passed all 28 tests and clippy `--all-targets -- -D warnings`.
No shared files in this worktree were changed; the branch's launch signature
remains unchanged for the orchestrator's independent `Result<Launch>` update.
