# Revive closed conversations through their native identities

## Decision

The daemon retains a bounded recent history of conversations it has observed, including those closed outside Agent Plane. The sidebar and picker share a workspace-scoped browser; the CLI can inspect all workspaces. Reviving is explicit and resumes the recorded conversation in a new window of its original project and workspace. It never inherits leadership or a family link from an ended process.

Each integration resumes by its published conversation identity. Codex publishes that identity through its configurable native terminal title; shortened IDs must resolve uniquely against its local thread database. A missing or ambiguous identity never chooses another conversation. This does not identify the pane of a Codex calling tool shell.

## Why

Closing a terminal should leave the conversation recoverable without launching an unrelated fresh chat. Native identities preserve each agent's own saved history, while a daemon-owned history also covers terminals closed by hand and survives restarts.

## Rejected

- **Remember only Agent Plane's kill actions.** Misses closes outside the plane.
- **Resume the most recent conversation in a folder.** Several agents can share that folder and would reopen the wrong conversation.
- **Use Codex hook environment for pane identity.** A shared server can supply another terminal's environment.
