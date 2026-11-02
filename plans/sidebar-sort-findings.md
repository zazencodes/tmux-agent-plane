# Sidebar attention sorting — WIP

Branch `sidebar-sort`, based on navigation commit `7429097`. This is separate from
`fix-sidebar-navigation`, where `sidebar-nav` owns the `x` confirmation. The user
requested a timed wrap-up: no main merge, install, pane closure or agent kills.

Implemented option A: session headings stay; per-session blocked/done/working/idle,
then oldest state_since first. The order snapshot lives in Navigation and travels
with V handoffs. While focused, live badges/ages update without reordering; new
agents append in-group and removed agents disappear. F, search acceptance and the
visible uppercase S re-sort rebuild the order, preserving selected pane identity.
Unfocused sidebars refresh ranking; a new keyboard visit uses current priorities.

Validation: cargo fmt --check, cargo test (76), strict all-target clippy and diff
checks passed. Unit coverage includes state changes in fixed rows, oldest blocked
first, append/remove, explicit S and focus re-entry, search acceptance, preserved
selection, groups and serialized handoff order. No live sorting proof is claimed.

Remaining before merge/user trial:

- Add isolated PTY sorting tests for focus latching, state/age updates, arrivals,
  removal, S/F/search reorder and V snapshot handoff.
- Migrate existing navigation live expectations: the daemon index remains in tmux
  location order; the sidebar now ranks within session, so tests cannot derive
  expected cursor movement directly from index.agents order.
- Combine with x's separate commit, resolve shared footer/key fields and test both.
- Rebase/merge onto current main (which includes Linux process fix and v0.2.0),
  reconcile Unreleased notes without moving shipped release entries, and run
  merged fmt/test/clippy plus native regressions before installing/restarting.
- Existing command-mode cross-PTY mixtures still have no total-order guarantee;
  search editing pauses preview to keep text input local. See navigation findings.

No sorting decisions remain pending: user chose option A and a visible re-sort key;
this implementation uses S. Live proof and integration are the outstanding work.
