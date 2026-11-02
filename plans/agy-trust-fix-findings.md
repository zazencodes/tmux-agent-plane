# agy folder trust findings

Tested Antigravity CLI 1.4.12 on macOS, 2026-11-02. Installed binary and user daemon were not replaced or restarted.

## Cause and native signal

A new, never-trusted folder shows its trust dialog while `--prompt-interactive` already runs the initial model turn. Native RW presence, a generated title, `CASCADE_RUN_STATUS_IDLE`, and completed USER_INPUT/MODEL_RESPONSE steps all exist behind that dialog. The previous adapter interpreted these as idle/done. No scan error or daemon exit occurred in the isolated repro.

Native `~/.gemini/antigravity-cli/settings.json` publishes `trustedWorkspaces`. Accepting the scratch folder through `plane keys <pane> Enter` immediately adds its exact path. A child of that trusted folder still prompts; parent-prefix trust is invalid in 1.4.12. `workspace_uris` remains empty after acceptance and cannot establish trust.

Source for the default path/key: [official CLI settings reference](https://antigravity.google/docs/cli/reference), Configuration keys (`settings.json`), `trustedWorkspaces`. The installed CLI's `--help` documents `--settings` as a session-only inline JSON or file overlay. [Official projects documentation](https://antigravity.google/docs/projects/) confirms selected projects can have multiple folders and resume can select a conversation's project.

## Change

Require exact native cwd trust before interpreting conversation state. Missing settings/key stays untracked; malformed settings fails loudly. Re-read on every scan so default native acceptance starts tracking immediately. The existing native pending request/current-turn, summary, WAL and background-work logic stays in place after trust.

Read settings/project options from actual OS argv, preserving prompt argument boundaries and `--`. Inline/file `--settings` with `trustedWorkspaces` replaces saved trust for the check; a saved global grant cannot bypass a session override that forces the dialog. Overrides explicitly granting the cwd can be tracked. Other settings overlays preserve saved trust.

## Advanced-mode limits

These are explicit tracking reductions from the previous adapter:

- Accepted by lead: `--project`, `--new-project`, and native conversation `project_id` other than `default-cli-project` remain untracked. A trusted process cwd does not establish the selected project's folder trust. A verified native project-to-folder mapping has not been found; no live non-default-project test was run, and no project was created for this investigation.
- Accepted by lead: session trust overrides excluding cwd stay untracked even after acceptance. Native inline and file probes prove they can reopen trust while saved global trust already contains cwd. Acceptance changes native in-memory trust while argv stays unchanged; saved global trust and the original overlay do not identify the modal's live state. Explicit grants containing cwd retain tracking. A live per-process trust signal would be needed to remove this limit safely.

Root owns README/AGENTS/skill integration updates. Both the named-project exclusion and the precise override-after-acceptance limit were explicitly accepted by plane-lead. One README/skill line should cover them.

## Proof and retained harness

`python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane --folder-trust --session-trust-overrides`

The harness uses a unique tmux server, short `/private/tmp/tap-agy-*/s` XDG state directory and `/private/tmp/tap-agy-*/h` private HOME for BOTH agent and daemon, detached windows/private PTY client, and notification stubs. Native RW presence and conversation DBs verify agent data-root isolation. Agent jobs and authorized scratch trust acceptance use plane commands. Terminal reads are retained for human review only, never parsed for detection. A before/after SHA-256 guard covers the real settings file in `finally`, including failed runs. Copied credentials are removed during cleanup.

- `tests/fixtures/agy/trust-{unaccepted,accepted}.json`: sanitized native settings before/after scratch acceptance. Unit tests cover default path, exact folder/child distinctions, dynamic acceptance, missing/malformed records, inline/file/equal-form options, selected-project exclusion and flag-like prompt text.
- `target/agy-live-fe963d7182db/results.json`: successful hash-guarded run; default trust transition, inline/file override-forced trust, question/permission waits, working/done/idle, same-cwd identities, fast waits and native exit. Real settings SHA-256 unchanged. Screens are in the same run directory.
- Final short-root run: `target/agy-live-8b0be9b7885a/results.json` passed with no errors or cleanup gaps. Native default trust acceptance, positive/negative inline/file overrides, question and permission waits, working/done/idle, distinct same-cwd sessions, fast waits and native exit all passed. Real settings SHA-256 unchanged; copied credential removed; daemon remained running through cleanup. The potential lifecycle socket path is 60 bytes.
- `cargo fmt`, `cargo test` (51 tests), `cargo clippy --all-targets -- -D warnings`: passed on final code.

An intermediate extended run (`target/agy-live-a7e33198134a`) returned a stale/not-running index error at the second override launch, with empty daemon stderr. Cleanup did not preserve its original exit status, so no actual scan error was established; it is not counted as proof. The final harness records daemon exit status before cleanup. A later extended run (`target/agy-live-7c78037d31e8`) proved the trust cases but hit a harness bookkeeping error in the regression list; that was fixed and the final full run passed. Root integration must change the harness foreground invocation from `daemon` to `daemon serve` after merging lifecycle changes.

Linux live validation remains outstanding. Native sampled-state fast-turn limitations remain unchanged.

## Isolation mistake and cleanup

An early harness run (`target/agy-live-4a99c92a362a`) started tmux without passing `env=env`. Its daemon used private HOME but the agent used real HOME. Accepting that authorized scratch folder added exactly `/Users/alex/pro/tmux-agent-plane-worktrees/fix-agy-trust/target/agy-live-4a99c92a362a/never-trusted` to the real trust list. This was reported immediately to both leads; plane-lead removed the accidental entry. This worker did not edit/undo real settings. The failed run is not counted as proof.

All tmux infrastructure now receives the private environment in both harness modes, the agent's native private artifacts are asserted, and real settings hashes are checked even on failure. The default-mode setup-failure path was exercised and its finally hash guard passed. Later hash-guarded runs passed. Scratch agents were exited through plane commands, old shell-only scratch infrastructure stopped, and copied probe credentials removed.
