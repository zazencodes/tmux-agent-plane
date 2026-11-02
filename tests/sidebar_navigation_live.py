#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private sidebar navigation proof: native Pi fixtures, no agent CLI/model calls.

uv run tests/sidebar_navigation_live.py target/debug/tmux-agent-plane
PTY decoding asserts UI only. Agent state comes exclusively from native fixtures/index.
Search editing pauses preview; Enter accepts and previews.
Evidence persists in /tmp/tap-nav-*. Agent jobs use the branch CLI.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

import pyte

ROOT = Path(tempfile.mkdtemp(prefix="tap-nav-", dir="/tmp"))
SOURCE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1]).resolve()
TMUX = shutil.which("tmux")
assert TMUX
BIN = ROOT / "bin"
BIN.mkdir()
STATE = ROOT / "s" / "tmux-agent-plane"
(STATE / "pi").mkdir(parents=True)
(ROOT / "home").mkdir()
(ROOT / "claude" / "sessions").mkdir(parents=True)
ENV = dict(os.environ, HOME=str(ROOT / "home"), XDG_STATE_HOME=str(ROOT / "s"),
           CLAUDE_CONFIG_DIR=str(ROOT / "claude"), GROK_HOME=str(ROOT / "grok"),
           GROK_LEADER_SOCKET=str(ROOT / "absent.sock"), PLANE_NAV_ROOT=str(ROOT), XDG_CONFIG_HOME=str(ROOT / "config"), TERM="xterm-256color",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
ENV.pop("TMUX", None)
ENV.pop("NO_COLOR", None)  # The agent tool environment disables colours; this PTY tests them.
# Keep Codex identity intact, use explicit names/window placements for agent jobs.
WRAPPER = BIN / "tmux"
WRAPPER.write_text(f"#!{sys.executable}\nimport json, os, sys\n"
    f"with open({str(ROOT / 'tmux.calls')!r}, 'a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
    f"if sys.argv[1] == 'switch-client' and os.path.exists({str(ROOT / 'switch.fail')!r}):\n"
    "    print('injected switch failure', file=sys.stderr); sys.exit(23)\n"
    f"os.execv({TMUX!r}, [{TMUX!r}, '-S', {str(ROOT / 't.sock')!r}, *sys.argv[1:]])\n")
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    path = BIN / tool
    path.write_text("#!/bin/sh\necho delivered >> " + shlex.quote(str(ROOT / "desktop.delivered")) + "\n")
    path.chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/sidebar/agent.c"), "-o", str(fixture)], check=True)
(BIN / "pi").symlink_to(fixture)


def tmux(*args, check=True):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args):
    result = subprocess.run([str(BINARY), *args], env=ENV, check=True, text=True,
                            capture_output=True, timeout=20)
    return result.stdout.strip()


def index():
    return json.loads((STATE / "agents.json").read_text())


def agent(name):
    return next(a for a in index()["agents"] if a["name"] == name)


def change(name, state, count):
    path = ROOT / (agent(name)["pane"] + ".command")
    temporary = path.with_suffix(".tmp")
    temporary.write_text(f"{state} {count}")
    temporary.replace(path)


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(100, 28)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 28, 100, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), "attach-session", "-t", session], env=ENV,
            stdin=self.slave, stdout=self.slave, stderr=self.slave)
        self.raw = (ROOT / f"{session}.raw").open("wb")

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            data = os.read(self.master, 65536)
            self.raw.write(data)
            self.raw.flush()
            self.stream.feed(data)

    def snapshot(self, label):
        self.drain()
        (ROOT / f"{self.process.pid}-{label}.txt").write_text("\n".join(self.screen.display) + "\n")

    def footer(self):
        return self.screen.display[-1][:38]

    def context(self):
        rows = tmux("list-clients", "-F", "#{client_pid} #{client_name} #{pane_id} #{window_id} #{session_name}")
        row = next(line for line in rows.splitlines() if line.split()[0] == str(self.process.pid))
        return row.split()[1:]

    def selected(self):
        self.drain()
        return next((line[1:38].split()[1] for line in self.screen.display if line[1:2] == "▌" and len(line[1:38].split()) > 1), None)

    def text(self):
        self.drain()
        return "\n".join(line[:38] for line in self.screen.display)

    def key(self, value):
        os.write(self.master, value)

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.raw.close()
        os.close(self.master)
        os.close(self.slave)


