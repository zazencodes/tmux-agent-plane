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
# Direct navigation keeps keyboard input in the agent; no sidebar focus is needed.
# run-shell is synchronous so repeated keys resolve the live client in order.
previous_key="$(tmux show-option -gqv @agent-plane-previous-key)"
next_key="$(tmux show-option -gqv @agent-plane-next-key)"
previous_key="${previous_key:-M-k}"
next_key="${next_key:-M-j}"
if [ "$previous_key" = "$next_key" ]; then
  tmux display-message "tmux-agent-plane: previous and next keys must differ"
  exit 1
fi
# Reloading after a key change removes only bindings this plugin still owns.
bindings="$(tmux list-keys -T root)"
for direction in previous next; do
  old_key="$(tmux show-option -gqv "@agent-plane-bound-$direction-key")"
  if [ -n "$old_key" ]; then
    while IFS= read -r binding; do
      case "$binding" in
        *" -T root $old_key "*"tmux-agent-plane cycle $direction --client "*)
          tmux unbind-key -n "$old_key" ;;
      esac
    done <<< "$bindings"
  fi
done
tmux bind-key -n "$previous_key" run-shell "tmux-agent-plane cycle previous --client #{q:client_name}"
tmux bind-key -n "$next_key" run-shell "tmux-agent-plane cycle next --client #{q:client_name}"
tmux set-option -g @agent-plane-bound-previous-key "$previous_key"
tmux set-option -g @agent-plane-bound-next-key "$next_key"
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
# prefix + a: searchable agents with a large preview. prefix + f: the same, already searching.
# Full screen: the picker draws its panel over a dimmed copy of the screen.
# Inherit the terminal's colours even when a tmux theme sets a separate popup background.
# Expand the opening client here: display-popup does not expand its shell command.
picker='display-popup -E -B -w 100% -h 100% -s "bg=terminal,fg=terminal" "tmux-agent-plane ui --client #{q:client_name}'
tmux bind-key a run-shell -C "$picker\""
tmux bind-key f run-shell -C "$picker --search\""

# Initial tmux.conf runs before the first session exists. Start from the session
# hook once there are panes to scan; reloading the plugin also starts an existing
# server's watcher. Start is idempotent and lifecycle failures go to daemon.log.
start='tmux-agent-plane daemon start >/dev/null 2>&1 || tmux display-message "tmux-agent-plane: daemon start failed; run tmux-agent-plane daemon start for diagnostics"'
tmux set-hook -g 'session-created[90]' "run-shell -b '$start'"
if tmux has-session 2>/dev/null; then
  sh -c "$start"
fi
