# 1. Lowercase keys

Status: accepted

## Context

The sidebar mixed lowercase action keys (`q`, `r`, `j`, `k`, `x`) with uppercase ones (`V`, `F`, `S`). Uppercase needs Shift, and it split the keys into two groups with no real meaning.

## Decision

Every tmux-agent-plane key binding in the sidebar and popup is a lowercase letter or a symbol such as `/`. We do not bind uppercase letters or Shift chords. A new key takes the next free lowercase letter.

The sidebar keys are `v` (view mode), `f` (state filter), `/` (search), `r` (rename), `x` (kill), `c` (new agent), `o` (configure), `j`/`k` (move) and `q` (quit).

## Consequences

- Docs, footer hints and tests use lowercase keys.
- The `prefix + S` and `prefix + A` tmux bindings belong to tmux's own key table, are configurable, and are out of scope.
