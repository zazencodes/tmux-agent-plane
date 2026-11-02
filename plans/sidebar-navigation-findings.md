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
