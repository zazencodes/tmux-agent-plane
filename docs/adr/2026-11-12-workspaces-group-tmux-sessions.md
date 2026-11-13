# Workspaces group whole tmux sessions, and only the plane's views are scoped

## Decision

- A workspace is a named group of tmux sessions, one level above tmux. Every session belongs to exactly one, recorded on the session itself, so membership survives daemon restarts and needs no store of its own. A workspace exists while it has sessions.
- Only the plane's own views are scoped: the sidebar, every picker (the Telescope one included), the agent keys, the sidebar's notices and tasks, and each session's status bar show the workspace being looked from. A view asks the plane which workspace its pane is in rather than working it out itself. tmux is untouched: its session list, switching and detaching still see every session. The CLI lists every agent with its workspace, so agents coordinate across them.
- A session joins the workspace of whoever creates it: the named one, else the creating agent's or pane's, else the one you are in (that of the client used last). Sessions made outside the plane join the one you are in.
- Nothing moves work between workspaces on its own. Automatic folder projects stay inside the agent's workspace; organizing projects never crosses workspaces; agents change workspaces only when the user explicitly asks.
- Closing a session keeps its clients in its workspace while that has sessions left.
- `main` is home: it cannot be renamed or closed, and no other workspace takes its name. Closing a workspace moves its sessions there intact; ending them takes a second, explicit confirmation.
- The name is "workspace", because "project" already means a tmux session here.

## Why

The user wants to set a body of work aside the way they detach from tmux, and see only the work in front of them, without tmux behaving any differently. Grouping whole sessions keeps the existing session-is-a-project model intact: a session never shows up in two workspaces, and moving an agent between workspaces is the existing move between sessions. Agents acting on their own across workspaces would confuse people, so every crossing is explicit.

## Rejected

- **Membership per agent or pane.** A session could then belong to several workspaces, and the sidebar's grouping by session would split.
- **Hiding sessions from tmux** (separate servers or sockets per workspace). The user keeps using tmux exactly as before; one server also keeps one daemon, one index and one restore.
- **One global "current workspace" for every view.** Two clients could not look at different workspaces, and a sidebar in a background window would show the wrong one.
- **Scoping `list` to the caller's workspace.** Codex callers cannot be identified reliably, and lead agents need to see everything to coordinate.
