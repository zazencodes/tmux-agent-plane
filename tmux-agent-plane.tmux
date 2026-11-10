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

# Closing a session keeps its clients in tmux while another session remains,
# whether or not the sidebar is open. tmux chooses the most recently active one.
tmux set-option -g detach-on-destroy off

# Install the configured keys, preserving the tmux bindings they replace. The toggle
# restores those bindings while leaving open sidebars usable.
tmux-agent-plane bindings
# While the sidebar is open, a window gets one as soon as it exists, so its agent has its final
# width before anyone looks at it. window-linked also fires for new sessions and moved panes.
# A fixed hook index, so reloading tmux.conf replaces these hooks and keeps any of your own.
tmux set-hook -g 'window-linked[90]' "run-shell -b \"tmux-agent-plane follow-sidebar '#{hook_window}'\""
# Close a lone sidebar on the layout event, before waiting for a daemon scan.
# Pin the event's window: its removed active pane may invalidate the hook's default target.
# run-shell -C runs tmux commands directly; ## defers the condition until that target is set.
tmux set-hook -g 'window-layout-changed[90]' \
  'run-shell -C "if-shell -F -t #{hook_window} '\''##{&&:##{==:##{window_panes},1},##{==:##{@agent-plane-sidebar},1}}'\'' '\''kill-pane -t #{hook_window}'\''"'
# Copy mode would freeze a sidebar on an old frame while it keeps redrawing underneath, and a
# sidebar has no scrollback to read. Other modes, such as prefix + w, end when used, so they stay.
tmux set-hook -g 'pane-mode-changed[90]' \
  'run-shell -C "if-shell -F -t #{hook_pane} '\''##{&&:##{==:##{pane_mode},copy-mode},##{==:##{@agent-plane-sidebar},1}}'\'' '\''send-keys -t #{hook_pane} -X cancel'\''"'
# Initial tmux.conf runs before the first session exists. Start from the session
# hook once there are panes to scan; reloading the plugin also starts an existing
# server's watcher. Start is idempotent and lifecycle failures go to daemon.log.
start='tmux-agent-plane daemon start >/dev/null 2>&1 || tmux display-message "tmux-agent-plane: daemon start failed; run tmux-agent-plane daemon start for diagnostics"'
tmux set-hook -g 'session-created[90]' "run-shell -b '$start'"
if tmux has-session 2>/dev/null; then
  sh -c "$start"
fi
