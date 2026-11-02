# Bottom notification research

2026-11-02 · brief item 4 · base `9c8085c`

**Status: IMPLEMENTED — user chose the existing sidebar footer.** The later explicit direction replaces normal done/blocked tmux messages with a shared five-second sidebar event. See [implementation and private-client proof](sidebar-events-findings.md). The options and investigation below record the earlier research; the choice is settled.

**tmux 3.7c has no clean independent bottom message area while the status remains at the top.** Its message facility uses the status area; positioned overlays take input; pane-based alternatives change the layout. This conclusion combines the native command/option surface, source routing and private-client checks below. No product UI, daemon code, installed binary, user configuration or running user server was changed. Desktop notifications remain unchanged.

## Smallest choices for the lead

| Choice | Behavior and cost |
| --- | --- |
| Keep current status-line + desktop notifications | Existing behavior; messages follow `status-position`. No layout or configuration changes. Considered during research; the user chose the sidebar footer. |
| Desktop-only, if desired | Omit the tmux message in a future approved change. Keeps the top bar and pane input intact, but removes the in-terminal message. Desktop delivery still depends on the OS. This is an option for the lead, not an implemented setting. |
| Existing sidebar footer — CHOSEN | Put normal done/blocked notices at the bottom of the existing sidebar UI, with desktop notifications unchanged. This requires the sidebar to be open and reaches only its width; it is not an independent full-screen bottom strip. Implementation belongs to `sidebar-events`; none was built in this research worktree. |
| Dedicated bottom pane | A detached split can retain the active pane, but reserves rows, resizes applications, adds a pane/process, and needs lifecycle handling across windows, zoom, resize, close and sidebar layouts. A real pane is selectable and cannot promise to never take input. No split implementation was built. |
| Bottom pane border | Smaller native alternative to an extra pane: `pane-border-status bottom` plus `pane-border-format`. Live proof preserves input and top status, but reserves a row and changes window-wide border configuration. Labels belong to panes, not an independent client notification area; no built-in notification timeout. Requires explicit acceptance of those costs. |

Changing `status-position bottom` moves the whole bar and its messages. Multiple status rows stay together at the configured edge. Temporarily relocating or disabling the bar would change the user's chosen layout. Those do not satisfy this task's constraints. [Status options][status-doc]

Both leads received the preliminary options before any feature work. Research-only delivery was not the product outcome: the subsequent user decision chose the sidebar footer, and item 4 implementation continues separately under `sidebar-events`.

## Native facilities and limits

- **`display-message`:** the command calls `status_message_set`; it has no independent vertical-position flag. `-c` chooses the client; `-t` supplies format context. `-d` controls lifetime, `-l` literal text, `-p` stdout, `-a`/`-v` format inspection, and `-I` feeds an empty pane. None creates a notification surface. [Command implementation][message-cmd]
- **`message-line`:** selects a row inside the status screen. `status_prompt_line_at` clamps it to `status_line_size - 1`; redraw positions that entire screen using `status-position`. Even `status off` with position top renders a temporary message at the top. `message-style`/`message-format` change formatting and horizontal geometry. [Message geometry][status-src], [redraw geometry][redraw-src]
- **Message input/redraw:** default messages freeze pane redraw until dismissal, but the dismissal key reaches the pane. `-C` permits redraw; it does not relocate the message. `-N` ignores/discards input until timeout; combining `-N -C` still discards keys. `-d 0` waits for a key. Therefore `-N` cannot make a notification input-transparent. [Message setup][message-setup], [input routing][input-src]
- **`display-popup`:** bottom positioning, border removal (`-B`) and automatic close (`-E`/`-EE`) affect geometry/lifetime, not modality. `-k` dismisses on a key; `-N` resets those lifetime flags. Popup key callbacks send input to the popup job or consume it. The selected pane ID can remain unchanged while its input is intercepted. With native focus reporting enabled, that pane also receives focus-out on open and focus-in on close. An existing overlay can prevent a new popup, so this is also shared UI state. [Popup flags/creation][popup-cmd], [key callback][popup-input], [native pane focus][focus-src]
- **`display-menu`:** also installs a client overlay and consumes keys to navigate, choose or dismiss it. Disabled items, mouse flags and bottom placement do not provide input passthrough. Both menu and popup use `server_client_set_overlay`; its focus update is shared. [Menu overlay][menu-src], [overlay setup][overlay-src]
- **Other native facilities:** `display-panes` draws fixed pane identifiers/dimensions, not arbitrary bottom messages; its template is a selection command. Command/confirmation prompts are interactive UI; `show-messages` exposes message history. Visual activity/bell/silence notices use status messages. Control-mode `%message` requires a client to render it; `refresh-client` redraws existing surfaces. None exposes an independent notification area. [Manual][manual], [pane indicator implementation][panes-src], [visual alerts][alerts-src]

