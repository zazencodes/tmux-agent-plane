# Antigravity CLI (`agy`) findings

Verified on macOS arm64, 2026-11-02, using installed Antigravity CLI **1.4.12** (`/Users/alex/.local/bin/agy`) and tmux **3.7c**. This branch starts at Phase 0 `a390d14`.

## Implemented native method

No agent hooks, plugin, wrapper, or configuration is required for tracking. The single detection path is:

1. Find native `agy` processes in each pane's process subtree.
2. Read their actual OS argument vectors. macOS uses `sysctl(KERN_PROCARGS2, pid)`; Linux uses `/proc/<pid>/cmdline`. This preserves argument boundaries when an interactive prompt contains `--print`. Exclude print/prompt mode, help/version invocations, and management subcommands before running `lsof`.
3. Read one `lsof -nP -Fpfan -a -p <pids> +d <presence-directory>` snapshot. Associate each process's read/write descriptor for `~/.gemini/antigravity-cli/presence/<conversation-uuid>.lock` with that exact conversation. Ignore read-only directory watchers. Require one selected conversation per process and one interactive session per pane; ambiguous selections are errors.
4. Read that conversation's native summary and SQLite step records with **bundled SQLite**, opened `SQLITE_OPEN_READ_ONLY`. The summary supplies the title, `status`, and `not_fully_idle`. The conversation database supplies current-turn pending interactions.

No cwd, latest-history file, mtime, terminal screen text, inactivity timeout, or missing spinner determines identity or state. Exited processes cannot be selected from old presence files because discovery starts from the live pane subtree and its open descriptors. The files themselves persist after exit.

### Requirements and installation text for the root README

The standard binary and tmux-plugin installation remains sufficient. There is **no agy opt-in symlink or configuration step**.

For agy tracking, `lsof` must be on `PATH`. macOS supplies `/usr/sbin/lsof`; Linux users need the `lsof` package. The adapter does not invoke `lsof` when there are no live interactive agy candidates, or when native presence/summary data is absent. Users running only Claude/Codex need no new executable. SQLite is bundled in the Rust binary; no system `sqlite3` command is needed.

A hand-started agy appears **after its first message**, when it publishes conversation state. A blank initial TUI has no summary and remains untracked. `read` and `keys` can address its pane id before then.

Task-bearing launch is `agy --prompt-interactive <task>`, which leaves the TUI available. `--print`, `-p`, and `--prompt` are headless modes and are excluded. The current owned module keeps the Phase 0 `launch(Option<&str>) -> Launch` contract. The lead accepted a shared integration change: reject taskless `spawn agy` before pane creation with `agy publishes no state until its first turn; give spawn a task`. The orchestrator owns the fallible-launch refactor and guard; this worker did not alter shared orchestration.

## State and freshness

Native `conversation_summaries` columns include `conversation_id`, `title`, `status`, `not_fully_idle`, and `killed`. Each `conversations/<id>.db` has `steps(idx, step_type, status, ..., step_payload, ...)`.

The adapter applies this precedence:

1. A current-turn `CORTEX_STEP_STATUS_WAITING` step with a native `Step.requested_interaction` is **blocked**. Proved permission and question variants are supported; other variants error.
2. Otherwise, `CASCADE_RUN_STATUS_RUNNING`, `BUSY`, or `CANCELING` is **working**. `IDLE` with `not_fully_idle = 1` is also **working**.
3. Only `IDLE` with `not_fully_idle = 0`, and no pending current-turn interaction, is **idle**.

`not_fully_idle` matters: the installed native hook reference defines `Stop.fullyIdle` in terms of all background tasks finishing. The live background probe confirmed the inverse summary flag's meaning. After the tool returned a background handle and the model replied `STARTED`, the summary became `IDLE/1` while the command was still running. On its completion the agent ran a follow-up turn (`RUNNING/1`), then settled to `IDLE/0`. Treating summary `IDLE` alone as idle would finish this turn early.

Pending interactions must override even `IDLE/0`: a real permission modal initially published `RUNNING/1`, then its summary settled to `IDLE/0` while the current step still had status `WAITING` and its permission request. The modal remained open. The adapter continues reporting blocked from that explicit request. Answering it changes the step; old requests are not inferred from tool names or the nullable `permissions` column.

Only steps at or after the latest `CORTEX_STEP_TYPE_USER_INPUT` (`step_type = 14`) are considered. Thus an earlier turn's history cannot keep a new turn blocked. Abruptly killing a blocked process removes its live association. Resuming that exact UUID in another pane was also tested: agy rewrote the abandoned step as interrupted, showed its ready prompt, and the adapter reported idle, without replaying the old request.

Both databases use WAL. Bundled rusqlite 0.40.2 uses SQLite 3.53.2 here and reads live WAL updates and checkpointed databases correctly. Apple's SQLite 3.43.2 intermittently failed read-only queries after checkpointing; `immutable=1` bypassed WAL and returned stale idle data. Neither approach is used by the adapter.

`done` is derived entirely by the daemon. The adapter publishes `completed_turns: None` and `completion_epoch: None`; no native durable completion counter was proved. A turn that starts and finishes between one-second daemon scans can therefore evade `send --wait` / `spawn --wait`. The live fast-turn checks succeeded, but do not establish a guarantee for sub-tick turns.

### Native wire fields and enums

The installed binary embeds its native protobuf descriptors. `CortexStepStatus` at binary offset 65197323 lists:

- 0 unspecified (error), 8 generating, 11 queued, 1 pending, 2 running, 9 waiting;
- 3 done, 4 invalid, 5 cleared, 6 canceled, 7 error, 12 interrupted.

