#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private native-publisher proof, no models or user server/config changes.

uv run tests/sidebar_events_live.py target/debug/tmux-agent-plane
PTY decoding asserts UI only. Agent state comes exclusively from native fixtures/index.
Evidence persists in /tmp/tap-footer-*. Agent jobs use the branch CLI.
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

ROOT = Path(tempfile.mkdtemp(prefix="tap-footer-", dir="/tmp"))
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
           GROK_LEADER_SOCKET=str(ROOT / "absent.sock"), PLANE_FIXTURE_ROOT=str(ROOT),
           PLANE_FIXTURE_INPUT="1", TERM="xterm-256color",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
ENV.pop("TMUX", None)
ENV.pop("NO_COLOR", None)  # The agent tool environment disables colours; this PTY tests them.
# Keep Codex identity intact, use explicit names/window placements for agent jobs.
WRAPPER = BIN / "tmux"
WRAPPER.write_text("#!/bin/sh\n" +
    "printf '%s\\n' \"$*\" >> " + shlex.quote(str(ROOT / "tmux.calls")) + "\n" +
    "exec " + shlex.quote(TMUX) + " -S " + shlex.quote(str(ROOT / "t.sock")) + ' "$@"\n')
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    path = BIN / tool
    path.write_text("#!/bin/sh\n" +
        "if [ -e " + shlex.quote(str(ROOT / "desktop.fail")) + " ]; then echo injected >&2; exit 23; fi\n" +
        "echo delivered >> " + shlex.quote(str(ROOT / "desktop.delivered")) + "\n")
    path.chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/daemon/agent.c"), "-o", str(fixture)], check=True)
for kind in ("claude", "pi"):
    (BIN / kind).symlink_to(fixture)
(ROOT / "claude.command").write_text("busy 0")
(ROOT / "pi.command").write_text("working 0")


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


def change(state, count):
    temporary = ROOT / "pi.tmp"
    temporary.write_text(f"{state} {count}")
    temporary.replace(ROOT / "pi.command")


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


def geometry():
    # All panes here are infrastructure or sidebars. Agent focus is read from the index.
    return tmux("list-windows", "-a", "-F", "#{window_id} #{window_layout}")


def input_bytes():
    path = ROOT / "claude.input"
    return path.read_bytes() if path.exists() else b""


def assert_footer(label, text, colour):
    wait(label, lambda: all(text in c.footer() for c in CLIENTS))
    for client in CLIENTS:
        client.snapshot(label)
        assert client.screen.display[0].startswith(("TOP-STATUS", "tmux-agent-plane:"))
        assert client.screen.buffer[27][1].fg == colour, client.screen.buffer[27][1]


def popup_open():
    # The daemon's display-popup job lives exactly as long as the popup on screen.
    rows = subprocess.run(["ps", "-A", "-ww", "-o", "args="], check=True, text=True, capture_output=True).stdout
    return any("display-popup" in row and str(ROOT) in row for row in rows.splitlines())


def round_trip(key, layout):
    wait("no popup holding the keyboard", lambda: not popup_open())
    before = input_bytes()
    os.write(CLIENTS[0].master, key)
    wait("original client input", lambda: input_bytes() == before + key)
    assert geometry() == layout
    assert agent("footer-original")["visible"]
    assert b"\x1b[O" not in input_bytes()[len(before):], "fixture lost focus"


