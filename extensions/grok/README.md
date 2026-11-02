# Grok Build leader integration

Grok Build 1.0.70 on macOS is supported through its official leader API.
Merge `use_leader = true` into the existing `[cli]` table in
`~/.grok/config.toml` (or `$GROK_HOME/config.toml`), using
[config.toml.example](config.toml.example), then restart Grok sessions.
Run `grok` normally, or `tmux-agent-plane spawn grok "your task" --in window`.
This integration needs no hook, plugin, symlink, or status-file publisher.

The daemon and Grok must use the same `GROK_HOME`. The socket is
`$GROK_LEADER_SOCKET` when nonempty, otherwise `$GROK_HOME/leader.sock`
(default `~/.grok/leader.sock`). A custom override must also be present in the
plane daemon's environment. Alternate relay/hash-suffixed leader sockets are
outside this integration's discovery scope; set the explicit override.

The adapter joins `active_sessions.json`'s session ID and PID to a process
under a tmux pane and to the leader's resident local roster entry. It checks
macOS's process-start token to reject PID reuse. Multiple resident sessions
in one pane are an ambiguity error. A missing socket, registry, or live Grok
process means no detection. A present server with live registry candidates
must answer valid protocol-v1 roster data within one two-second deadline;
connection failures, malformed data and unknown live states are errors.
Nonresident historical roster entries are ignored.

The API maps `working`, `needs_input`, and `idle` to plane states. The daemon
alone derives `done`. Questions and permissions both report `needs_input`.
No terminal content, title, cwd match, quiet period, or timeout supplies state.

The roster exposes no safe monotonic completion counter/epoch. Both optional
counter fields are `None`: a turn completed entirely between daemon samples
can be missed, so `send --wait` and first-turn `spawn --wait` can time out.
That gap is not a successful wait test. Linux PID-start token semantics have
not been verified; a live opted-in Linux session fails explicitly. Linux
users without the opt-in retain existing behavior.

## Isolated live check

After the orchestrator registers the adapter, build the branch and run:

```sh
cargo build
python3 extensions/grok/check-live.py "$PWD/target/debug/tmux-agent-plane"
```

The script uses a unique detached tmux server, a unique XDG state directory,
and an isolated Grok home with a symlink to existing authenticated credentials.
It disables folder-trust prompting only in its own empty scratch directory.
All agent jobs go through the supplied plane CLI. It does not replace the
installed plane binary, restart the user daemon, attach a user-facing client,
or consume Codex turns. It retains logs and prints their directory on exit.
The done-to-viewed-idle check uses a client inside a private PTY and focuses
the agent through plane; it never opens a terminal window on the desktop.
