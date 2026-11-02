#!/usr/bin/env python3
"""Run real Pi UI/agent lifecycle tests with a local provider and isolated tmux server.

After building a binary with Pi registered: python3 tests/pi_live.py
Requires Pi 1.0.2+, tmux and Python 3.9+. No provider credentials or model usage.
All agents are started, read, sent input, focused and closed through the branch CLI.
"""
import json
import os
from pathlib import Path
import pty
import shutil
import signal
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
BASE = Path(tempfile.mkdtemp(prefix="pi-live-", dir="/tmp"))
CLI = ROOT / "target/debug/tmux-agent-plane"
PI = shutil.which("pi")
TMUX = shutil.which("tmux")
assert PI and TMUX, "Pi and tmux must be installed"
ENV = os.environ.copy()
ENV.pop("TMUX_PANE", None)
SOCKET = BASE / "tmux.sock"
ENV.update(PI_CODING_AGENT_DIR=str(BASE / "config"), XDG_STATE_HOME=str(BASE / "state"),
           HOME=str(BASE / "home"), PI_OFFLINE="1", TMUX=str(SOCKET) + ",0,0")
DAEMON = None
AGENTS = set()
RESULTS = []


def shell_quote(value):
    return "'" + str(value).replace("'", "'\\''") + "'"


def infrastructure(*args):
    return subprocess.run([TMUX, "-S", str(SOCKET), *args], env=ENV,
                          capture_output=True, text=True, check=True)


def plane(*args):
    result = subprocess.run([str(CLI), *map(str, args)], env=ENV,
                            capture_output=True, text=True, timeout=40)
    RESULTS.append({"args": args, "stdout": result.stdout, "stderr": result.stderr,
                    "returncode": result.returncode})
    assert result.returncode == 0, (args, result.stderr)
    return result.stdout


def agents():
    return json.loads(plane("list", "--json"))


def agent(name):
    return next(a for a in agents() if a["name"] == name)


def eventually(predicate):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.1)
    raise AssertionError("integration condition did not become true")


def start_daemon():
    global DAEMON
    with (BASE / "daemon.log").open("a") as log:
        DAEMON = subprocess.Popen([str(CLI), "daemon", "serve"], env=ENV, stdout=log, stderr=log)
    eventually(lambda: (BASE / "state/tmux-agent-plane/agents.json").exists())
    time.sleep(1.1)
    assert DAEMON.poll() is None, (BASE / "daemon.log").read_text()


def spawn(name, task=None):
    AGENTS.add(name)
    args = ["spawn", "pi", "--in", "window", "--cwd", str(BASE / "scratch"), "--name", name]
    if task:
        args += ["--wait", "--timeout", "15", "--", task]
    plane(*args)