CLIENTS = []


def wait(label, condition, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        try:
            if condition():
                return
        except (FileNotFoundError, json.JSONDecodeError, StopIteration):
            pass
        time.sleep(0.05)
    for client in CLIENTS:
        client.snapshot("failure")
    raise AssertionError(f"timeout: {label}; evidence {ROOT}")


def sidebar_pane(window):
    # Sidebar infrastructure only; agent screens/state/input use plane/index/native fixtures.
    lines = tmux("list-panes", "-t", window, "-F", "#{pane_id} #{@agent-plane-sidebar}")
    return next(line.split()[0] for line in lines.splitlines() if line.endswith(" 1"))


def input_bytes(name):
    path = ROOT / (agent(name)["pane"] + ".input")
    return path.read_bytes() if path.exists() else b""


def query(client, value):
    client.key(b"\\\x15" + value.encode() + b"\r")
    wait("query " + value, lambda: ("\\ " + value) in client.text() and "▏" not in client.text().splitlines()[3])


def selected(client, name):
    wait("selected " + name, lambda: client.selected() == name)


def previewed(client, name):
    def arrived():
        target = sidebar_pane(agent(name)["window"])
        context = client.context()
        return (context[1] == target and context[2] == agent(name)["window"] and
                client.selected() == name and "V on" in client.text() and
                not tmux("show-option", "-pqv", "-t", target, "@agent-plane-sidebar-navigation"))
    wait("preview " + name + " with handoff applied", arrived)


def settle(seconds=1.2):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        for client in CLIENTS:
            client.drain()
        time.sleep(0.03)


SPAWNED = []
RESULTS = {}
try:
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "scratch", "-x", "100", "-y", "28", "/bin/sh")
    tmux("set-option", "-g", "default-shell", "/bin/sh")
    tmux("set-option", "-g", "default-command", "/bin/sh")
    tmux("set-option", "-g", "status-position", "top")
    tmux("set-option", "-g", "status-format[0]", "TOP-STATUS")
    tmux("set-option", "-as", "terminal-features", ",xterm-256color:RGB")
    plane("daemon", "start")
    wait("daemon index", lambda: (STATE / "agents.json").exists())
    for name, placement in [("nav-work", "window"), ("nav-done", "window"), ("nav-idle", "window"), ("nav-away", "session")]:
        plane("spawn", "pi", "--in", placement, "--cwd", str(ROOT), "--name", name)
        SPAWNED.append(name)
    wait("native fixtures", lambda: len(index()["agents"]) == 4)
    CLIENTS.append(Client("scratch"))
    client = CLIENTS[0]
    wait("client attached", lambda: tmux("list-clients", "-F", "#{client_name}"))
    change("nav-idle", "idle", 0)
    plane("focus", "nav-idle")
    wait("idle seen", lambda: agent("nav-idle")["state"] == "idle")
    plane("focus", "nav-work")
    change("nav-done", "idle", 1)
    change("nav-away", "blocked", 0)
    wait("states", lambda: agent("nav-done")["state"] == "done" and agent("nav-away")["state"] == "blocked")
    plane("toggle-sidebar", agent("nav-work")["window"])
    wait("sidebar controls", lambda: "V off" in client.text() and "F all" in client.text())
    selected(client, "nav-away")  # blocked first
    original = client.context()
    client.key(b"j")
    wait("normal move", lambda: client.selected() != "nav-away")
    assert client.context() == original
    query(client, "nav-work")
    selected(client, "nav-work")
    assert client.context() == original
    tmux("refresh-client", "-f", "active-pane", "-t", client.context()[0])
    flags = tmux("list-clients", "-F", "#{client_pid} #{client_flags}")
    assert "active-pane" in next(line for line in flags.splitlines() if line.split()[0] == str(client.process.pid))
    client.key(b"V")
    previewed(client, "nav-work")  # same window, sidebar remains active
    client.snapshot("same-window")
    # Clear query while viewing, then fast jjj. Exact net movement, one final handoff.
    client.key(b"\x1b")
    wait("query cleared before burst", lambda: "\\ search" in client.text())
    selected(client, "nav-work")
    ordered = [a["name"] for a in index()["agents"]]
    expected = ordered[(ordered.index("nav-work") + 3) % len(ordered)]
    client.key(b"jjj")
    previewed(client, expected)
    client.snapshot("burst-jjj")
    # Keys delivered while native switch commands are still in flight must also survive.
    start = expected
    for _ in range(7):
        client.key(b"j")
        time.sleep(0.015)
    expected = ordered[(ordered.index(start) + 7) % len(ordered)]
    previewed(client, expected)
    settle(0.7)
    assert client.selected() == expected
    client.snapshot("timed-stream-seven-j")
    # Mix text, movement and editing across preview switches. Assert content/order,
    # rather than only the commutative count of j presses.
    editing_context = client.context()
    calls_before_edit = len((ROOT / "tmux.calls").read_text().splitlines())
    client.key(b"\\")
    wait("timed mixed search starts", lambda: "preview" in client.footer() and "paused" in client.text())
    stream = [bytes([byte]) for byte in b"nav-"] + [b"\x1b[B"] + [bytes([byte]) for byte in b"done"] + [b"\x1b[A", b"\x7f", b"e"]
    for key in stream:
        client.key(key)
        time.sleep(0.035)
    settle(1.5)
    client.snapshot("timed-mixed-search")
    observed = client.text().splitlines()[3]
    (ROOT / "mixed-order.json").write_text(json.dumps({"expected": "nav-done", "query_row": observed}) + "\n")
    assert "\\ nav-done▏" in observed, observed
    assert client.context() == editing_context, "search must stay in its originating sidebar"
    edit_calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()[calls_before_edit:]]
    assert not any(call[0] == "switch-client" for call in edit_calls), edit_calls
    client.key(b"\r")
    previewed(client, "nav-done")
    wait("timed search accepted", lambda: "paused" not in client.text() and "\\ nav-done" in client.text())
    client.key(b"\x1b")
    wait("timed query cleared", lambda: "\\ search" in client.text())
    expected = "nav-done"
    # Successive arrows after handoff use the transferred selection, never destination defaults.
    expected = ordered[(ordered.index(expected) + 1) % len(ordered)]
    client.key(b"\x1b[B")
    previewed(client, expected)
    expected = ordered[(ordered.index(expected) - 1) % len(ordered)]
    client.key(b"\x1b[A")
    previewed(client, expected)
    # Incremental search and arrows stay local; accepting previews without agent focus.
    editing_context = client.context()
    client.key(b"\\nav-do")
    wait("partial query stays local", lambda: "paused" in client.text() and "\\ nav-do" in client.text())
    assert client.context() == editing_context
    client.key(b"ne")
    wait("incremental query", lambda: "\\ nav-done" in client.text() and "preview" in client.footer())
    assert client.context() == editing_context
    client.key(b"\r")
    previewed(client, "nav-done")
    wait("Enter accepts search without focus", lambda: "paused" not in client.text())
    settle(2.2)
    assert agent("nav-done")["state"] == "done" and not agent("nav-done")["visible"]
    assert all(input_bytes(name) == b"" for name in SPAWNED)
    client.snapshot("preview-done-unseen")
    client.key(b"\r")
    wait("Enter focuses and marks seen", lambda: agent("nav-done")["state"] == "idle" and agent("nav-done")["visible"] and client.context()[1] == agent("nav-done")["pane"])
    client.key(b"x")
    wait("deliberate agent input", lambda: input_bytes("nav-done") == b"x")
    # Return to sidebar infrastructure, without sending an agent key.
    tmux("switch-client", "-c", client.context()[0], "-t", sidebar_pane(agent("nav-done")["window"]))
    wait("back in sidebar", lambda: "V on" in client.text())
    client.key(b"V\x1b")
    wait("mode off", lambda: "V off" in client.text() and "\\ search" in client.text())
    original = client.context()
    client.key(b"jj")
    settle(0.3)
    assert client.context() == original
    # Restore all four states. Only F may skip categories/change the state filter.
    change("nav-done", "idle", 2)
    wait("done again", lambda: agent("nav-done")["state"] == "done" and "✓ 1" in client.text())
    for state in ["blocked", "done", "working", "idle", "all"]:
        client.key(b"F")
        wait("filter " + state, lambda state=state: "F " + state in client.text())
        client.snapshot("filter-" + state)
    query(client, "NAV-WORK")
    client.key(b"F")
    wait("query skips empty categories", lambda: "F working" in client.text())
    change("nav-work", "blocked", 0)
    wait("filter stays empty", lambda: "F working" in client.text() and "no matching agents" in client.text())
    original = client.context()
    settle(1.2)
    assert "F working" in client.text() and client.context() == original
    client.key(b"F")
    wait("All reached", lambda: "F all" in client.text())
    client.key(b"F")
    wait("blocked query category", lambda: "F blocked" in client.text())
    # Search editing: V/F/r/q are text, Escape restores accepted query, second Escape clears.
    client.key(b"\\qVFr")
    wait("empty literal query", lambda: "no matching agents" in client.text() and "keep" in client.footer())
    client.key(b"\x1b")
    wait("cancel query", lambda: "\\ NAV-WORK" in client.text() and "keep" not in client.footer())
    client.key(b"\x1b")
    wait("query cleared", lambda: "\\ search" in client.text())
    client.key(b"F")  # blocked -> done
    wait("done filter", lambda: "F done" in client.text())
    client.key(b"F")
    wait("skip working now empty", lambda: "F idle" in client.text())
    client.key(b"F")
    wait("all filter", lambda: "F all" in client.text())
    # A live event yields to search controls, rename controls and rename errors.
    change("nav-done", "idle", 3)
    wait("fresh event", lambda: (index().get("recent_event") or {}).get("name") == "nav-done" and agent("nav-done")["completions"] == 3)
    wait("bottom event", lambda: "nav-done done" in client.footer())
    client.key(b"\\")
    wait("search controls over event", lambda: "keep" in client.footer())
    client.snapshot("search-priority")
    client.key(b"\x1b")
    wait("search cancelled", lambda: "keep" not in client.footer())
    client.key(b"r")
    wait("rename controls over event", lambda: "save" in client.footer())
    client.key(b"\x15\r")
    wait("rename error priority", lambda: "name" in client.footer() and "save" not in client.footer())
    client.snapshot("rename-error")
    client.key(b"\x1b")
    wait("event resumes", lambda: "nav-done done" in client.footer())
    # Independent observer stays fixed while first client previews another session.
    tmux("new-session", "-d", "-s", "observer", "-x", "100", "-y", "28", "/bin/sh")
    observer_window = tmux("list-windows", "-t", "observer", "-F", "#{window_id}")
    plane("follow-sidebar", observer_window)
    CLIENTS.append(Client("observer"))
    observer = CLIENTS[1]
    wait("observer", lambda: "V off" in observer.text())
    observer_context = observer.context()
    query(client, "nav-away")
    client.key(b"V")
    previewed(client, "nav-away")
    assert observer.context() == observer_context
    client.snapshot("cross-session-independent")
    observer.snapshot("observer-unchanged")
    # Destination with another client must be refused before a handoff/window mutation.
    client.key(b"V")
    query(client, "nav-work")
    client.key(b"V")
    previewed(client, "nav-work")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=nav-away:")
    observer_context = observer.context()
    source_context = client.context()
    query(client, "nav-away")  # view on: target shared; selection changes, no client moves
    wait("shared target refusal", lambda: "target shared" in client.footer())
    assert client.context() == source_context and observer.context() == observer_context
    client.snapshot("shared-target-refused")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=observer:")
    # Ambiguous input origin: two clients share this sidebar; refuse focus and preview.
    tmux("switch-client", "-c", observer.context()[0], "-t", source_context[1])
    client.key(b"V")  # mode off
    wait("view off before ambiguity test", lambda: "V off" in client.text())
    client.key(b"V")
    wait("ambiguous client refusal", lambda: "multiple clients" in client.footer())
    client.snapshot("ambiguous-refused")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=observer:")
    # Inject a real tmux switch-client failure after HANDOFF was written.
    client.key(b"V")  # off
    query(client, "nav-done")
    target_sidebar = sidebar_pane(agent("nav-done")["window"])
    (ROOT / "switch.fail").touch()
    client.key(b"V")
    wait("injected switch failure", lambda: "switch-client" in client.footer())
    assert not tmux("show-option", "-pqv", "-t", target_sidebar, "@agent-plane-sidebar-navigation")
    assert client.context() == source_context
    client.snapshot("failed-switch-clears-handoff")
    (ROOT / "switch.fail").unlink()
    # Rename cancellation when its original target leaves: never rename the next row.
    client.key(b"V")
    query(client, "nav-idle")
    client.key(b"r")
    wait("rename active", lambda: "save" in client.footer())
    plane("kill", "nav-idle")
    SPAWNED.remove("nav-idle")
    wait("rename target vanished", lambda: "rename cancelled" in client.footer())
    client.snapshot("rename-exit")
    client.key(b"\x1b")
    query(client, "no-agent-matches")
    wait("empty", lambda: "no matching agents" in client.text())
    tmux("resize-pane", "-t", client.context()[1], "-x", "20")
    settle(0.5)
    client.snapshot("narrow-empty")
    assert "V off" in client.text() and "F all" in client.text()
    wait("event expired", lambda: index().get("recent_event") is None and "jump" in client.footer())
    for name in SPAWNED:
        assert input_bytes(name) == (b"x" if name == "nav-done" else b""), (name, input_bytes(name))
    calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()]
    first_preview = next(i for i, call in enumerate(calls) if call[0] == "switch-client" and "-c" in call)
    switches = [call for call in calls[first_preview:] if call[0] == "switch-client"]
    assert all("-c" in call for call in switches), switches
    marked = {call[call.index("-t") + 1] for call in calls
              if call[0] == "set-option" and "@agent-plane-sidebar" in call and call[-1] == "1"}
    # Spawn types into fixture shells before they become agents. Thereafter, relay sends
    # only to marked sidebar panes; native fixture input logs are the final byte-level guard.
    first_sidebar = next(i for i, call in enumerate(calls)
                         if call[0] == "set-option" and "@agent-plane-sidebar" in call)
    relays = [call for call in calls[first_sidebar:] if call[0] == "send-keys"]
    assert relays and all(call[call.index("-t") + 1] in marked for call in relays), relays
    assert not any(call[0] in ("copy-mode", "clock-mode") for call in calls)
    RESULTS = {"passed": True, "native_fixtures": 4, "models_used": 0,
        "same_window": True, "cross_window_and_session": True, "view_off_selects_only": True,
        "burst_jjj_exact_net_movement": True, "timed_stream_seven_j_exact_net_movement": True,
        "arrow_handoff_identity": True, "timed_mixed_text_arrows_backspace_order": True,
        "active_pane_client_flag": True, "search_edit_and_arrows_stay_local_until_accepted": True,
        "preview_done_stays_unseen": True, "Enter_marks_seen": True,
        "composed_query_filter_cycle": True, "refresh_keeps_empty_filter": True,
        "search_cancel_keep_clear_literal_keys": True, "footer_priority_and_expiry": True,
        "independent_clients_unchanged": True, "shared_target_and_ambiguous_origin_refused": True,
        "failed_switch_clears_handoff": True, "rename_exit_cannot_retarget": True,
        "narrow_empty": True, "unintended_agent_input": 0}
finally:
    for name in SPAWNED:
        plane("kill", name)
    plane("daemon", "stop")
    for client in CLIENTS:
        client.close()
    tmux("kill-server", check=False)
    (ROOT / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
    print(json.dumps(RESULTS, indent=2))
    print("Evidence:", ROOT)
