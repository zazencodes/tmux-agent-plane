# Sidebar notification events

2026-11-02 · item 4 user choice · base `1e81f72`

Normal off-screen done/blocked notifications now use the existing sidebar footer. The daemon publishes one latest event in its index, and every open sidebar renders it on its bottom line in the state's colour for five seconds. Local rename failures take priority, followed by active rename controls, the live event, then normal hints. Index errors still occupy the main panel and clear any cached event. A closed sidebar stays closed.

The former normal Client delivery queue could retain a disconnected destination indefinitely. Removing that destination also removes the stale-client retry path. Desktop delivery still isolates failures, retries only unsuccessful deliveries, and continues across independent notifications. Diagnostic tmux announcements, the persistent error badge, durable full error log, fault isolation, recoverable presentation and detached lifecycle are preserved.

The existing off-screen state-change policy remains. One requested correction covers durable Pi/OpenCode completions: when `completions` increases while an unseen agent remains `done`, both the footer and desktop paths notify for the new completion. Repeated samples with the same count stay quiet. Visible agents and initial observations stay quiet.

## Index contract

`Index.recent_event` is `Option<RecentEvent>`, defaulting to `None` when omitted by an older index. The JSON object has:

- `name`: string, capped by the daemon at 128 Unicode characters.
- `state`: `done` or `blocked`; the `EventState` enum rejects other states during deserialization.
- `occurred_at`: Unix milliseconds from the daemon transition tick.
- `expires_at`: `occurred_at + 5000` milliseconds.

Only one event is retained, independent of window/session. A sidebar checks `occurred_at <= now < expires_at` on every draw and rejects a lifetime exceeding 5000ms. The daemon drops expired metadata on its next tick. Restarting the daemon starts with no recent event, retaining agent state/counters without replaying the old notice. No new setting, notification pane, popup or layout operation was introduced.

## Proof

Run:

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
uv run tests/sidebar_events_live.py target/debug/tmux-agent-plane
```

All checks passed: 65 Rust tests and a private live run retained at `/tmp/tap-footer-bxmkl87a/` (`results.json`, both PTY byte streams and text snapshots, native fixture publishers, daemon logs and tmux command audit).

The live harness runs a foreground `daemon serve` under a private HOME and short explicit `/tmp` XDG state/socket paths. It spawns and kills native-format Claude/Pi fixtures through the branch CLI with explicit names and window placement. No real agent CLI, model, credentials, installed binary, user daemon or configuration is used. PTY decoding verifies UI only; state comes from native fixture files and the daemon index. The inherited `NO_COLOR=1` is cleared only inside this colour test environment.

Observed/asserted:

- Two independent clients/sessions render the same event in footer row 28; done is `#a6e3a1` and blocked is `#f38ba8`.
- A second completion while still Done updates the real event timestamp and expiry.
- Expiry returns both footers to hints; restarting during a live blocked notice clears it without replay.
- Events with all sidebars closed leave geometry unchanged.
- Keys `a`, `b`, `c` enter through the original agent client's PTY and reach that fixture; its indexed visibility, selected focus and every window layout remain unchanged. No focus-out reaches the fixture during notification checks.
- Injected desktop failures leave both agents/index/footer alive. Three expected desktop deliveries (including two failed then retried completions) complete once delivery recovers. Fault/recovery remains logged and diagnostic tmux messages remain active.
- The audited normal-event `display-message` call list is empty. The remaining calls are formatting queries and diagnostic messages.

Coverage is macOS/tmux 3.7c and synthetic native publishers; OS desktop presentation is deliberately stubbed. No actual agy process runs. Root owns migrating the pre-existing diagnostics harness's normal tmux notification assertions and shared lifecycle/stale-index documentation.
