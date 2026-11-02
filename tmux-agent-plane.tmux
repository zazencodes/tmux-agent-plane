#!/usr/bin/env bash
# tmux plugin entry point. TPM runs this file; without TPM, add to tmux.conf:
#   run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux
set -eu

if ! command -v tmux-agent-plane >/dev/null; then
  tmux display-message "tmux-agent-plane: the tmux-agent-plane binary is not on tmux's PATH"
  exit 1
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

# The daemon exits when the tmux server does; a second copy exits at once.
tmux run-shell -b "tmux-agent-plane daemon"
