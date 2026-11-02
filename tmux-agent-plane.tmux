#!/usr/bin/env bash
# tmux plugin entry point. TPM runs this file; without TPM, add to tmux.conf:
#   run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux
set -eu

if ! command -v tmux-agent-plane >/dev/null; then
  tmux display-message "tmux-agent-plane: the tmux-agent-plane binary is not on tmux's PATH"
  # The explicit message reports the installation error; a failed plugin job
  # would also open tmux view mode over the current pane.
  exit 0
fi

# prefix + S (or @agent-plane-sidebar-key): open the agent sidebar on the left of every window
# and move into it; close it everywhere when you are in it.
sidebar_key="$(tmux show-option -gqv @agent-plane-sidebar-key)"
tmux bind-key "${sidebar_key:-S}" run-shell "tmux-agent-plane toggle-sidebar '#{window_id}'"
# While the sidebar is open, a window gets one when it comes on screen (new windows, sessions).
# A fixed hook index, so reloading tmux.conf replaces these hooks and keeps any of your own.
follow="run-shell -b \"tmux-agent-plane follow-sidebar '#{window_id}'\""
tmux set-hook -g 'session-window-changed[90]' "$follow"
tmux set-hook -g 'client-session-changed[90]' "$follow"
# prefix + A: the agent list with a large preview, in a popup.
tmux bind-key A display-popup -E -w 90% -h 85% -T " agents " "tmux-agent-plane ui"

# Initial tmux.conf runs before the first session exists. Start from the session
# hook once there are panes to scan; reloading the plugin also starts an existing
# server's watcher. Start is idempotent and lifecycle failures go to daemon.log.
start='tmux-agent-plane daemon start >/dev/null 2>&1 || tmux display-message "tmux-agent-plane: daemon start failed; run tmux-agent-plane daemon start for diagnostics"'
tmux set-hook -g 'session-created[90]' "run-shell -b '$start'"
if tmux has-session 2>/dev/null; then
  sh -c "$start"
fi
