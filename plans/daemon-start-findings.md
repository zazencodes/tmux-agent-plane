# Daemon lifecycle findings

Cause: the plugin kept `tmux-agent-plane daemon` as a long-running `tmux run-shell -b` job. tmux owns that job's output/status and opens pane view mode when it terminates with a signal or nonzero status. The documented process-name `pkill` restart triggered exactly that path.

Fix: the plugin invokes the short `daemon start` control command. It launches one detached `daemon serve` process with `setsid`, null stdin and append-only stdout/stderr in the shared `daemon.log`. There is no supervisor process. The private mode-0600 Unix socket handles start/stop/restart, and a control thread or SIGTERM/SIGINT handler requests graceful shutdown through the daemon's atomic flag. The existing daemon-owned index lock is preserved. A command lock serializes lifecycle operations; a control lock spans the full worker lifetime, including socket cleanup after the daemon loop releases its index lock. This prevents an exiting worker from unlinking a successor's socket. Restart waits for shutdown before starting the current executable, and refuses an unmanaged index-lock owner rather than guessing a PID.

Daemon fatal errors use the diagnostics worker's existing report wrapper exactly once. Lifecycle setup/control faults use its public fatal reporter; CLI control failures log their full cause and return nonzero. The plugin handles a failed start by showing a diagnostic message while returning success to tmux, so the plugin shell job cannot open failure output. README and AGENTS use the single command `tmux-agent-plane daemon restart`.

Proof: `cargo fmt --all --check`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, and `cargo build`; `python3 tests/daemon_lifecycle_live.py` uses an isolated tmux server, unique XDG state, notification stubs and a private attached PTY client. The negative control (`run-shell -b 'exit 7'`) produces `pane_in_mode=1`, `pane_mode=view-mode`. All lifecycle cases then assert `pane_in_mode=0`, empty pane mode, no tmux jobs, and one unchanged scratch pane:

- plugin start, plugin duplicate load, and missing-binary installation error;
- concurrent duplicate starts, preserving the original PID;
- targeted graceful restart with a new PID;
- normal stop and already-stopped stop;
- SIGTERM with signal 15 recorded in the log;
- forced fatal index-write exit, with log reason and stopped status;
- startup failure from a malformed index, with diagnostics and no failed plugin job;
- refusal to target an unmanaged daemon lock owner;
- control-socket protocol failure stops loudly with diagnostics;
- successful recovery after fatal failure.

The harness also verifies that the detached daemon's process group ID equals its PID and counts exactly one fatal record per intentional fault. It retains `assertions.json`, `daemon.log`, and the private client's PTY bytes in a printed `/tmp/plane-lifecycle-*` directory. Final live artifact path is recorded in the worker report.

Limits: an already-running pre-lifecycle binary has no private control socket and cannot be safely targeted by this protocol; the command fails loudly. Lead accepted a one-time stop during installation; the Unreleased upgrade note records this migration. SIGKILL cannot execute shutdown/logging code. No installed binary, user daemon/configuration, trust settings, agent sessions, or main checkout were changed.

Final integrated evidence: lifecycle `/tmp/plane-lifecycle-zhi29me6` (13 passing cases plus negative control), and diagnostics `TMPDIR=/tmp python3 tests/daemon-errors-live.py target/debug/tmux-agent-plane --serve`, `/tmp/plane-daemon-errors-vj2di86h` (all passed, including fatal stderr and broken-log reason/path). Short `/tmp` state roots keep Unix socket paths below macOS sockaddr_un limits.
