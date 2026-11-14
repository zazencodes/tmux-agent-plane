#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private cleanup proof on an isolated tmux server: a scripted background command, no model calls.

Usage: uv run tests/cleanup_live.py target/debug/tmux-agent-plane

Drives `z` in a real sidebar: the prompt and its cancel, the request to the daemon, what the
background command is offered, a choice it was not offered rejecting the whole answer, and an
accepted answer closing exactly the exited task it chose. Agents and sessions qualify only after
30 quiet minutes, so their rules are covered by the unit tests in src/cleanup.rs.
"""
import fcntl
import json
import os
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte

ROOT = Path(tempfile.mkdtemp(prefix="tap-clean-", dir="/tmp"))
SOURCE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1]).resolve()
TMUX = shutil.which("tmux")
assert TMUX
SOCKET = str(ROOT / "t.sock")
BIN = ROOT / "bin"
BIN.mkdir()
STATE = ROOT / "s" / "tmux-agent-plane"
(STATE / "pi").mkdir(parents=True)
(ROOT / "home").mkdir()
WORK = ROOT / "work"
WORK.mkdir()
ENV = dict(os.environ, HOME=str(ROOT / "home"), XDG_STATE_HOME=str(ROOT / "s"),
           CLAUDE_CONFIG_DIR=str(ROOT / "claude"), GROK_HOME=str(ROOT / "grok"),
           GROK_LEADER_SOCKET=str(ROOT / "absent.sock"), PLANE_NAV_ROOT=str(ROOT),
           XDG_CONFIG_HOME=str(ROOT / "config"), TERM="xterm-256color", SHELL="/bin/sh",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
for name in ("TMUX", "TMUX_PANE", "CODEX_THREAD_ID", "CODEX_SESSION_ID", "NO_COLOR"):
    ENV.pop(name, None)
WRAPPER = BIN / "tmux"
# Fault popups take the keyboard and would eat the test's keys; daemon.log and the status bar
# show the fault instead.
WRAPPER.write_text("#!/bin/sh\n[ \"$1\" = display-popup ] && exit 0\n"
                   f"exec {TMUX} -S {SOCKET} \"$@\"\n")
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    (BIN / tool).write_text("#!/bin/sh\n")
    (BIN / tool).chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/sidebar/agent.c"),
                "-o", str(fixture)], check=True)
(BIN / "pi").symlink_to(fixture)
# The background command: records each prompt and prints the answer the test prepared.
MODEL = BIN / "fake-model"
MODEL.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$1\" >> {ROOT / 'prompts.txt'}\n"
                 f"cat {ROOT / 'answer.json'}\n")
MODEL.chmod(0o755)
CONFIG = ROOT / "config" / "tmux-agent-plane" / "config.json"
CONFIG.parent.mkdir(parents=True)
CONFIG.write_text(json.dumps({"auto_name": {"enabled": False,
                                            "command": ["fake-model", "{prompt}"],
                                            "output": "text"}}))
RESULTS = {}
CLIENTS = []


def tmux(*args, check=True):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args, check=True):
    result = subprocess.run([str(BINARY), *args], env=ENV, text=True, capture_output=True,
                            timeout=40)
    if check and result.returncode != 0:
        raise AssertionError(f"{args} failed: {result.stderr}")
    return result


def eventually(label, predicate, timeout=15):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.2)
    for client in CLIENTS:
        print(client.text(), file=sys.stderr)
    raise AssertionError(f"timed out: {label}")


def index():
    return json.loads((STATE / "agents.json").read_text())


def task(name):
    return next((t for t in index()["tasks"] if t["name"] == name), None)


def prompts():
    path = ROOT / "prompts.txt"
    return path.read_text().splitlines() if path.exists() else []


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    """A real attached client in a PTY, decoded with pyte. The sidebar is the left 38 columns."""

    def __init__(self, session):
        self.screen = Screen(110, 30)
        self.stream = pyte.ByteStream(self.screen)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 30, 110, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), "attach-session", "-t", session], env=ENV,
                                        stdin=slave, stdout=slave, stderr=slave)

    def text(self):
        while select.select([self.master], [], [], 0.05)[0]:
            self.stream.feed(os.read(self.master, 65536))
        return "\n".join(line[:38] for line in self.screen.display)

    def where(self):
        rows = tmux("list-clients", "-F", "#{client_pid} #{window_id} #{pane_id}")
        return next(r.split()[1:] for r in rows.splitlines() if r.split()[0] == str(self.process.pid))

    def key(self, data):
        os.write(self.master, data)
        time.sleep(0.4)


def check(label, condition):
    assert condition, label
    RESULTS[label] = True
    print("ok", label)


try:
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "harness", "-x", "160", "-y", "40", "/bin/sh")
    subprocess.run(["bash", str(SOURCE.parent / "tmux-agent-plane.tmux")], env=ENV, check=True)
    plane("daemon", "start")
    eventually("daemon index", lambda: (STATE / "agents.json").exists())
    plane("spawn", "pi", "--in", "window", "--session", "harness", "--cwd", str(WORK),
          "--name", "fresh")
    eventually("agent", lambda: any(a["name"] == "fresh" for a in index()["agents"]))
    for name, command in (("old", ["sh", "-c", "exit 1"]), ("keep", ["true"]),
                          ("web", ["sleep", "600"])):
        plane("task", "start", name, "--cwd", str(WORK), "--", *command)
    eventually("tasks settle", lambda: task("old") and task("old")["state"] == "exited"
               and task("keep") and task("keep")["state"] == "exited"
               and task("web") and task("web")["state"] == "running")

    client = Client("harness")
    CLIENTS.append(client)
    eventually("client attached", lambda: tmux("list-clients"))
    plane("toggle-sidebar", eventually("client window", lambda: client.where())[0])
    eventually("sidebar", lambda: "fresh" in client.text())

    client.key(b"z")
    text = eventually("cleanup prompt", lambda: "clean up main? [Y/n]" in (t := client.text()) and t)
    check("z asks to clean up the workspace and names the command", "fake-model" in text)
    client.key(b"n")
    eventually("prompt gone", lambda: "clean up main?" not in client.text())
    time.sleep(1.5)
    check("any other key cancels without asking the model", prompts() == [])

    # A choice that was not offered (a running task) rejects the whole answer.
    (ROOT / "answer.json").write_text(json.dumps({
        "agents": [], "sessions": [],
        "tasks": [{"name": "old", "reason": "failed long ago"},
                  {"name": "web", "reason": "not needed"}]}))
    client.key(b"z")
    eventually("cleanup prompt again", lambda: "clean up main?" in client.text())
    client.key(b"\r")
    eventually("rejection in the footer", lambda: "cleanup failed" in client.text(), 20)
    check("a rejected answer closes nothing",
          task("old") and task("web") and task("keep"))
    offered = json.loads(prompts()[0].split("Inventory: ", 1)[1])
    check("only exited tasks are offered",
          sorted(t["name"] for t in offered["exited_tasks"]) == ["keep", "old"])
    check("a fresh agent is not closable",
          [a["closable"] for a in offered["agents"] if a["name"] == "fresh"] == [False])
    eventually("cleanup fault in the status bar",
               lambda: "⚠" in tmux("show-option", "-gqv", "@agent_plane_status"))

    # An answer within what was offered closes exactly its choice.
    (ROOT / "answer.json").write_text(json.dumps({
        "agents": [], "sessions": [],
        "tasks": [{"name": "old", "reason": "failed long ago"}]}))
    client.key(b"z")
    eventually("cleanup prompt a third time", lambda: "clean up main?" in client.text())
    client.key(b"y")
    eventually("result in the footer", lambda: "closed 1 task" in client.text(), 20)
    eventually("old leaves the list", lambda: task("old") is None)
    check("the other tasks and the agent stay", task("keep") and task("web")
          and any(a["name"] == "fresh" for a in index()["agents"]))
    log = (STATE / "daemon.log").read_text()
    check("daemon.log records the reason", "closed task old: failed long ago" in log)
    eventually("fault clears after a good cleanup",
               lambda: "⚠" not in tmux("show-option", "-gqv", "@agent_plane_status"))
    print(json.dumps({"passed": True, "checks": len(RESULTS), "root": str(ROOT)}))
finally:
    plane("daemon", "stop", check=False)
    subprocess.run([TMUX, "-S", SOCKET, "kill-server"], env=ENV, capture_output=True)
