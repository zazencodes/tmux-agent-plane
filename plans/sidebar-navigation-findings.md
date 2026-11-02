# Sidebar navigation

Worktree: `/Users/alex/pro/tmux-agent-plane-worktrees/fix-sidebar-navigation`.
Branch: `fix-sidebar-navigation`, based on integrated footer commit `b52b749`.
Assigned worker: `sidebar-nav`; reports to `agents-lead`. This work stays separate
from the fixes release and requires lead review/user trial before integration.

## Controls and design

- `V` is off initially. Arrows and `j`/`k` select without switching context.
  Turning `V` on previews the selected agent's window/session with its sidebar
  active. Movement continues from the same selected pane across sidebar processes.
  `V` again stops previewing; `Enter` deliberately focuses the selected agent.
- `F` cycles All → Blocked → Done → Working → Idle → All. Only pressing `F`
  skips categories with no matches under the current query. Refresh never changes
  the chosen filter. A newly empty filter shows an explicit empty result.
- `\` starts incremental, case-insensitive search over name, task, blocked detail,
  location (including session) and working directory. Search composes with state.
  Editing stays in the current sidebar, **including arrow movement**. The mode
  row says `paused`; `Enter` accepts the query and previews its final selection
  when `V` is on. Another Enter deliberately focuses the agent. Escape cancels
  editing and restores the previous query/selection; Escape outside editing
  clears the query. Ctrl-W deletes a word and Ctrl-U clears. V/F/r/q are text
  during search. The canonical commands are uppercase `V` and `F`.
- Pane identity preserves selection when the index is reordered/refreshed.
  When selection disappears, blocked then done take priority, then the first
  match. Rename is pinned to its starting pane and cancels if that pane leaves
  the list, rather than applying the edit to its replacement.
- Existing panel, palette, session grouping, two-line agent rows and bottom
  footer remain. Two compact rows expose view/filter/query. Errors and edit
  controls outrank a live footer event; its absolute five-second expiry remains.

## Client routing and state

A preview transfers navigation to the destination sidebar through a one-shot pane
option. Both client tty and client PID must match, avoiding adoption by a reused
tty. The destination adopts and draws it before processing buffered input.
Inactive sidebars check at the existing 500 ms refresh, plus startup and input,
rather than launching tmux processes every 100 ms. A failed switch clears its
pending handoff before reporting the error.

`switch-client -c` explicitly targets the originating client. Since tmux sessions
share their current window, and linked windows share their active pane, explicit
`-c` alone cannot isolate all changes. Ambiguous input clients and destinations
shared with another attached client are refused visibly before switching them.

Buffered key bursts are applied before switching. Keys that arrive in the old
PTY during a switch can be relayed only to the same live client's current,
uniquely occupied, marked sidebar. The relay never targets an agent; after
intentional agent focus, further stranded keys are discarded with a local notice.
There is no server-wide total-order protocol across PTYs: command-mode mixtures
that overlap a preview switch remain non-atomic. This branch is for review and
user trial, and does not promise arbitrary cross-PTY key ordering.

The existing daemon marks an agent seen from active-pane visibility, not from
whether someone has read its screen. Preview exposes the agent's window but
leaves the sidebar active, so Done remains unseen. Enter focuses it and allows
the daemon to mark it idle. Both ordinary clients and a client with tmux's
`active-pane` flag were exercised.

## Input-order finding and deliberate adjustment

An earlier design also previewed while typing search. A strict 35 ms stream of
text, arrows and Backspace expected `nav-done` but produced `nav-neoe`:
`/tmp/tap-nav-qu93n3_z/mixed-order.json` and its decoded PTY snapshot. Later direct
input in the destination overtook buffered source input. A count-only j test
could not establish order.

The final UX pauses preview for the entire search edit, including arrows, and
previews on Enter acceptance. This keeps incremental filtering and editing in
one PTY without introducing a distributed ordering protocol. The strict mixed
stream remains an exact-content assertion, with an additional assertion that
no switch-client command occurs during editing.

## Validation

- `cargo fmt --check`, `cargo test` (74 tests),
  `cargo clippy --all-targets -- -D warnings`, and `git diff --check`.
- `uv run tests/sidebar_navigation_live.py target/debug/tmux-agent-plane`:
  four compiled native Pi publisher fixtures; no real agent/model calls;
  isolated tmux socket, short `/tmp` state, private HOME/config, and native PTY
  clients. Checks same/cross-window/session preview, mode-off selection, buffered
  jjj and timed seven-j movement, strict mixed search text/arrows/Backspace order,
  accepted-query handoff, Done/Enter semantics, query/filter composition,
  stable empty filters, footer/edit/error priority and expiry, independent and
  shared clients, actual injected switch failure cleanup, rename target exit,
  and a narrow empty list. Final passing evidence: `/tmp/tap-nav-dute_vir`,
  including `results.json`, `mixed-order.json`, decoded PTY snapshots, raw PTY
  streams, native publisher/index files and `tmux.calls`. Native fixture input logs and tmux command logs prove
  relays address marked sidebars and never send navigation bytes to agents.
- Existing footer native regression passed at `/tmp/tap-footer-ezlde5z2`:
  done/blocked colour and expiry across clients, desktop delivery retry,
  diagnostic recovery, restart clears events, closed sidebars stay closed,
  unchanged pane input/focus/layout. These are private fixture servers only.
- Existing blocked-send native regression passed at `/tmp/bs-qvmk64nw`:
  blocked refusal before buffers/keys, deliberate multiline answers and wait
  lifecycle, normal nonblocked sends, blocked follow-up questions, timeouts and
  stale index. The guarded send code was untouched.
- Impeccable context loaded once for src/sidebar.rs. One batched inspection of
  decoded PTY states preserved the incumbent visual world; detector returned
  `[]`. Final confirmation includes paused search and narrow/empty states.

No main checkout edits, installation, user daemon restart, merge, push, tag or
publish. Scratch fixture agents/servers are cleaned up; the assigned worker pane
is retained. Mouse-wheel behavior is unchanged; documented movement is arrows
and j/k. Linux CI's separate process-table fix belongs to the lead's integration.

## WIP: confirmed sidebar kill (separate user request)

The navigation implementation is committed and live-proven at `7429097`. This
follow-up implements `x` separately; sorting is owned by root on `sidebar-sort`
and is **not included or merged here**.

Implemented:

- `x` pins pane, kind, native session/publisher identity (and the unique native
  Codex PID), name, task/detail, state/turn and originating client tty/PID.
- Footer confirmation is above notices/events, with two name/state lines and
  an extra explicit `ends session <name>? Y/n` line for remote final sessions.
- Enter or unmodified lowercase `y` confirms. Every other key cancels without
  replay, including movement, x, uppercase Y, modifiers and q.
- At confirmation, reread index, native adapter signal, client context and pane/
  window/session topology. Target/state/identity/context changes cancel; no
  retargeting. Never offer for the sidebar's own pane.
- A window containing only the target and its sidebar is treated as ending when
  the sidebar exits alone. Check every linked session: refuse to end the
  originating client's attached session; name remote final-session endings.
- The CLI and sidebar share `commands::kill_pane`, which issues only kill-pane,
  never kill-session. After success, select the next visible agent (previous at
  end) and hide closed panes until the daemon index catches up.
- README key table, normal footer hints and Unreleased entry updated.

Checks completed after the x changes: `cargo fmt`, `cargo test` (78 passing),
`cargo clippy --all-targets -- -D warnings`. New unit/rendering tests cover
Enter/y parsing, all-other-key cancellation/no replay, native identity/state/
turn/name/location guards, own-sidebar exclusion, window/sidebar-alone topology,
attached vs remote/linked session policy, and legible 20/38-column confirmation
with priority over local notices. No failing unit checks are known.

**No destructive x live test was run.** The user's wrap-up explicitly prohibited
closing panes/agents or starting destructive tests. This is a WIP commit, not a
claim that x is ready for integration or installation. Existing navigation
artifacts above predate x and do not validate kill behavior. No new real model
agents, no panes/agents closed during this wrap-up, no merge/main edits/install/
daemon restart/push/release. The assigned worker pane is retained.

TODO:

- Run a private native-publisher + PTY x test after the no-close constraint is
  lifted: Enter/y confirmation, `x Enter x Enter`, every-other-key cancellation
  without replay, state/identity change before y, target exit/rename, immediate
  next-visible selection, own-sidebar exclusion and footer event/error priority.
- Verify ordinary pane kill, final-window behavior, linked windows, refusal to
  end the originating client's attached session, and the explicit remote ends-
  session prompt. Assert command logs contain no kill-session and that no
  unrelated pane/session/client input changes. Check narrow confirmation live.
- Review the fresh-check/kill-pane boundary: checks are fresh snapshots, not an
  atomic server-side topology/identity transaction. Keep this limitation visible
  when deciding whether the attached-session protection needs a stronger guard.
- Review/merge root's separate sorting work only after its isolated live proof;
  resolve shared sidebar fields/keys/footer routinely, rerun combined checks and
  native regressions before any main integration/install/user trial.
- Existing navigation follow-up: command-mode input crossing sidebar PTYs is
  non-atomic; search editing deliberately pauses preview to keep typed input local.

There are no remaining user decisions about x: default yes is Enter/lowercase y;
remote final-session endings are allowed with a named warning, while ending the
originating attached session is refused. Root owns main `.agents/TODO.md` and
receives these follow-ups; this worker does not edit it.
