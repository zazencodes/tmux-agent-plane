#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private background-task proof on an isolated tmux server: no agent CLI or model calls.

Usage: uv run tests/tasks_live.py target/debug/tmux-agent-plane

Starts real long-running commands as tasks and checks list/ports, reuse, clashes, read, crash
states, restart, stop, that a task running an agent CLI is never an agent, and that a task
outlives the agent that started it. The sidebar part drives a real sidebar in a PTY.
"""
import fcntl
import json
import os
import pty
import re
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

ROOT = Path(tempfile.mkdtemp(prefix="tap-task-", dir="/tmp"))
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
for name in ("TMUX", "TMUX_PANE", "CODEX_THREAD_ID", "CODEX_SESSION_ID"):
    ENV.pop(name, None)
WRAPPER = BIN / "tmux"
WRAPPER.write_text(f"#!/bin/sh\nexec {TMUX} -S {SOCKET} \"$@\"\n")
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    (BIN / tool).write_text("#!/bin/sh\n")
    (BIN / tool).chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/sidebar/agent.c"),
                "-o", str(fixture)], check=True)
(BIN / "pi").symlink_to(fixture)
RESULTS = {}
CLIENTS = []


def tmux(*args, check=True):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args, check=True, env=None):
    result = subprocess.run([str(BINARY), *args], env=env or ENV, text=True,
                            capture_output=True, timeout=40)
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
        print(client.text(), client.where(), tmux("list-panes", "-a", "-F",
              "#{session_name} #{window_id} #{pane_id} #{@agent-plane-sidebar} #{pane_active}"),
              sep="\n", file=sys.stderr)
    raise AssertionError(f"timed out: {label}")


def index():
    return json.loads((STATE / "agents.json").read_text())


def task(name):
    return next((t for t in index()["tasks"] if t["name"] == name), None)


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

    def selected(self):
        return next((line for line in self.text().splitlines() if line[1:2] == "▌"), "")

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
    plane("daemon", "start")
    eventually("daemon index", lambda: (STATE / "agents.json").exists())

    # A real agent fixture: the positive control for detection, and a task starter.
    plane("spawn", "pi", "--in", "window", "--session", "harness", "--cwd", str(WORK),
          "--name", "starter")
    starter = eventually("starter agent", lambda: next(
        (a for a in index()["agents"] if a["name"] == "starter"), None))

    # Start a dev server on a free port from inside the agent's pane: started_by is the agent.
    as_starter = dict(ENV, TMUX_PANE=starter["pane"])
    out = plane("task", "start", "web", "--cwd", str(WORK), "--", "python3", "-m", "http.server",
                "0", "--bind", "127.0.0.1", env=as_starter).stdout.split()
    check("start prints name pane state", out[0] == "web" and out[1].startswith("%")
          and out[2] == "running")
    web_pane = out[1]
    screen = eventually("server banner", lambda: re.search(
        r"port (\d+)", plane("task", "read", "web").stdout))
    port = int(screen.group(1))
    listed = eventually("port in index", lambda: (t := task("web")) and t["ports"] and t, 12)
    check("list shows the listening port", listed["ports"] == [port])
    check("list shows definition", listed["cwd"] == str(WORK.resolve())
          and listed["command"] == ["python3", "-m", "http.server", "0", "--bind", "127.0.0.1"]
          and listed["started_by"] == "starter" and listed["state"] == "running")
    text = plane("task", "list").stdout
    check("text list has port and starter", f":{port}" in text and "starter" in text
          and "running" in text)
    check("task window lives in the tasks session",
          listed["session"] == "agent-plane-tasks"
          and tmux("display-message", "-p", "-t", web_pane, "#{window_name}") == "web")
    check("tasks session is not a project", "agent-plane-tasks" not in plane(
        "sessions", "list").stdout)

    # Reuse and clash.
    again = plane("task", "start", "web", "--cwd", str(WORK), "--", "python3", "-m", "http.server",
                  "0", "--bind", "127.0.0.1").stdout.strip()
    check("same spec reuses the running task", again == f"web {web_pane} running (already running)")
    clash = plane("task", "start", "web", "--cwd", str(WORK), "--", "python3", "-m", "http.server",
                  "9", check=False)
    check("different command fails loudly with the existing definition",
          clash.returncode != 0 and str(WORK.resolve()) in clash.stderr
          and "python3 -m http.server 0" in clash.stderr and "starter" in clash.stderr)
    clash = plane("task", "start", "web", "--cwd", "/tmp", "--", "python3", "-m", "http.server",
                  "0", "--bind", "127.0.0.1", check=False)
    check("different cwd fails loudly", clash.returncode != 0 and "different work" in clash.stderr)
    check("bad names and shell syntax are refused",
          plane("task", "start", "two words", "--", "true", check=False).returncode != 0
          and plane("task", "start", "x", "--", "npm run dev", check=False).returncode != 0)

    # A crash keeps its output and exit status; restart reruns it in the same pane.
    out = plane("task", "start", "crash", "--cwd", str(WORK), "--", "sh", "-c",
                "echo run-$$; sleep 2; exit 3").stdout.split()
    crash_pane = out[1]
    crashed = eventually("crash exit", lambda: (t := task("crash")) and t["state"] == "exited" and t)
    check("crash shows exited 3", crashed["exit_code"] == 3
          and "exited 3" in plane("task", "list").stdout)
    check("crash output stays readable", "run-" in plane("task", "read", "crash", "--lines", "50").stdout)
    out = plane("task", "restart", "crash").stdout.split()
    check("restart reruns in the same pane", out[:3] == ["crash", crash_pane, "running"])
    eventually("crash running again", lambda: (t := task("crash")) and t["state"] == "running")
    eventually("crash exits again", lambda: (t := task("crash")) and t["state"] == "exited")
    check("restart clears the earlier run", plane("task", "read", "crash", "--lines", "200")
          .stdout.count("run-") == 1)
    again = plane("task", "start", "crash", "--cwd", str(WORK), "--", "sh", "-c",
                  "echo run-$$; sleep 2; exit 3").stdout.strip()
    check("starting an exited task with the same spec reruns it",
          again == f"crash {crash_pane} running (restarted)")

    plane("task", "start", "signalled", "--cwd", str(WORK), "--", "sh", "-c", "kill -TERM $$")
    killed = eventually("signal exit", lambda: (t := task("signalled")) and t["state"] == "exited" and t)
    check("a signal shows as 128 + its number", killed["exit_code"] == 143)
    plane("task", "start", "quick", "--cwd", str(WORK), "--", "true")
    quick = eventually("instant exit", lambda: (t := task("quick")) and t["state"] == "exited" and t)
    check("a command that ends at once still leaves its status", quick["exit_code"] == 0)

    # A task running an agent CLI is never an agent; the same CLI in a plain pane is.
    plane("task", "start", "fake-agent", "--cwd", str(WORK), "--", "pi")
    time.sleep(3)
    agent_panes = {a["pane"] for a in index()["agents"]}
    check("a task running an agent CLI is never an agent",
          task("fake-agent")["pane"] not in agent_panes and starter["pane"] in agent_panes)
    check("no tracking faults", "⚠" not in tmux("show-option", "-gqv", "@agent_plane_status"))

    # The starter exits; its task keeps running.
    plane("kill", "starter")
    eventually("starter gone", lambda: all(a["name"] != "starter" for a in index()["agents"]))
    time.sleep(1.5)
    check("a task outlives its starter", task("web")["state"] == "running"
          and task("web")["ports"] == [port])

    # Stop ends tasks; they leave the list and the session goes with the last one.
    out = plane("task", "stop", "web").stdout.strip()
    check("stop ends the task", out == f"stopped web ({web_pane})")
    eventually("web leaves the list", lambda: task("web") is None)
    for name in ("crash", "signalled", "quick", "fake-agent"):
        plane("task", "stop", name)
    eventually("no tasks", lambda: not index()["tasks"])
    check("the tasks session ends with its last task",
          "agent-plane-tasks" not in tmux("list-sessions", "-F", "#{session_name}"))
    check("unknown task fails loudly", plane("task", "read", "web", check=False).returncode != 0)
    log = (STATE / "daemon.log").read_text()
    check("daemon log has no task faults", "[tasks" not in log and "[scan:" not in log)

    # Sidebar: t shows tasks below the agents; j/k/G preview them, Enter opens, R restarts and
    # x asks to stop. The plugin's hooks run as they would for a user.
    subprocess.run(["bash", str(SOURCE.parent / "tmux-agent-plane.tmux")], env=ENV, check=True)
    plane("spawn", "pi", "--in", "window", "--session", "harness", "--cwd", str(WORK),
          "--name", "watcher")
    watcher = eventually("watcher", lambda: next(
        (a for a in index()["agents"] if a["name"] == "watcher"), None))
    plane("task", "start", "web2", "--cwd", str(WORK), "--", "python3", "-m", "http.server", "0",
          "--bind", "127.0.0.1")
    plane("task", "start", "flaky", "--cwd", str(WORK), "--", "sh", "-c",
          "echo flaky-$$; sleep 3; exit 2")
    web2 = eventually("web2 port", lambda: (t := task("web2")) and t["ports"] and t, 15)
    eventually("flaky exits", lambda: task("flaky")["state"] == "exited")
    client = Client("harness")
    CLIENTS.append(client)
    eventually("client attached", lambda: tmux("list-clients"))
    plane("toggle-sidebar", eventually("client window", lambda: client.where())[0])
    eventually("sidebar footer hint", lambda: "t tasks" in client.text())
    check("sidebar hides tasks until t", "── tasks" not in client.text())
    client.key(b"t")
    text = eventually("tasks section", lambda: "── tasks" in (t := client.text()) and t)
    web_row = next(line for line in text.splitlines() if "web2" in line)
    flaky_row = next(line for line in text.splitlines() if "flaky" in line)
    check("sidebar rows show port, exit code and project",
          f":{web2['ports'][0]}" in web_row and "▸" in web_row and "work" in web_row
          and "✕" in flaky_row and "exit 2" in flaky_row)
    check("tasks come after the agents", text.index("── tasks") > text.index("watcher"))
    client.key(b"G")
    eventually("G selects the last task", lambda: "web2" in client.selected())
    eventually("moving onto a task previews its window",
               lambda: client.where()[0] == web2["window"])
    check("task footer offers stop and restart", "x stop" in client.text())
    client.key(b"k")
    eventually("k selects flaky", lambda: "flaky" in client.selected())
    flaky = task("flaky")
    client.key(b"R")
    eventually("R restarts flaky", lambda: task("flaky")["started_at"] > flaky["started_at"])
    check("R restarts the selected task", True)
    client.key(b"\r")
    eventually("Enter opens the task", lambda: client.where()[1] == flaky["pane"])
    check("Enter puts the keyboard in the task", True)
    plane("enter-sidebar", flaky["window"])
    eventually("back in the sidebar", lambda: client.where()[1] != flaky["pane"])
    client.key(b"j")
    eventually("j moves to web2", lambda: "web2" in client.selected())
    client.key(b"x")
    eventually("stop asks", lambda: "stop web2" in client.text() and "[Y/n]" in client.text())
    client.key(b"n")
    eventually("any other key cancels", lambda: "[Y/n]" not in client.text())
    time.sleep(1)
    check("a cancelled stop leaves the task", task("web2") is not None)
    client.key(b"x")
    eventually("stop asks again", lambda: "[Y/n]" in client.text())
    client.key(b"y")
    eventually("y stops web2", lambda: task("web2") is None)
    check("the sidebar moves to the next row before stopping",
          client.where()[0] == flaky["window"] and "flaky" in client.selected())
    client.key(b"t")
    eventually("t hides tasks", lambda: "── tasks" not in client.text())
    check("the hint stays while tasks exist", "t tasks" in client.text())
    typed = (ROOT / (watcher["pane"] + ".input"))
    keys = typed.read_bytes().replace(b"\x1b[I", b"").replace(b"\x1b[O", b"") if typed.exists() else b""
    check("no sidebar key reached an agent", keys == b"")
    check("still no tracking faults", "⚠" not in tmux("show-option", "-gqv", "@agent_plane_status"))
    plane("task", "stop", "flaky")
    client.process.terminate()
    print(json.dumps({"passed": True, "checks": len(RESULTS), "root": str(ROOT)}))
finally:
    plane("daemon", "stop", check=False)
    subprocess.run([TMUX, "-S", SOCKET, "kill-server"], env=ENV, capture_output=True)
