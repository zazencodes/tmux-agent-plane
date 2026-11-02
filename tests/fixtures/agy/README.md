# agy native fixtures

Captured on macOS with Antigravity CLI 1.4.12, 2026-11-02. See `plans/agy-findings.md` for the live procedure and limitations.

- `presence.lsof`: selected fields from live `lsof -Fpfan` output. The read-only directory record must be ignored; each read/write lock identifies its PID's conversation.
- `waiting.json`: sanitized native `Step.requested_interaction` wire fields for an actual question and permission request. The question is reconstructed from the observed decoded native message. The permission preserves the native field bytes. Other step metadata, including model signatures, is omitted.
- `background.json`: actual bundled-SQLite samples during an asynchronous `sleep 30` and its completion notification. `IDLE` with `not_fully_idle=true` still means work remains.
