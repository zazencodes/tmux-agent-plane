#!/usr/bin/env python3
"""Prove the README sync recipe and its scope/reattach limits on a private server."""
import fcntl
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import tempfile
import termios
import time

root = Path(tempfile.mkdtemp(prefix="tap-sync-", dir="/tmp"))
env = dict(os.environ, HOME=str(root), XDG_STATE_HOME=str(root), TERM="xterm-256color")
env.pop("TMUX", None)
base = ["tmux", "-S", str(root / "sock"), "-f", "/dev/null"]
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 88, 157, 0, 0))
client = None


def tmux(*args):
    return subprocess.check_output(base + list(args), env=env).decode().strip()


def drain():
    data = b""
    until = time.monotonic() + 0.3
    while time.monotonic() < until:
        if select.select([master], [], [], 0.02)[0]:
            data += os.read(master, 1000000)
    return data


def attach():
    process = subprocess.Popen(base + ["attach", "-t", "s"], stdin=slave,
                               stdout=slave, stderr=slave, env=env)
    drain()
    return process


def features():
    return tmux("list-clients", "-F", "#{client_termfeatures}").split(",")


try:
    tmux("new-session", "-d", "-s", "s", "sleep 600")
    tmux("new-window", "-t", "s:1", "sleep 600")
    client = attach()
    # Emulate the terminal's own XTVERSION response, without a visible terminal.
    os.write(master, b"\x1bP>|ghostty 1.3.1\x1b\\")
    drain()
    assert tmux("list-clients", "-F", "#{client_termtype}") == "ghostty 1.3.1"
    assert "sync" not in features()
    tmux("set", "-as", "terminal-features", ",ghostty*:sync")
    tmux("refresh-client")
    drain()
    assert "sync" not in features()
    tmux("detach-client")
    client.wait(timeout=5)
    client = attach()
    assert "sync" not in features(), "feature patterns must match TERM, not XTVERSION"
    before = tmux("show-options", "-sv", "terminal-features")
    recipe = "set -as terminal-features ',xterm-256color:sync'"
    assert recipe in (Path(__file__).resolve().parents[1] / "README.md").read_text()
    tmux("set", "-as", "terminal-features", ",xterm-256color:sync")
    after = tmux("show-options", "-sv", "terminal-features")
    assert before in after, "existing feature rules must survive"
    tmux("refresh-client")
    drain()
    assert "sync" not in features()
    tmux("detach-client")
    client.wait(timeout=5)
    client = attach()
    assert "sync" in features()
    tmux("select-window", "-t", "s:0")
    output = drain()
    assert output.count(b"\x1b[?2026h") == output.count(b"\x1b[?2026l") > 0
    print("PASS: README recipe, append preservation, TERM scope, reattach, synchronized redraw")
finally:
    subprocess.run(base + ["kill-server"], env=env, capture_output=True)
    if client is not None:
        client.wait(timeout=5)
    os.close(master)
    os.close(slave)
    shutil.rmtree(root)