`CascadeRunStatus` at offset 65197767 lists 0 unspecified (error), 1 idle, 2 running, 3 canceling, 4 busy. RUNNING/IDLE transitions were verified live; BUSY/CANCELING are recognized native active enum values and were not forced in the live run.

`step_payload` is a native protobuf `Step`: field **56** is `requested_interaction`; its oneof fields **21** and **22** are permission and ask-question requests respectively. The adapter skips unrelated protobuf fields, requires a well-formed request for status 9, and errors on unsupported wire data or interaction variants. Unknown summary or step statuses and non-boolean `not_fully_idle` values are errors.

## Live evidence and checks

All agent launches, reads, keys, and shutdowns used a locally built plane CLI. Temporary kind registration and research CLI additions were restored before committing. Raw tmux only set up isolated infrastructure. The installed binary, user daemon, other worktrees, and shared orchestration were left untouched.

Representative identity proof:

- Pane `%1`, shell PID **70426**, native agy PID **70892**, open presence UUID **26ea77b0-f31b-447b-ae72-d9495d596eea**.
- Independent permission probe: pane `%3`, native PID **49755**, UUID **d0d83804-ad42-4bb7-840b-60618acdfdb1**.
- Two panes in the same cwd were distinguished by different native PIDs/open conversation UUIDs. No cwd association was used.
- Headless `agy --print` was found to open presence files too. This negative research result motivated the exact argv exclusion, with token-boundary regression tests.
- Default terminal title remained `Mac` during working and question waits. OSC 7501 is a documented native signal, but tmux 3.7c exposes no documented per-pane format for it. Terminal titles/OSC are not the shipped method.

The comprehensive isolated run `target/agy-live-fbc5fedba8fc/results.json` passed:

- task-bearing `spawn --wait` returning blocked on a real question;
- answering the question, unseen done, then viewed idle through a hidden PTY client and plane `focus`;
- two panes with the same cwd and distinct session UUIDs;
- a real permission prompt, including a sustained pending request, and `send --wait` returning blocked;
- permission answer followed by working, unseen done, and viewed idle;
- actual short first-turn `spawn --wait` and short `send --wait` completions;
- plane `kill` disappearance and native `/exit` disappearance.

Its daemon log is empty. All its agents and isolated infrastructure were cleaned up. Notification executables were stubbed only in the harness environment, so no desktop UI was opened. A temporary **workspace test hook** forced `run_command` permission requests because the user's native settings allow every tool. It was removed after testing and is not part of tracking or installation. An initial harness attempt used too broad a hook matcher and denied the question tool; the retained harness uses `run_command` only, and the subsequent run above passed.

The targeted background probe retained native samples in `target/agy-empty-check/background-samples.json`: RUNNING/1 at the start, IDLE/1 while the background command ran, RUNNING/1 on its completion notification, then IDLE/0. That agent was killed through plane and its isolated server was removed.

### Reproduce against the merged binary

Run from an authenticated, already-trusted test checkout without an existing `.agents/hooks.json`:

```sh
cargo build
python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane
```

The retained harness creates a unique tmux server and `XDG_STATE_HOME` below `target/`, runs background agent windows, attaches only a hidden PTY client, temporarily installs its test hook, and cleans up through plane before removing the server. It writes a result JSON and daemon log. Fast-wait timeout-only cases are recorded as **GAP** and the remaining checks continue; other failures remain fatal. If an unindexed agent prevents safe cleanup, it retains the isolated server and its environment for plane-based inspection.

## Fixtures, validation, and remaining limits

`tests/fixtures/agy/` retains sanitized lsof identity records, question/permission interaction wire data, and native background-state samples. The question wire fixture is reconstructed from the observed native decoded request; the permission fixture retains only its real requested-interaction field. Model signatures, account data, and unrelated transcript payloads were removed.

Unit tests cover exact argv mode exclusions, native OS argv reading, absent-agent behavior, PID-scoped presence parsing, explicit permission/question blocking, unknown/malformed native data, current-turn isolation from history, pending interaction overriding IDLE, background work under an IDLE summary, live WAL changes, checkpoint reopening, and interactive launch arguments.

Final validation passed with temporary registration and temporary shared counter/epoch fields to match the orchestrator contract: `cargo fmt`, `cargo test` (19 passing tests), and `cargo clippy --all-targets -- -D warnings`. Those shared edits are restored afterward. The orchestrator must add `pub mod agy`, `Kind::Agy`, `as_str() = "agy"`, and adapter pointers to `agy::scan` / `agy::launch`, then apply its agreed fallible-launch guard.

Limits remain explicit:

- Native first-turn publication scope and sampled fast-wait limitation described above.
- Linux argv and Linux lsof code is provided but has no live Linux validation on this macOS host.
- Multiple selected presence conversations in one process, or multiple interactive agy processes in one pane, error. Nested/subagent conversation layouts have not been validated.
- MCP elicitation and other interaction variants beyond permission/question fail loudly until proved.
- Native schema/protobuf details are internal and version-sensitive. Version 1.4.12 was checked; unknown published data is an error, not assumed idle.

## Primary sources

- [Official Antigravity CLI repository](https://github.com/google-antigravity/antigravity-cli): product identity and interactive CLI usage; the implementation is not published there.
- [Official CLI changelog](https://github.com/google-antigravity/antigravity-cli/blob/main/CHANGELOG.md): native OSC 7501 reporting, print mode, and hook behavior.
- [Official hook reference](https://antigravity.google/docs/hooks/): supported events and native `fullyIdle` contract.
- [Official plugin authoring reference](https://antigravity.google/docs/build-plugins/): workspace hook shape used solely for the permission test.
- Installed primary material: `agy --version`, `agy --help`, embedded native descriptors, native live databases/open descriptors, and `~/.gemini/antigravity-cli/builtin/skills/agy-customizations/docs/hooks.md`.
