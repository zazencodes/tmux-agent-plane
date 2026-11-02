#!/usr/bin/env python3
"""Prove daemon lifecycle never leaves a tmux job or pane view overlay.

Run after cargo build: python3 tests/daemon_lifecycle_live.py.
Uses an isolated server, state directory, notification stubs and private PTY
client. No agents, user settings, installed binary or user daemon are touched.
Retains structured assertions and daemon.log under /tmp/plane-lifecycle-*/.
"""
import concurrent.futures
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import shlex
import shutil
import signal
import socket
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "target/debug/tmux-agent-plane"
TMUX = shutil.which("tmux")
assert TMUX and CLI.exists(), "tmux and cargo build required"
# Unix socket paths must fit macOS's sockaddr_un; keep the scratch root short.
WORK = Path(tempfile.mkdtemp(prefix="plane-lifecycle-", dir="/tmp"))
BIN = WORK / "bin"
BIN.mkdir()
SERVER = "plane-lifecycle-" + str(os.getpid())
for name, command in {
    "tmux": "exec " + shlex.quote(TMUX) + " -L " + shlex.quote(SERVER) + ' "$@"',
    "osascript": "exit 0",
    "notify-send": "exit 0",
}.items():
    path = BIN / name
    path.write_text("#!/bin/sh\n" + command + "\n")
    path.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(CLI)
ENV = os.environ.copy()
ENV.update(PATH=str(BIN) + os.pathsep + ENV["PATH"],
           XDG_STATE_HOME=str(WORK / "state"), TERM="xterm-256color")
STATE = WORK / "state/tmux-agent-plane"
LOG = STATE / "daemon.log"
SOCKET = STATE / "daemon.sock"
RESULTS = []


def tmux(*args, check=True):
    return subprocess.run([TMUX, "-L", SERVER, *args], env=ENV, text=True,
                          capture_output=True, check=check, timeout=20)


def plane(*args, check=True):
    return subprocess.run([str(CLI), *args], env=ENV, text=True,
                          capture_output=True, check=check, timeout=20)


def wait_for(predicate, description):
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.05)
    raise AssertionError("timed out: " + description)


def pid():
    return int(re.findall(r"daemon started \(pid (\d+)\)", LOG.read_text())[-1])


def clean(label):
    # These are structural tmux facts, never screen-derived agent state.
    mode = tmux("display-message", "-p", "-t", "harness:0.0",
                "#{pane_in_mode}|#{pane_mode}").stdout.strip()
    jobs = tmux("show-messages", "-J").stdout.strip()
    panes = tmux("list-panes", "-a", "-F", "#{pane_id}").stdout.splitlines()
    assert mode == "0|", (label, mode)
    assert not jobs, (label, jobs)
    assert len(panes) == 1, (label, panes)
    RESULTS.append(dict(case=label, mode=mode, jobs=jobs, panes=panes))
    print("PASS:", label, flush=True)


def plugin():
    tmux("run-shell", "-t", "harness:0.0", shlex.quote(str(ROOT / "tmux-agent-plane.tmux")))


def job(action):
    tmux("run-shell", "-b", "-t", "harness:0.0", "tmux-agent-plane daemon " + action)
    wait_for(lambda: not tmux("show-messages", "-J").stdout.strip(), action + " job exit")