## Reproducible private-client proof

Run `uv run tests/notifications_live.py` from this worktree. The retained harness uses pinned `pyte==0.8.2`, a unique explicit tmux socket, `-f /dev/null`, unique `XDG_STATE_HOME`, notification-command stubs and an 80×24 private PTY. It runs only a raw-input fixture, with no agents, daemon or model calls. Keys enter through the client PTY, because `send-keys` would bypass overlay input routing. Terminal decoding verifies known fixture UI; it is never used for agent state detection. Cleanup kills only the private server/client.

Successful run: `target/notify-live-4_htoari/` (raw PTY bytes, text snapshots, binary input logs and `results.json`, retained locally under ignored `target/`). Rows below are one-based:

| Check | Observed/asserted result |
| --- | --- |
| Top status + default message | Message row 1; redraw freezes; dismissal key reaches fixture. |
| `display-message -C` | Row 1; redraw continues; dismissal key reaches fixture. |
| `display-message -N -C` | Row 1; typed key discarded and message stays. |
| `message-line 4` | Row 1 with one status row; row 2 with two. |
| Bottom status | Both message and normal bar move to row 24. |
| Status off, position top | Message still row 1. |
| Bottom borderless popup | Draws near bottom; `p` reaches popup only; selected pane unchanged. Forced close of its running job returns 129. |
| Bottom menu | `m` selects menu item; pane receives no `m`; selected pane unchanged. |
| Bottom pane border | Notice row 24; top status row 1; pane height 23 → 22; key reaches pane. |
| Popup with focus reporting | Pane receives `ESC [ O` on open, `ESC [ I` on close; selected pane unchanged. |

Validated installed `/opt/homebrew/bin/tmux`, version `tmux 3.7c`, SHA-256 `6a9a68532c31a72ac822481da8879ae413f158d7a2b07c50d96126012336afe0`. Primary upstream tag `3.7c` resolves to commit `e476c1230b958df0cb12977517d24b3dc931375b`; its manpage matches the installed manpage except Homebrew's system-config-directory substitution. Citations below pin that commit.

Repository checks passed: `cargo fmt --check`, `cargo test` (49 passed), `cargo clippy --all-targets -- -D warnings`. No user-facing change means no changelog entry. Remaining limits: live proof is macOS/tmux 3.7c, not every supported tmux version or terminal; dedicated splits were assessed as a layout trade-off, not implemented/tested. OS notification delivery was deliberately suppressed. Sidebar footer behavior requires the owning worker's implementation and proof.

[manual]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/tmux.1
[status-doc]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/tmux.1#L5011-L5110
[message-cmd]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-display-message.c#L39-L177
[status-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/status.c#L241-L280
[redraw-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/screen-redraw.c#L775-L798
[message-setup]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/status.c#L460-L509
[input-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/server-client.c#L1442-L1500
[popup-cmd]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-display-menu.c#L400-L521
[popup-input]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/popup.c#L622-L644
[focus-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/window.c#L500-L534
[menu-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/menu.c#L634-L645
[overlay-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/server-client.c#L90-L122
[panes-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/cmd-display-panes.c#L80-L220
[alerts-src]: https://github.com/tmux/tmux/blob/e476c1230b958df0cb12977517d24b3dc931375b/alerts.c#L290-L326