try:
    for directory in ["config/extensions", "state", "scratch", "bin", "home"]:
        (BASE / directory).mkdir(parents=True)
    (BASE / "config/extensions/tmux-agent-plane").symlink_to(ROOT / "extensions/pi")
    (BASE / "config/extensions/local-provider.ts").symlink_to(ROOT / "tests/fixtures/pi/live-provider.ts")
    (BASE / "config/settings.json").write_text(json.dumps({"defaultProvider": "plane-fixture",
        "defaultModel": "local", "defaultThinkingLevel": "off", "quietStartup": True,
        "packages": [], "extensions": []}))
    wrappers = {
        "pi": "exec " + shell_quote(PI) + ' --offline --no-context-files --no-skills --no-prompt-templates --no-approve "$@"\n',
        "tmux": "exec " + shell_quote(TMUX) + " -S " + shell_quote(SOCKET) + ' "$@"\n',
        "osascript": "exit 0\n",  # Never display test desktop notifications.
        "notify-send": "exit 0\n",
    }
    for name, contents in wrappers.items():
        path = BASE / "bin" / name
        path.write_text("#!/bin/sh\n" + contents)
        path.chmod(0o755)
    ENV["PATH"] = str(BASE / "bin") + os.pathsep + ENV["PATH"]
    infrastructure("-f", "/dev/null", "new-session", "-d", "-s", "pi-fixture", "-c", str(BASE / "scratch"), "/bin/sh")
    infrastructure("set-option", "-g", "default-shell", "/bin/sh")
    infrastructure("set-option", "-g", "default-command", "/bin/sh")
    start_daemon()
    spawn("pi-one")
    assert agent("pi-one")["state"] == "idle"
    spawn("pi-two", "slow first turn")
    assert agent("pi-two")["state"] == "done"
    plane("send", "pi-one", "slow next turn")
    eventually(lambda: agent("pi-one")["state"] == "working")
    plane("wait", "pi-one", "--timeout", "15")
    for command, detail in [("plane-confirm", "Allow fixture command?"), ("plane-question", "Fixture question")]:
        plane("send", "pi-one", "/" + command, "--wait", "--timeout", "10")
        assert agent("pi-one")["state"] == "blocked"
        assert agent("pi-one")["detail"] == detail
        plane("keys", "pi-one", "Escape")
        eventually(lambda: agent("pi-one")["state"] == "idle")
    spawn("pi-fast", "fast first turn")
    plane("send", "pi-fast", "fast second turn", "--wait", "--timeout", "10")
    before = agent("pi-fast")
    native_id = before["session_id"]
    epoch = before["completion_epoch"]
    source_count = before["reported_completions"]
    original_file = next((BASE / "config/sessions").rglob("*" + native_id + ".jsonl"))
    plane("send", "pi-fast", "/reload")
    time.sleep(1.5)
    assert agent("pi-fast")["reported_completions"] == source_count
    plane("send", "pi-fast", "/new")
    eventually(lambda: agent("pi-fast")["session_id"] != native_id)
    assert agent("pi-fast")["reported_completions"] == source_count
    plane("send", "pi-fast", "fast new session", "--wait", "--timeout", "10")
    plane("send", "pi-fast", "/plane-resume " + str(original_file))
    eventually(lambda: agent("pi-fast")["session_id"] == native_id)
    revisited = agent("pi-fast")
    assert revisited["completion_epoch"] == epoch
    assert revisited["reported_completions"] == source_count + 1
    assert revisited["completions"] == before["completions"] + 1
    plane("send", "pi-fast", "fast revisited session", "--wait", "--timeout", "10")
    assert agent("pi-fast")["completions"] == before["completions"] + 2
    # Private client attaches only to the isolated infrastructure session.
    child, fd = pty.fork()
    if child == 0:
        os.environ.update(ENV)
        os.environ["TERM"] = "xterm-256color"
        os.execl(TMUX, "tmux", "-S", str(SOCKET), "attach-session", "-t", "pi-fixture")
    def drain():
        try:
            while os.read(fd, 65536):
                pass
        except OSError:
            pass
    threading.Thread(target=drain, daemon=True).start()
    time.sleep(0.5)
    plane("focus", "pi-fast")
    eventually(lambda: agent("pi-fast")["visible"] and agent("pi-fast")["state"] == "idle")
    os.kill(child, signal.SIGTERM)  # Stop the private tmux client infrastructure.
    os.waitpid(child, 0)
    os.close(fd)
    plane("send", "pi-fast", "/quit")
    eventually(lambda: all(a["name"] != "pi-fast" for a in agents()))
    AGENTS.remove("pi-fast")
    plane("send", "pi-one", "/plane-freeze")
    # Stale publishers are withdrawn, while the daemon/index survive and recover.
    eventually(lambda: not agents())
    assert DAEMON.poll() is None
    diagnostic_log = BASE / "state/tmux-agent-plane/daemon.log"
    assert "stale or future live Pi status" in diagnostic_log.read_text()
    eventually(lambda: len(agents()) == len(AGENTS))
    assert DAEMON.poll() is None
    assert "[scan:pi] recovered" in diagnostic_log.read_text()
    print("PASS: identity, working/blocked/idle/done, same cwd, waits, fast turns, reload/new/resume, visibility, exit, stale withdrawal/recovery")
finally:
    if DAEMON and DAEMON.poll() is not None:
        time.sleep(2)
        start_daemon()
    for name in sorted(AGENTS):
        plane("kill", name)
    if DAEMON and DAEMON.poll() is None:
        DAEMON.terminate()
        DAEMON.wait(timeout=5)
    infrastructure("kill-server")
    (BASE / "results.json").write_text(json.dumps(RESULTS, indent=2))
    print("Retained isolated test results:", BASE)
