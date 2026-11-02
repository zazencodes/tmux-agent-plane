---
name: tmux-agent-plane
description: The system-wide index of the user's coding agents (Claude Code, Codex) across every tmux session, and the only way to work with them, through the `tmux-agent-plane` CLI. Load it first, before any tmux command, whenever a task touches another agent: the user names an agent (`claude-154`, `codex-26`, `reviewer`) or a pane, asks how an agent is doing or what agents are running, wants to check on, read, message, answer, wait for, show, close, start or spawn an agent (in a pane, window or session), wants work delegated to another agent, or wants a super agent that coordinates others.
---

# tmux-agent-plane

Every Claude Code and Codex agent running in the user's tmux is tracked by a background daemon. Each agent has a name (`claude-154`, or one given at spawn), a tmux pane id (`%154`) and a state:

- `working`: running a turn.
- `blocked`: waiting on a person: a question, a permission prompt, a dialog. `detail` says what, when the agent reports it.
- `done`: finished a turn nobody has looked at yet.
- `idle`: ready for input, already seen.

You are an agent in this list too. `list` marks you with `*` (or `"self": true` in JSON).

## Rule: the CLI, never raw tmux

Do every agent job through `tmux-agent-plane`: finding agents, reading their screens, typing into them, starting, showing and closing them. Never run `tmux list-panes`, `capture-pane`, `send-keys`, `split-window`, `new-window` or `kill-pane` against agents yourself. The daemon already indexes every agent in every session; `list` answers in one call what tmux would make you piece together, and the CLI keeps names, parents and done/seen state right. Raw tmux is fine only for panes that are not agents (a dev server, a log tail).

## Commands

```bash
tmux-agent-plane list                     # all agents: name, kind, state, where, parent, cwd, task
tmux-agent-plane list --json              # the same with every field
tmux-agent-plane read <agent> [--lines N] # its screen, plus N lines of scrollback
tmux-agent-plane send <agent> "<prompt>" [--wait] [--timeout S]
tmux-agent-plane keys <agent> <key>...    # tmux key names: Enter Escape Up Down Tab 1 2 y n
tmux-agent-plane wait <agent> [--timeout S]
tmux-agent-plane focus <agent>            # bring it on screen for the user
tmux-agent-plane toggle-sidebar "$TMUX_PANE"  # open the user's agent sidebar in your window (run again to close)
tmux-agent-plane kill <agent>             # close its pane, ending it
tmux-agent-plane spawn claude|codex ["<task>"] [--in pane|window|session] [--cwd DIR] [--name NAME] [--wait] [--timeout S]
```

`<agent>` is a name or pane id. `read` and `keys` also take any pane id, including a pane whose agent has not started yet.

## Checking on an agent

1. `list` to see its state and task.
2. `read <agent>` to see what it is doing. Add `--lines 200` for more history.
3. Report the state and the gist of the screen. Do not paste the whole screen back.

## Starting an agent

- `spawn` types the agent command into a fresh shell, so the user's aliases and flags apply, and the shell stays when the agent exits.
- `--in pane` (default) splits your pane beside you. Use `--in window` for longer independent work, `--in session` for a separate project. Follow what the user asked for.
- Give the full task as the prompt: the new agent has none of your context. Say which files, what done looks like, and what to report back.
- Name agents you will talk to again (`--name reviewer`).
- `spawn` prints `<name> <pane> <location>`. The spawned agent records you as its parent.
- A folder the agent has not trusted yet stops at a trust prompt, and `spawn` reports that the agent did not start. `read <pane>`, then answer with `keys <pane> ...` only when the user asked for that folder.

## Talking to an agent and waiting

- `send <agent> "<prompt>" --wait` pastes the prompt, presses Enter, and returns when that turn ends or the agent blocks. It prints `<name> <state>: <task>`.
- `spawn ... "<task>" --wait` returns when the first turn ends or the agent blocks.
- `wait <agent>` returns as soon as the agent is not `working`, so it returns at once for an idle agent. Use it to wait for work already running; use `send --wait` for work you start.
- Waits block until done. Pass `--timeout` shorter than your own shell tool limit, or run the wait in the background, then `read` the agent for its answer.
- A command fails loudly on a timeout, when the agent exits, or when the daemon is not running.

## Blocked agents

1. `read <agent>` to see the question or prompt.
2. If the answer is clear from the user's instructions, answer it: `keys <agent> 2` to pick option 2, `keys <agent> Enter` to confirm, `send <agent> "<text>"` to type a reply.
3. Otherwise, ask the user. Never approve a destructive or irreversible action on another agent's behalf without the user's say.

## Working as a super agent

When coordinating several agents: spawn each with a self-contained task and a name, `send --wait` or `wait` on them, `read` their results, and check `list` for anything `blocked` or `done`. Give each agent its own files or worktree so they do not edit the same files. When the user wants to look at an agent, `focus` it. Close a finished agent only when the user wants it gone: `kill <agent>`.
