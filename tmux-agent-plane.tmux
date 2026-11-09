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
# Global keys work without the prefix: previous/next agent, into and out of the sidebar. Each
# option holds one key or several, separated by spaces. They keep keyboard input in the agent and
# need no sidebar focus; run-shell is synchronous so repeated keys resolve the live client in order.
# set -f: key names such as * stay literal when the lists are split.
set -f
key_option() {
  value="$(tmux show-option -gqv "@agent-plane-$1-key")"
  printf '%s' "${value:-$2}"
}
previous_keys="$(key_option previous M-k)"
next_keys="$(key_option next M-j)"
in_keys="$(key_option sidebar-in M-h)"
out_keys="$(key_option sidebar-out M-l)"
repeated="$(printf '%s\n' $previous_keys $next_keys $in_keys $out_keys | sort | uniq -d | tr '\n' ' ')"
if [ -n "$repeated" ]; then
  tmux display-message "tmux-agent-plane: each global key needs one action; repeated: $repeated"
  exit 1
fi
# @agent-plane-keys off turns every global key off, so it reaches the pane as usual. prefix + K
# (or @agent-plane-keys-toggle-key) turns them on and off; the status bar says when they are off.
case "$(tmux show-option -gqv @agent-plane-keys)" in
  ''|on|off) ;;
  *)
    tmux display-message "tmux-agent-plane: @agent-plane-keys must be on or off"
    exit 1 ;;
esac
toggle_key="$(key_option keys-toggle K)"
# Reloading after a key change removes only bindings this plugin still owns. tmux pads its
# columns, so squeeze the spaces before matching.
bindings="$(tmux list-keys | tr -s ' ')"
owns() {
  while IFS= read -r binding; do
    case "$binding" in
      *" -T $1 $2 "*"$3"*) return 0 ;;
    esac
  done <<< "$bindings"
  return 1
}
for name in previous next sidebar-in sidebar-out; do
  for old_key in $(tmux show-option -gqv "@agent-plane-bound-$name-key"); do
    if owns root "$old_key" agent-plane; then
      tmux unbind-key -n "$old_key"
    fi
  done
done
old_toggle="$(tmux show-option -gqv @agent-plane-bound-keys-toggle-key)"
if [ -n "$old_toggle" ] && owns prefix "$old_toggle" @agent-plane-keys; then
  tmux unbind-key "$old_toggle"
fi
off='#{==:#{@agent-plane-keys},off}'
for key in $previous_keys; do
  tmux bind-key -n "$key" if-shell -F "$off" "send-keys $key" \
    "run-shell 'tmux-agent-plane cycle previous --client #{q:client_name}'"
done
for key in $next_keys; do
  tmux bind-key -n "$key" if-shell -F "$off" "send-keys $key" \
    "run-shell 'tmux-agent-plane cycle next --client #{q:client_name}'"
done
# The in key moves into the sidebar and stays there. The out key leaves it for the selected agent
# by sending it Shift-Right, which it understands directly; other panes get the key as usual.
for key in $in_keys; do
  tmux bind-key -n "$key" if-shell -F "$off" "send-keys $key" \
    "run-shell \"tmux-agent-plane enter-sidebar '#{window_id}'\""
done
for key in $out_keys; do
  tmux bind-key -n "$key" if-shell -F "#{&&:#{!=:#{@agent-plane-keys},off},#{==:#{@agent-plane-sidebar},1}}" \
    'send-keys S-Right' "send-keys $key"
done
tmux bind-key "$toggle_key" if-shell -F "$off" \
  "set-option -g @agent-plane-keys on ; display-message 'tmux-agent-plane: global keys on'" \
  "set-option -g @agent-plane-keys off ; display-message 'tmux-agent-plane: global keys off; prefix + $toggle_key turns them on'"
tmux set-option -g @agent-plane-bound-previous-key "$previous_keys"
tmux set-option -g @agent-plane-bound-next-key "$next_keys"
tmux set-option -g @agent-plane-bound-sidebar-in-key "$in_keys"
tmux set-option -g @agent-plane-bound-sidebar-out-key "$out_keys"
tmux set-option -g @agent-plane-bound-keys-toggle-key "$toggle_key"
set +f
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
# prefix + a: agents with a large preview. prefix + f: the same, already filtering.
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