DAEMON = None
SPAWNED = []
RESULTS = {}
try:
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "scratch", "-x", "100", "-y", "28", "/bin/sh")
    tmux("set-option", "-g", "default-shell", "/bin/sh")
    tmux("set-option", "-g", "default-command", "/bin/sh")
    tmux("set-option", "-g", "status-position", "top")
    tmux("set-option", "-g", "status-format[0]", "TOP-STATUS")
    tmux("set-option", "-s", "focus-events", "on")
    tmux("set-option", "-as", "terminal-features", ",xterm-256color:RGB")
    stderr = (ROOT / "daemon.stderr").open("w")
    DAEMON = subprocess.Popen([str(BINARY), "daemon", "serve"], env=ENV, stdout=stderr, stderr=stderr)
    wait("daemon index", lambda: (STATE / "agents.json").exists())
    for kind, name in [("claude", "footer-original"), ("pi", "footer-source")]:
        plane("spawn", kind, "--in", "window", "--cwd", str(ROOT), "--name", name)
        SPAWNED.append(name)
    wait("working native agents", lambda: len(index()["agents"]) == 2 and
         all(a["state"] == "working" for a in index()["agents"]))
    # Prove events do not open closed sidebars.
    closed_layout = geometry()
    change("idle", 1)
    wait("event with sidebars closed", lambda: (index().get("recent_event") or {}).get("state") == "done")
    assert geometry() == closed_layout
    change("working", 1)
    wait("working again", lambda: agent("footer-source")["state"] == "working")
    plane("toggle-sidebar", agent("footer-original")["window"])
    CLIENTS.append(Client("scratch"))
    wait("first private client", lambda: tmux("list-clients", "-F", "#{client_name}"))
    plane("focus", "footer-original")
    # Second client uses an independent session; a plain scratch shell is infrastructure.
    tmux("new-session", "-d", "-s", "observer", "-x", "100", "-y", "28", "/bin/sh")
    observer_window = tmux("list-windows", "-t", "observer", "-F", "#{window_id}")
    plane("follow-sidebar", observer_window)
    CLIENTS.append(Client("observer"))
    wait("two sidebar hints", lambda: all("jump" in c.footer() for c in CLIENTS))
    # Explicit terminal focus event clears setup effects before measuring notifications.
    os.write(CLIENTS[0].master, b"\x1b[I")
    wait("original fixture visible", lambda: agent("footer-original")["visible"])
    layout = geometry()
    round_trip(b"a", layout)
    notification_input = input_bytes()
    (ROOT / "desktop.fail").touch()
    change("idle", 2)
    wait("done metadata", lambda: agent("footer-source")["completions"] == 2 and
         (index().get("recent_event") or {}).get("state") == "done")
    done = index()["recent_event"]
    assert done["expires_at"] - done["occurred_at"] == 5000
    assert abs(time.time() * 1000 - done["occurred_at"]) < 2000
    assert_footer("done", "✓ footer-source done", "a6e3a1")
    round_trip(b"b", layout)
    assert DAEMON.poll() is None
    # Another fast completed turn remains Done without an intervening Working sample.
    change("idle", 3)
    wait("same-state new completion event", lambda: agent("footer-source")["completions"] == 3 and
         index()["recent_event"]["occurred_at"] > done["occurred_at"])
    fast_done = index()["recent_event"]
    assert_footer("fast-done", "✓ footer-source done", "a6e3a1")
    wait("desktop failure visible", lambda: "notification:desktop" in tmux("show-option", "-gqv", "@agent_plane_status"))
    wait("expiry to hints", lambda: all("jump" in c.footer() for c in CLIENTS) and index()["recent_event"] is None)
    for c in CLIENTS:
        c.snapshot("expired")
    (ROOT / "desktop.fail").unlink()
    wait("desktop retry recovery", lambda: (ROOT / "desktop.delivered").read_text().count("delivered") == 3 and
         "[notification:desktop] recovered" in (STATE / "daemon.log").read_text())
    # Diagnostic announcements are popups that hold the keyboard, opened per client once it is idle.
    wait("recovery popups opened", lambda: sum(line.startswith("display-popup ") and "notification:desktop: recovered" in line
         for line in (ROOT / "tmux.calls").read_text().splitlines()) == 2)
    wait("recovery popups closed", lambda: not popup_open())
    change("working", 3)
    wait("working before block", lambda: agent("footer-source")["state"] == "working")
    change("blocked", 3)
    assert_footer("blocked", "◆ footer-source blocked", "f38ba8")
    blocked = index()["recent_event"]
    round_trip(b"c", layout)
    # Restart during a still-live event: retained agent state must not replay it.
    DAEMON.terminate()
    DAEMON.wait(timeout=5)
    DAEMON = subprocess.Popen([str(BINARY), "daemon", "serve"], env=ENV, stdout=stderr, stderr=stderr)
    wait("restart clears event", lambda: index()["updated_at"] > blocked["occurred_at"] and
         index()["recent_event"] is None and all("jump" in c.footer() for c in CLIENTS))
    assert geometry() == layout
    wait("restart popups closed", lambda: not popup_open())
    calls = (ROOT / "tmux.calls").read_text().splitlines()
    # Only diagnostic popups move focus, once per popup; agent notifications never do.
    original_client = next(line.split()[1] for line in tmux("list-clients", "-F", "#{client_pid} #{client_name}").splitlines()
                           if line.split()[0] == str(CLIENTS[0].process.pid))
    popups = sum(line.startswith(f"display-popup -c {original_client} ") for line in calls)
    observed = input_bytes()[len(notification_input):]
    assert popups >= 2 and observed.count(b"\x1b[O") == observed.count(b"\x1b[I") == popups, (popups, observed)
    assert observed.replace(b"\x1b[O", b"").replace(b"\x1b[I", b"") == b"bc", observed
    normal = [line for line in calls if line.startswith("display-message ") and "-p" not in line.split()
              and ("footer-source finished" in line or "footer-source needs you" in line)]
    assert not normal, normal
    # Both diagnostic paths are still active while desktop errors are injected.
    assert "[notification:desktop] ERROR:" in (STATE / "daemon.log").read_text()
    assert any(line.startswith("display-popup -c ") and "notification:desktop" in line for line in calls)
    RESULTS = {"passed": True, "done": done, "fast_done": fast_done, "blocked": blocked, "two_sessions": True,
               "footer_row": 28, "done_colour": "a6e3a1", "blocked_colour": "f38ba8",
               "expiry_returns_hints": True, "restart_clears_event": True,
               "closed_sidebars_stay_closed": True, "pane_input": "abc", "focus_moves_only_for_diagnostic_popups": True, "layout_unchanged": True,
               "desktop_failure_does_not_stop_events": True, "normal_display_messages": normal}
finally:
    for name in SPAWNED:
        try:
            plane("kill", name)
        except subprocess.CalledProcessError:
            pass
    if DAEMON and DAEMON.poll() is None:
        DAEMON.terminate()
        DAEMON.wait(timeout=5)
    for client in CLIENTS:
        client.close()
    tmux("kill-server", check=False)
    (ROOT / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
    print(json.dumps(RESULTS, indent=2))
    print("Evidence:", ROOT)
