# Daemon error isolation findings

The old daemon propagated scan, completion-fold and notification errors directly out of its loop. Detection discarded all observations when any adapter failed. There was no durable error log. The historical exit after agy spawn remains **unknown**: read-only inspection found no daemon log in the current state directory, and `tmux show-messages -J` returned no job records. Timing alone does not establish an agy scan failure.

## Change

- `diagnostics::Batch<T>` carries successful values and explicit scoped issues. A failed kind scan withdraws that kind's observations; other kinds continue. Missing/ambiguous identities fail the involved kinds, including both claimants of a pane.
- A failed `advance` withdraws only that observation. Last good counters and unseen state remain internal while the pane is live, with lookup restricted to the same kind. Successful observations of a different kind replace that baseline; dead panes discard it. Regression stays erroneous across ticks until the counter catches up or its publisher epoch changes.
- Notification destinations deliver independently. Failed destinations retry on following ticks; successful destinations are removed. Desktop stderr and exit status enter the full diagnostic chain. Error announcements use tmux only and cannot feed the desktop queue or recursively announce their own delivery failure.
- `state_dir()/daemon.log` appends timestamped full chains, daemon activation, distinct new faults and recoveries. Repeated identical faults remain in the degraded status segment without repeating log records. Errors remain visible with zero healthy agents. Tmux messages announce fault/recovery changes.
- Shared process-table, index/log writes and operational tmux failures remain fatal. `daemon::report_fatal` persists the full chain and best-effort stopped status/tmux message; the returned error includes the log path. If persistence itself fails, stderr includes both reasons and the path. The existing `daemon.lock` remains the index singleton lock.
- Lifecycle integration calls `daemon::request_shutdown()` and uses `daemon::report_fatal(&error)` for its own runtime failures. Only the `mod diagnostics;` registration is included from `main.rs`; lifecycle owns the remaining CLI wiring and removes the temporary unused-public-API allowance.

## Proof

`cargo fmt --check`, `cargo test` (59 tests), `cargo clippy --all-targets -- -D warnings`, and `cargo build` pass.

Run the retained live harness with:

```sh
python3 tests/daemon-errors-live.py target/debug/tmux-agent-plane
```

After lifecycle integration, pass `--serve` to select its explicit internal foreground command. Root can migrate the harness to that final CLI directly. The harness builds native-format Claude/Pi publisher fixtures, starts its own unique tmux server and XDG state directory, uses a private PTY client, and replaces both desktop commands with stubs. Fixture agent operations use plane spawn/kill with explicit names and `--in window`; no real agent/model request or trust-setting edit occurs. The installed binary, user daemon and user tmux configuration remain untouched. It prints and retains its evidence directory.

The live assertions cover:

1. Unknown native Claude state: Pi stays tracked and the index keeps advancing; repeated failure logs once; recovery restores tracking. With both fixtures erroneous, an empty agent array still shows both faults.
2. Desktop command exit 23 with injected stderr: both agent completions and both private-client tmux messages are delivered, the index stays fresh, and only failed desktop destinations retry. Removal of the fault emits recovery once.
3. Pi completion count 5 → 3: Pi stays withdrawn across repeated ticks while Claude remains tracked. Count 5 restores the original five completions and unseen `done` state without replaying notifications.
4. `agents.json.tmp` changed to a directory: daemon exits nonzero, full index-write reason is persisted, stopped status appears, and stderr names the log.
5. `daemon.log` changed to a directory: daemon exits nonzero and stderr explains failed persistence with the full path.

Unit tests additionally cover both ambiguous claimants, missing panes, same-pane kind replacement, dropping dead baselines, failures to other tmux clients, distinct errors sharing a component, and logger failure during a diagnostic update.

## Limits

The old exit cause cannot be reconstructed without a retained error record. Live checks use native-format fixtures rather than paid agent turns. This change isolates failures at adapter-kind scope; an adapter's own scan remains atomic. The public agent schema is unchanged, and health issues are surfaced in the tmux status/messages and durable log.
