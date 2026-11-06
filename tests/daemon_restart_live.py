#!/usr/bin/env python3
"""Prove Enter restarts a dead daemon from the sidebar and the picker popup.

Run after cargo build: python3 tests/daemon_restart_live.py.
Uses an isolated server and state directory, and signals only daemons it started.
A failed run keeps its files under /tmp/plane-restart-*/.
"""
import os
from pathlib import Path
import pty
import re
import shlex
import shutil
import signal
import subprocess
import tempfile
import threading
import time

ROOT = Path(__file__).resolve().parents[1]
CLI = ROOT / "target/debug/tmux-agent-plane"
TMUX = shutil.which("tmux")
assert TMUX and CLI.exists(), "tmux and cargo build required"
WORK = Path(tempfile.mkdtemp(prefix="plane-restart-", dir="/tmp"))
BIN = WORK / "bin"
BIN.mkdir()
SERVER = "plane-restart-" + str(os.getpid())
wrapper = BIN / "tmux"
wrapper.write_text(f'#!/bin/sh\nexec {shlex.quote(TMUX)} -L {shlex.quote(SERVER)} -f /dev/null "$@"\n')
wrapper.chmod(0o755)
ENV = dict(os.environ, PATH=f"{BIN}{os.pathsep}{os.environ['PATH']}",
           XDG_STATE_HOME=str(WORK / "state"), TERM="xterm-256color")
ENV.pop("TMUX", None)
ENV.pop("TMUX_PANE", None)
LOG = WORK / "state/tmux-agent-plane/daemon.log"


def tmux(*args):
    return subprocess.run([str(wrapper), *args], env=ENV, check=True, text=True,
                          capture_output=True, timeout=10).stdout.strip()


def wait(what, check, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if check():
            return
        time.sleep(0.1)
    raise AssertionError(f"timed out waiting for {what}")


def daemon_pid():
    return int(re.findall(r"daemon started \(pid (\d+)\)", LOG.read_text())[-1])


def running(pid):
    state = subprocess.run(["ps", "-o", "stat=", "-p", str(pid)], text=True,
                           capture_output=True).stdout.strip()
    return bool(state) and not state.startswith("Z")


def kill_daemon():
    pid = daemon_pid()
    os.kill(pid, signal.SIGKILL)
    wait("daemon death", lambda: not running(pid))
    return pid


def expect_new_daemon(old):
    wait("a new daemon", lambda: daemon_pid() != old and running(daemon_pid()))
    return daemon_pid()


def sidebar_case():
    """Enter in a sidebar pane; the new daemon outlives that pane."""
    old = kill_daemon()
    pane = tmux("split-window", "-d", "-P", "-F", "#{pane_id}", "-t", "main", f"exec {shlex.quote(str(CLI))} sidebar")
    screen = lambda: tmux("capture-pane", "-p", "-t", pane)
    wait("the daemon-down message", lambda: "enter restarts" in screen())
    tmux("send-keys", "-t", pane, "Enter")
    new = expect_new_daemon(old)
    wait("the message to clear", lambda: "not running" not in screen() and "restart" not in screen())
    tmux("kill-pane", "-t", pane)
    time.sleep(1.5)
    assert running(new), "daemon died with the sidebar that restarted it"


def drain(master):
    """Keep the client's terminal output moving."""
    try:
        while os.read(master, 65536):
            pass
    except OSError:
        pass


def picker_case():
    """Enter in a real popup, reached through the client's terminal."""
    old = kill_daemon()
    master, slave = pty.openpty()
    client = subprocess.Popen([str(wrapper), "attach-session", "-t", "main"], env=ENV,
                              stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
    os.close(slave)
    threading.Thread(target=drain, args=(master,), daemon=True).start()
    try:
        wait("the client", lambda: tmux("list-clients", "-F", "#{client_name}") != "")
        name = tmux("list-clients", "-F", "#{client_name}")
        command = f"exec {shlex.quote(str(CLI))} ui --client {shlex.quote(name)}"
        # display-popup returns only when the popup closes.
        popup = subprocess.Popen([str(wrapper), "display-popup", "-b", "rounded", "-c", name, "-E", command],
                                 env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        # The index reads as current for five seconds after the daemon's last write.
        time.sleep(6)
        os.write(master, b"\r")
        new = expect_new_daemon(old)
        time.sleep(0.5)
        os.write(master, b"q")
        popup.wait(timeout=10)
        time.sleep(1.5)
        assert running(new), "daemon died with the popup that restarted it"
    finally:
        client.kill()
        client.wait()
        os.close(master)


def main():
    tmux("new-session", "-d", "-s", "main", "-x", "140", "-y", "40")
    subprocess.run([str(CLI), "daemon", "start"], env=ENV, check=True, timeout=20)
    sidebar_case()
    picker_case()
    print("ok: Enter restarts the daemon from the sidebar and the picker")


try:
    main()
    tmux("kill-server")
    shutil.rmtree(WORK)
except BaseException:
    print(f"kept {WORK}")
    subprocess.run([str(wrapper), "kill-server"], env=ENV)
    raise
