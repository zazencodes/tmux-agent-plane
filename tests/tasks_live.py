#!/usr/bin/env python3
"""Private background-task proof on an isolated tmux server: no agent CLI or model calls.

Usage: tests/tasks_live.py target/debug/tmux-agent-plane

Starts real long-running commands as tasks and checks list/ports, reuse, clashes, read, crash
states, restart, stop, that a task running an agent CLI is never an agent, and that a task
outlives the agent that started it. The sidebar part drives a real sidebar in a PTY.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

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
    raise AssertionError(f"timed out: {label}")


def index():
    return json.loads((STATE / "agents.json").read_text())


def task(name):
    return next((t for t in index()["tasks"] if t["name"] == name), None)


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
    print(json.dumps({"passed": True, "checks": len(RESULTS), "root": str(ROOT)}))
finally:
    plane("daemon", "stop", check=False)
    subprocess.run([TMUX, "-S", SOCKET, "kill-server"], env=ENV, capture_output=True)
