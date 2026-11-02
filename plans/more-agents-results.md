# More agents integration results

Work stays on `more-agents` and its four worker branches. The main checkout, installed plane binary and user daemon were left untouched. Nothing was pushed, tagged or published. Grok Build was installed and authenticated with the lead's authorization.

## Detection and scope

| Kind / tested version | Canonical signals | States and live checks | Gaps |
| --- | --- | --- | --- |
| Pi 1.0.2 | Opt-in official extension API publishes PID, pane, native session, `ctx.isIdle()`, UI prompt lifecycle, process-lifetime completion count/epoch and 1s atomic heartbeat | Working, confirmation/question blocked, idle; daemon done/seen; independent same-cwd sessions; plain wait and fast/slow spawn/send waits; reload/new/revisit; exit and stale publisher failure | Local deterministic provider exercised real Pi UI/loop without external inference; provider login/retry/compaction not live-induced. Pre-session trust and dialogs outside extension UI API are outside the proven scope. |
| OpenCode 1.19.3 | Opt-in TUI plugin publishes live PID/pane/route session; native status/permission/question snapshots; ordered status events supply a launch-total completion count with stable epoch | Working/retry, permission/question blocked, idle; daemon done/seen; same-cwd identity; both waits including repeated fast turns; exit. Regressions cover out-of-order snapshots/events and atomic fresh state/count publication. | Local HTTP model fixture; provider auth/retry, Linux, other releases, remote attach and deep/background workflows not live-tested. Generic menus are outside turn blocking. |
| Antigravity 1.4.12 | Native process argv and open read/write presence descriptor identify an interactive PID/session; bundled SQLite reads live summaries, background-work flag and current-turn pending permission/question interaction | Working/background work, permission/question blocked and idle; daemon done/seen; same-cwd identities; task-bearing question spawn-wait, permission send-wait, plain state checks, observed short waits, exit. Abrupt kill/resume cleared pending state. | First-turn publication scope accepted by lead: manual sessions appear after their first message; taskless spawn fails before creating a pane. Sampled completions may evade waits. Linux runtime requires separate validation; BUSY/CANCELING were descriptor-tested, not forced live. Unsupported interaction variants error. |
| Grok Build 1.0.70 | Official opt-in leader mode; native active-session ID/PID/macOS start token joined to resident local roster `working`/`needs_input`/`idle` | Authenticated question and permission blocked; working; unseen done to private-viewer idle; same-cwd identity; idle spawn; literal option-like task; exit; plain/send/spawn waits | No durable completion counter: fully missed working transitions can time out a wait. Linux start-token semantics are unverified, so live opted-in Linux tracking errors explicitly. |

`done` remains daemon-owned. Pi/OpenCode integrations are independent symlink opt-ins; Grok's opt-in is one configuration entry. Missing optional files/socket means no tracking and no setup/error for users who do not use those kinds. Unknown live state, stale opted-in publishers and ambiguous identity fail loudly. No terminal content supplies state.

## Shared behavior

- Agent adapters own detection, cleaned titles and validated interactive launch syntax; launch errors are checked before any pane creation. Shared detection takes one process snapshot and rejects ambiguous pane identities.
- Durable completion epochs prevent both missed sub-tick turns and old-session completion replay. OpenCode publishes a completion only alongside a newly verified native state snapshot.
- Codex tool-shell caller identity is unknown when a shared app-server supplies inherited `TMUX_PANE`. Listing marks no self row; window/session spawns work with `parent: null`; pane splits and inherited/self targets error. The four original workers retain their old `%14` parent records, as requested.
- The README and skill document agent setup, launch syntax, pre-turn scope and sampled waits. A separate worker reviewed seven command-selection scenarios against the updated skill.

## Reproducible checks

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
node --test extensions/opencode/plugin.test.mjs
cargo build
python3 tests/caller_live.py
python3 tests/pi_live.py
python3 extensions/opencode/test-live.py --binary target/debug/tmux-agent-plane
python3 extensions/grok/check-live.py "$PWD/target/debug/tmux-agent-plane"
# Authenticated Antigravity in a trusted checkout:
python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane
```

Every live harness uses an isolated tmux server and XDG state, detached test windows and private PTY/control clients. Agent jobs use the branch CLI; test notifications are suppressed. Test agents and isolated backends are cleaned up. Pi/OpenCode model fixtures make no external inference calls. Grok/agy checks use authenticated native apps.

Integrated evidence:

- Pi: `target/pi-live-fym2gjb7/results.json` — full harness passed.
- OpenCode: `target/opencode-live-8a__1rl3/` — full harness plus final publication fix passed; all 8 publisher tests passed.
- Grok: `/var/folders/ll/6ymmghv964ddzxgzk0g867_w0000gn/T/grok-plane-live-ddgevram/` — full harness passed; fast-turn uncertainty explicitly retained.
- Codex caller: `target/caller-live-8x_4p8ef/` — missing/blank agy task preflight, listing, self/pane-split rejection and real idle window/session spawns with no parent passed; no model turns submitted.
- Antigravity: `target/agy-live-76ecdecc7835/results.json` — full merged native harness passed. Its question-key delivery waits one second after native blocked so the TUI can install its handler; this pause never supplies an agent state. Native background-work and interrupted-resume proofs are retained in the worker findings and parser fixtures.

Final validation: **49 Rust tests**, **8 OpenCode publisher tests**, `cargo fmt --check`, and `cargo clippy --all-targets -- -D warnings` pass. All four retained live suites passed on the integrated branch. The Grok exit-race followup additionally proves the actual macOS ESRCH result and ignores only vanished candidates; other native lookup errors still fail loudly.

Installed-binary/user-daemon replacement was not performed. Branch integration and isolated checks are complete; the standard install/restart verification remains conditional on the lead authorizing that separate step.
