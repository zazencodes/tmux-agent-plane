# agy native fixtures

Captured on macOS with Antigravity CLI 1.4.12, 2026-11-02.

- `presence.lsof`: selected fields from live `lsof -Fpfan` output. The read-only directory record must be ignored; each read/write lock identifies its PID's conversation.
- `waiting.json`: sanitized native `Step.requested_interaction` wire fields for an actual question and permission request. The question is reconstructed from the observed decoded native message. The permission preserves the native field bytes. Other step metadata, including model signatures, is omitted.
- `background.json`: actual bundled-SQLite samples during an asynchronous `sleep 30` and its completion notification. `IDLE` with `not_fully_idle=true` still means work remains.
- `trust-unaccepted.json` / `trust-accepted.json`: sanitized native `settings.json` before and after accepting a fresh scratch folder. The unaccepted file omits the empty native list; acceptance publishes the exact cwd. Native 1.4.12 still prompts in a child of a trusted folder. Default path/key source: https://antigravity.google/docs/cli/reference.

Reproduce folder trust and official session overrides without changing user trust:

```sh
python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane --folder-trust --session-trust-overrides
```

Both harness modes use a private HOME for the agent and daemon and assert the real settings SHA-256 is unchanged, even after failure. The trust mode proves acceptance from native saved trust and presence/DB artifacts. Terminal snapshots are human-review evidence only.
