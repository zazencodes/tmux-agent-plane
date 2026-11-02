# Blocked send safety proof

`send` previously read the target from the index and immediately loaded/pasted a
buffer and pressed Enter, even for `State::Blocked`. This let coordination status
messages become answers to another agent's question. The task supplied the skill
RED: this happened twice to plane-lead.

The command now checks the indexed state before any terminal or buffer operation.
An ordinary blocked send names the agent, explains the question/permission wait,
and points to `read`, `keys`, and deliberate `send --answer`. `--answer` is strict:
it requires a blocked target, so a reply submitted after the indexed question ends
fails without starting a turn. There is no automatic retry. Working, idle and done
agents retain ordinary sends, including multiline bracketed paste. Reply waits
ignore the original blocked state's timestamp and follow completion or a later
blocked state, preserving the existing wait timeout accounting.

The skill and README send examples distinguish deliberate authorized answers from
status/escalation reports. Reports require a current state check, never use
`--answer`, and are deferred or routed through an available lead. Menus and
permissions use `keys` only when the user's instructions make the answer clear.

## Reproduce

```sh
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build
python3 tests/blocked_send_live.py target/debug/tmux-agent-plane
```

The Python harness builds a C raw-terminal fixture that publishes native Claude
session files. It starts a private detached tmux server and managed daemon under a
private HOME and short `/tmp` XDG state directory, stubs notifications, and logs
every tmux invocation. Agent spawn/kill/send jobs use the branch CLI with explicit
names and window placement; raw tmux only manages the base shell and server.
Terminal bytes come from the fixture's input file, and state comes from native
session files and the daemon index. No screen scraping, model turns, actual agy,
installed binary changes, user daemon changes or user configuration changes.

## Results

- Before the fix, `/tmp/bs-717ak8cj` reproduced RED: an indexed blocked status send
  exited successfully and sent input.
- After the fix, `/tmp/bs-6ulx04lz` passed the initial suite. The expanded suite
  passed at `/tmp/bs-_1viajjx`, including normal and reply waits that end at a
  subsequent question.
- Blocked plain status: nonzero exit, useful error, zero received bytes and zero
  load/paste/delete-buffer or send-keys calls.
- Deliberate multiline answer: exact one bracketed paste plus one Enter; quotes
  and line boundaries preserved (tmux normalizes LF to CR on the terminal wire).
- Working/idle/done: normal sends work; `--answer` refuses without mutation.
- Reply wait: remains running while the original question remains blocked, then
  returns on completion or on the next question after observed working state.
- Normal send wait: returns at the new turn's question.
- Reply timeout: the existing blocked state does not cause early success; the
  command reports the requested one-second timeout.
- Stale index: ordinary and answer sends still fail before terminal mutations.
- All 61 Rust tests and clippy with warnings denied pass; formatting is clean.

Root relayed notify-research's read-only skill GREEN: all four scenarios passed.
An urgent status to blocked plane-lead was routed to working agents-lead or
deferred, with neither `--answer` nor keys. Authorized text used read/verify then
`send --answer`; an authorized menu used observed keys. A stale `--answer` for an
idle target stopped/deferred without retrying a normal send. No scenario sends or
other mutations were executed.

## Limits

This guard uses the latest daemon index, as requested; state may change between a
one-second observation and delivery. It does not identify a particular question
or make an answer and a native state transition atomic. Existing sampled-agent
fast-turn limitations still apply to `--wait`. The proof uses a native-format
fixture, not new model turns across all agent CLIs. No global install was done;
coordination can use this worktree's built binary until root integrates it.