client = None
master = None
try:
    # Load the plugin while tmux is still reading its initial config, before
    # new-session has created a target. Loading it into an existing server
    # alone misses this startup ordering bug.
    config = WORK / "tmux.conf"
    config.write_text("run-shell " + shlex.quote(str(ROOT / "tmux-agent-plane.tmux")) + "\n")
    tmux("-f", str(config), "new-session", "-d", "-s", "harness", "-c", str(WORK))
    wait_for(lambda: plane("list", "--json", check=False).returncode == 0,
             "plugin cold start")
    wait_for(lambda: not tmux("show-messages", "-J").stdout.strip(),
             "cold start job exit")
    assert "[fatal]" not in LOG.read_text(), LOG.read_text()
    cold_pid = pid()
    clean("plugin cold start before first session")
    master, slave = pty.openpty()
    client = subprocess.Popen([TMUX, "-L", SERVER, "attach-session", "-t", "harness"],
                              env=ENV, stdin=slave, stdout=slave, stderr=slave,
                              start_new_session=True)
    os.close(slave)

    def drain():
        with (WORK / "client.pty").open("wb") as output:
            try:
                while True:
                    chunk = os.read(master, 65536)
                    if not chunk:
                        break
                    output.write(chunk)
            except OSError:
                pass

    threading.Thread(target=drain, daemon=True).start()
    wait_for(lambda: tmux("list-clients", "-F", "#{client_name}").stdout.strip(), "private attached client")
    # Calibrate the oracle: the original run-shell failure really does enter
    # view mode on this tmux/client combination. This scratch pane has no agent.
    tmux("run-shell", "-b", "-t", "harness:0.0", "exit 7")
    wait_for(lambda: tmux("display-message", "-p", "-t", "harness:0.0",
                         "#{pane_in_mode}").stdout.strip() == "1", "negative control view mode")
    control_mode = tmux("display-message", "-p", "-t", "harness:0.0",
                        "#{pane_in_mode}|#{pane_mode}").stdout.strip()
    assert control_mode == "1|view-mode", control_mode
    RESULTS.append(dict(case="negative control: failing raw run-shell", mode=control_mode))
    tmux("send-keys", "-t", "harness:0.0", "q")
    wait_for(lambda: tmux("display-message", "-p", "-t", "harness:0.0",
                         "#{pane_in_mode}").stdout.strip() == "0", "dismiss negative control")
    plugin()
    first = pid()
    assert first == cold_pid
    clean("plugin start")
    # Child is detached from the plugin job, has no controlling terminal and
    # redirects all output to daemon.log (setsid makes pgid equal its pid).
    pgid = int(subprocess.check_output(["ps", "-o", "pgid=", "-p", str(first)], text=True))
    assert pgid == first, (first, pgid)
    plugin()
    assert pid() == first
    clean("plugin duplicate start")
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        list(pool.map(lambda _: plane("daemon", "start"), range(4)))
    assert pid() == first
    clean("concurrent duplicate starts")
    job("restart")
    second = pid()
    assert second != first
    clean("targeted graceful restart from tmux job")
    assert "graceful stop requested" in LOG.read_text()
    job("stop")
    assert not SOCKET.exists()
    clean("normal stop from tmux job")
    plugin()
    os.kill(pid(), signal.SIGTERM)
    wait_for(lambda: not SOCKET.exists(), "SIGTERM cleanup")
    wait_for(lambda: "daemon stopped after signal 15" in LOG.read_text(), "SIGTERM reason")
    clean("SIGTERM after plugin start")
    plugin()
    # Force a real fatal index write while the detached worker runs.
    index = STATE / "agents.json"
    index.unlink()
    index.mkdir()
    wait_for(lambda: not SOCKET.exists(), "fatal index write exit")
    assert "[fatal]" in LOG.read_text()
    status = tmux("show-option", "-gqv", "@agent_plane_status").stdout
    assert "daemon stopped" in status, status
    clean("nonzero fatal exit after plugin start")
    index.rmdir()
    # Startup failures must remain loud in log/status without making the plugin
    # itself a failing tmux run-shell job.
    index.write_text("invalid JSON")
    plugin()
    wait_for(lambda: not SOCKET.exists(), "startup failure cleanup")
    assert "parsing" in LOG.read_text() and "during startup" in LOG.read_text()
    clean("plugin startup failure")
    index.unlink()
    plane("daemon", "stop")
    clean("already stopped")
    # An occupied index lock without a control socket cannot be targeted safely.
    with (STATE / "daemon.lock").open("w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        result = plane("daemon", "restart", check=False)
        assert result.returncode and "cannot stop daemon safely" in result.stderr, result
        result = plane("daemon", "start", check=False)
        assert result.returncode and "unmanaged daemon" in result.stderr, result
    clean("unmanaged index owner rejected safely")
    plugin()
    assert pid() not in (first, second)
    with socket.socket(socket.AF_UNIX) as control:
        control.connect(str(SOCKET))
        control.sendall(b"invalid")
        control.shutdown(socket.SHUT_WR)
        assert control.recv(32) == b""
    wait_for(lambda: not SOCKET.exists(), "control failure shutdown")
    assert "unknown daemon control request" in LOG.read_text()
    clean("control failure logged and stopped")
    plugin()
    job("stop")
    clean("recovery after fatal failure")
    assert LOG.read_text().count("[fatal]") == 3, LOG.read_text()
    (BIN / "tmux-agent-plane").unlink()
    tmux("run-shell", "-t", "harness:0.0",
         "PATH=" + shlex.quote(str(BIN) + ":/usr/bin:/bin") + " " +
         shlex.quote(str(ROOT / "tmux-agent-plane.tmux")))
    clean("plugin missing-binary error")
    (BIN / "tmux-agent-plane").symlink_to(CLI)
    (WORK / "assertions.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
finally:
    plane("daemon", "stop", check=False)
    tmux("kill-server", check=False)
    if client:
        client.wait(timeout=5)
    if master is not None:
        os.close(master)
    print("Artifacts:", WORK, flush=True)
