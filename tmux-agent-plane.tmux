#!/usr/bin/env bash
# tmux plugin entry point. TPM runs this file; without TPM, add to tmux.conf:
#   run-shell /path/to/tmux-agent-plane/tmux-agent-plane.tmux
set -eu

if ! command -v tmux-agent-plane >/dev/null; then
  tmux display-message "tmux-agent-plane: the tmux-agent-plane binary is not on tmux's PATH"
  exit 1
fi

# prefix + S: the agent sidebar on the left of the current window, toggled.
tmux bind-key S run-shell "tmux-agent-plane toggle-sidebar '#{window_id}'"
# prefix + A: the agent list with a large preview, in a popup.
tmux bind-key A display-popup -E -w 90% -h 85% -T " agents " "tmux-agent-plane ui"

# The daemon exits when the tmux server does; a second copy exits at once.
tmux run-shell -b "tmux-agent-plane daemon"
