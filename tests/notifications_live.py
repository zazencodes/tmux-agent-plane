#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Research tmux notification geometry and real client input on a private PTY.

Run: uv run tests/notifications_live.py
Only a unique socket and scratch state are used. No agents or daemon are run.
Terminal snapshots are UI assertions against this fixture, never agent detection.
Raw terminal output, snapshots, input logs and results remain in target/notify-live-*.
"""

import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
import tty


def fixture(log_path, label):
    tty.setraw(0)
    with open(log_path, "ab", buffering=0) as log:
        count = 0
        while True:
            count += 1
            os.write(1, f"{label}-{count:06d}\r\n".encode())
            ready, _, _ = select.select([0], [], [], 0.1)
            if ready:
                data = os.read(0, 4096)
                if not data:
                    break
                log.write(data)
                if data == b"\x06":
                    os.write(1, b"\x1b[?1004h")  # Request terminal focus events.


if len(sys.argv) > 1 and sys.argv[1] == "--fixture":
    fixture(sys.argv[2], sys.argv[3])
    raise SystemExit

import pyte  # Provided by the pinned uv script dependency.

ROOT = Path(__file__).resolve().parents[1]
(ROOT / "target").mkdir(exist_ok=True)
WORK = Path(tempfile.mkdtemp(prefix="notify-live-", dir=ROOT / "target"))
TMUX = shutil.which("tmux")
assert TMUX, "tmux must be installed"
# macOS Unix sockets have a short path limit; the worktree path exceeds it.
SOCKET_DIR = tempfile.TemporaryDirectory(prefix="tap-notify-", dir="/tmp")
SOCKET = str(Path(SOCKET_DIR.name) / "server.sock")
ENV = os.environ.copy()
ENV.pop("TMUX", None)
ENV.pop("TMUX_PANE", None)
ENV.update(TERM="xterm-256color", XDG_STATE_HOME=str(WORK / "state"))
BIN = WORK / "bin"
BIN.mkdir()
for name in ("osascript", "notify-send"):
    path = BIN / name
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o755)
ENV["PATH"] = str(BIN) + os.pathsep + ENV["PATH"]


class PrivateScreen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        # pyte 0.8.2 lacks the private=True argument for tmux's DEC query.
        # No emulated-terminal replies are needed for these UI assertions.
        pass


SCREEN = PrivateScreen(80, 24)
STREAM = pyte.ByteStream(SCREEN)
RESULTS = {"tmux": subprocess.check_output([TMUX, "-V"], text=True).strip()}
CLIENT_PID = None
MASTER = None
RAW = (WORK / "client.raw").open("wb")


def tmux(*args):
    result = subprocess.run(
        [TMUX, "-S", SOCKET, *args], env=ENV, text=True, stderr=subprocess.PIPE,
        stdout=subprocess.PIPE, timeout=5,
    )
    if result.returncode:
        raise RuntimeError(f"tmux {args}: {result.stderr}")
    return result.stdout.strip()


def drain(duration=0.2):
    deadline = time.monotonic() + duration
    while time.monotonic() < deadline:
        ready, _, _ = select.select([MASTER], [], [], max(0, deadline - time.monotonic()))
        if ready:
            data = os.read(MASTER, 65536)
            if not data:
                raise RuntimeError("private tmux client exited")
            RAW.write(data)
            RAW.flush()
            STREAM.feed(data)


def snapshot(name):
    lines = SCREEN.display
    (WORK / (name + ".txt")).write_text("\n".join(lines) + "\n")
    return lines


def rows(marker):
    return [n + 1 for n, line in enumerate(SCREEN.display) if marker in line]


def received(path):
    return path.read_bytes() if path.exists() else b""


def press(key):
    # Write to the actual private client terminal; send-keys bypasses overlays.
    os.write(MASTER, key)
    drain()


def command(label, path):
    return shlex.join([sys.executable, str(Path(__file__).resolve()), "--fixture", str(path), label])


def message(marker, *flags):
    tmux("display-message", "-c", CLIENT, "-d", "5000", *flags, marker)
    drain()
    snapshot(marker)
    return rows(marker)


def active():
    return tmux("display-message", "-p", "-t", "harness:0", "#{pane_id}")


try:
    pane_log = WORK / "pane-input.bin"
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "harness", "-x", "80", "-y", "24",
         command("PANE", pane_log))
    tmux("set-option", "-g", "status-position", "top")
    tmux("set-option", "-g", "status-format[0]", "TOP-STATUS")
    CLIENT_PID, MASTER = pty.fork()
    if CLIENT_PID == 0:
        fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", 24, 80, 0, 0))
        os.execve(TMUX, [TMUX, "-S", SOCKET, "attach-session", "-t", "harness"], ENV)
    drain(0.5)
    CLIENT = tmux("list-clients", "-F", "#{client_name}")
    assert CLIENT and "\n" not in CLIENT
    pane = active()
    assert rows("TOP-STATUS") == [1]
    snapshot("baseline")
    press(b"a")
    assert received(pane_log) == b"a"

    assert message("MSG-TOP") == [1]
    before = snapshot("default-freeze-before")
    drain(0.4)
    assert SCREEN.display == before, "default display-message should freeze pane redraw"
    press(b"b")
    assert received(pane_log) == b"ab" and not rows("MSG-TOP")
    RESULTS["default_message"] = {"row": 1, "pane_redraw_frozen": True,
                                  "dismissal_key_reaches_pane": True}

    assert message("MSG-C", "-C") == [1]
    before = SCREEN.display.copy()
    drain(0.4)
    assert SCREEN.display != before, "-C should allow pane redraw"
    press(b"c")
    assert received(pane_log) == b"abc" and not rows("MSG-C")
    RESULTS["C_message"] = {"row": 1, "pane_redraw_continues": True,
                            "dismissal_key_reaches_pane": True}

    assert message("MSG-NC", "-N", "-C") == [1]
    press(b"d")
    assert received(pane_log) == b"abc" and rows("MSG-NC") == [1]
    RESULTS["NC_message"] = {"row": 1, "key_discarded": True}
    message("CLEAR")
    press(b"e")
    assert received(pane_log) == b"abce"

    tmux("set-option", "-g", "message-line", "4")
    assert message("MSG-CLAMP-ONE") == [1]
    press(b"f")
    tmux("set-option", "-g", "status", "2")
    tmux("set-option", "-g", "status-format[1]", "SECOND-STATUS")
    assert message("MSG-CLAMP-TWO") == [2]
    press(b"g")
    assert rows("TOP-STATUS") == [1]
    RESULTS["message_line_4"] = {"status_1_row": 1, "status_2_row": 2}
    tmux("set-option", "-g", "status", "on")
    tmux("set-option", "-g", "message-line", "0")
    tmux("set-option", "-g", "status-position", "bottom")
    assert message("MSG-BOTTOM") == [24]
    press(b"h")
    assert rows("TOP-STATUS") == [24]
    RESULTS["bottom_status"] = {"message_row": 24, "status_row": 24}
    tmux("set-option", "-g", "status-position", "top")
    tmux("set-option", "-g", "status", "off")
    assert message("MSG-STATUS-OFF") == [1]
    press(b"i")
    RESULTS["status_off_position_top"] = {"message_row": 1}
    tmux("set-option", "-g", "status", "on")

    popup_log = WORK / "popup-input.bin"
    before_input = received(pane_log)
    popup = subprocess.Popen(
        [TMUX, "-S", SOCKET, "display-popup", "-c", CLIENT, "-B", "-E", "-w", "35",
         "-h", "3", "-x", "0", "-y", "24", command("POPUP", popup_log)],
        env=ENV, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    drain(0.4)
    assert rows("POPUP") and rows("TOP-STATUS") == [1]
    snapshot("bottom-popup")
    press(b"p")
    assert received(popup_log) == b"p" and received(pane_log) == before_input
    assert active() == pane, "unchanged pane_active does not prove non-modal input"
    RESULTS["bottom_popup"] = {"rows": rows("POPUP"), "key_reaches_popup": True,
                               "key_reaches_pane": False, "active_pane_unchanged": True}
    tmux("display-popup", "-c", CLIENT, "-C")
    popup.communicate(timeout=5)
    # Closing a running popup terminates its job; this is not a successful job exit.
    RESULTS["bottom_popup"]["forced_close_exit_code"] = popup.returncode
    drain()

    before_input = received(pane_log)
    menu = subprocess.Popen(
        [TMUX, "-S", SOCKET, "display-menu", "-c", CLIENT, "-x", "0", "-y", "24",
         "Notice", "m", "set-option -g @research-menu chosen"],
        env=ENV, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    drain()
    assert rows("Notice")
    snapshot("bottom-menu")
    press(b"m")
    menu.communicate(timeout=5)
    assert menu.returncode == 0 and tmux("show-option", "-gv", "@research-menu") == "chosen"
    assert received(pane_log) == before_input and active() == pane
    RESULTS["bottom_menu"] = {"key_chooses_menu_item": True, "key_reaches_pane": False}

    height_before = int(tmux("display-message", "-p", "-t", pane, "#{pane_height}"))
    tmux("set-window-option", "-t", "harness:0", "pane-border-format", "BORDER-NOTICE")
    tmux("set-window-option", "-t", "harness:0", "pane-border-status", "bottom")
    drain()
    assert rows("BORDER-NOTICE") == [24] and rows("TOP-STATUS") == [1]
    height_after = int(tmux("display-message", "-p", "-t", pane, "#{pane_height}"))
    assert height_after == height_before - 1
    before_input = received(pane_log)
    press(b"z")
    assert received(pane_log) == before_input + b"z" and active() == pane
    snapshot("bottom-pane-border")
    RESULTS["bottom_pane_border"] = {"row": 24, "top_status_row": 1,
                                    "pane_height_before": height_before,
                                    "pane_height_after": height_after, "key_reaches_pane": True}

    tmux("set-option", "-s", "focus-events", "on")
    press(b"\x06")
    press(b"\x1b[I")  # This private terminal explicitly reports itself focused.
    before_input = received(pane_log)
    popup = subprocess.Popen(
        [TMUX, "-S", SOCKET, "display-popup", "-c", CLIENT, "-B", "-E", "-w", "35",
         "-h", "3", "-x", "0", "-y", "24", command("FOCUS", popup_log)],
        env=ENV, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
    )
    drain()
    assert received(pane_log) == before_input + b"\x1b[O"
    assert active() == pane
    tmux("display-popup", "-c", CLIENT, "-C")
    popup.communicate(timeout=5)
    drain()
    assert received(pane_log) == before_input + b"\x1b[O\x1b[I"
    RESULTS["popup_focus_events"] = {"pane_focus_out_on_open": True,
                                     "pane_focus_in_on_close": True,
                                     "active_pane_unchanged": True}
    RESULTS["passed"] = True
finally:
    (WORK / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
    subprocess.run([TMUX, "-S", SOCKET, "kill-server"], env=ENV, capture_output=True, timeout=5)
    if MASTER is not None:
        os.close(MASTER)
    if CLIENT_PID:
        try:
            # A client with unconsumed terminal output can defer graceful exit.
            os.kill(CLIENT_PID, signal.SIGKILL)
        except ProcessLookupError:
            pass
        os.waitpid(CLIENT_PID, 0)
    RAW.close()
    SOCKET_DIR.cleanup()
    print(json.dumps(RESULTS, indent=2))
    print("Artifacts:", WORK)
