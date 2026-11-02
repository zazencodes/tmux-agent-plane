# Integrated fixes proof

2026-11-02. Branch `fixes`, from main `9c8085c`. No install, main-checkout changes, push, tag or publish. Sidebar navigation is separately assigned to `fix-sidebar-navigation` and excluded from this fixes release.

## Results

| Item | Cause and final behavior | Integrated proof |
| --- | --- | --- |
| Antigravity trust | Native summaries can finish behind folder trust. Native saved trust, exact OS argv overlays and selected default-project identity gate tracking. Untrusted sessions remain untracked. | Real Antigravity private-HOME run: `target/agy-live-2fbbe6e10798/results.json`. Default acceptance, inline/file grants and exclusions, questions/permission/working/done/viewed idle, two identities, fast waits and native exit passed. |
| Error survival | Scan/advance/notification/presentation failures formerly escaped the loop. Scoped faults withdraw bad rows, keep healthy agents/index updating, log full chains once, and announce degradation/recovery. Fatal index/log/process/pane errors remain fatal. | `/tmp/plane-daemon-errors-ws9eke6l`; output `/tmp/fixes-verify-evidence/daemon-errors-final.log`. Covers kind isolation, all-kinds failure, counter regression/recovery, desktop retries, sidebar events, refresh failure, fatal index/log, injected pane failure, duplicate worker and actual private-server shutdown. |
| Detached lifecycle | tmux owned a long-lived run-shell worker, so its failures painted job output over the pane. Public start/restart/stop use a detached worker/control socket and short successful plugin job. | `/tmp/plane-lifecycle-44mz_gty`: all 13 cases passed, with raw failing run-shell as a calibrated negative control and no overlays/jobs for the public lifecycle. |
| Sidebar notices | Normal display-message notifications followed the top status line. One latest five-second index event now paints the existing sidebar footer; desktops and diagnostics remain. | `/tmp/tap-footer-xt8rqj0k/results.json`: two private clients/sessions, green/red row 28, expiry, restart clears, closed stays closed, pane input `abc`, unchanged focus/layout, desktop retry, no normal display-message calls. |
| Blocked sends | A normal status prompt was pasted into an open question. Ordinary sends now reject before buffer/key calls; explicit `--answer` requires blocked. | `/tmp/bs-sxfn1gy5`: refusal has zero bytes/mutations, multiline reply exactly once, stale-answer rejection, reply/new-question waits, timeout and stale index. |

## Broader regression checks

- Caller `/tmp/caller-live-3ecpf2bh`: Codex caller/inherited-pane rejection, list, null-parent window/session launch, agy no-task preflight. No Codex prompt/model turn.
- Pi `/tmp/pi-live-nyjvmw66`: actual Pi loop with local provider, fast turns, native session/new/resume, dialogs, visibility/exit and stale withdrawal/recovery.
- OpenCode `/tmp/opencode-live-zocnkh0j`: actual TUI plugin with local model, identity, durable fast completions, questions/permission, waits, viewed idle and exit.
- Grok `/tmp/grok-plane-live-63esclz9`: authenticated native leader, question/permission/working, unseen done/viewed idle, same-cwd identities, option-like task, idle spawn and exit. Private credential copy removed at exit. Fast no-tool wait happened to pass; sampled-turn limitation remains.
- `cargo fmt --check`, 67 Rust tests, `cargo clippy --all-targets -- -D warnings`, 8 OpenCode publisher tests, and Python syntax checks passed.
- Reference skill RED found missing trust/lifecycle guidance; notify-research GREEN read-only scenarios recovered correct native trust exclusions, diagnostic log and managed restart after the update. Blocked-send skill already passed four peer scenarios.

## Isolation and limits

Antigravity agent and daemon both used private HOME; all infrastructure propagated that environment. Real user settings hash matched the post-lead-cleanup baseline in finally. Root also checked it after all native runs: `6369d2a32101dd0b6acc175b5b140ececcea2a4f72e35fcb7ecf178781f9e98c`. The earlier worker harness isolation mistake and lead-owned cleanup remain documented in `agy-trust-fix-findings.md`; no claim is made that the earlier run was isolated.

Verification worker's 793 binary/settings hashes matched; installed daemon PID/start/command remained unchanged. Test agents, isolated servers and native backend processes were cleaned up. Real desktop alerts were stubbed.

The original historical daemon exit cause remains unknown: no old log or tmux job record survived. Newly identified silent list-panes/server probe exits now preserve the original fatal reason, but are not assigned as that historical cause.

Accepted limits: Antigravity named projects (including resume/fork) and overlays excluding cwd from trustedWorkspaces remain untracked; ordinary agy needs its first message and trust acceptance. Managed restart refuses old unmanaged watchers without PID guessing; the one-time 0.1.x migration is an Unreleased upgrade note for plane-lead to perform. Linux native behavior was not live-tested here. Sampled kinds can miss sub-tick turns; Pi/OpenCode durable counters address that gap. Blocked-send uses the latest index and has no atomic question identity.
