#!/usr/bin/env python3
"""Real Telescope + CLI proof on a private server; no daemon or model calls.

  cargo build --locked
  python3 extensions/nvim/tests/test_picker.py \
    --telescope ~/.local/share/nvim/lazy/telescope.nvim \
    --plenary ~/.local/share/nvim/lazy/plenary.nvim

For Alex's config, add --dotfiles /path/to/dotfiles --ui-select
~/.local/share/nvim/lazy/telescope-ui-select.nvim.

The index contains synthetic agents in inert shell panes. Neovim runs inside
the originating pane with two attached PTY clients. Every CLI/tmux call stays
on the private server and private state root. Artifacts remain in /tmp/tap-nvim-*.
"""
import argparse
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
import threading
import time

SOURCE = Path(__file__).resolve()
REPO = SOURCE.parents[3]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=Path, default=REPO / "target/debug/tmux-agent-plane")
parser.add_argument("--telescope", type=Path, required=True)
parser.add_argument("--plenary", type=Path, required=True)
parser.add_argument("--dotfiles", type=Path, help="also validate Alex's Telescope wiring")
parser.add_argument("--ui-select", type=Path, help="installed telescope-ui-select.nvim (with --dotfiles)")
args = parser.parse_args()
for path in (args.binary, args.telescope, args.plenary):
    assert path.exists(), path
TMUX, NVIM = shutil.which("tmux"), shutil.which("nvim")
assert TMUX and NVIM, "tmux and Neovim 0.10+ are required"
ROOT = Path(tempfile.mkdtemp(prefix="tap-nvim-", dir="/tmp"))
BIN = ROOT / "bin"
BIN.mkdir()
STATE = ROOT / "state/tmux-agent-plane"
STATE.mkdir(parents=True)
ENV = dict(os.environ, HOME=str(ROOT / "home"), XDG_STATE_HOME=str(ROOT / "state"),
           XDG_CONFIG_HOME=str(ROOT / "config"), XDG_DATA_HOME=str(ROOT / "data"),
           XDG_CACHE_HOME=str(ROOT / "cache"), PATH=str(BIN) + os.pathsep + os.environ["PATH"],
           TERM="xterm-256color", PICKER_TEST_ROOT=str(ROOT), PICKER_EXTENSION=str(SOURCE.parents[1]),
           PICKER_TELESCOPE=str(args.telescope.resolve()), PICKER_PLENARY=str(args.plenary.resolve()))
ENV.pop("TMUX", None)
# Preserve Codex identity guards; all target panes differ from the Neovim pane.
ENV.pop("TMUX_PANE", None)
if args.dotfiles:
    assert args.ui_select and args.ui_select.is_dir(), "--dotfiles requires --ui-select"
    ENV.update(PICKER_DOTFILES=str(args.dotfiles.resolve()), PICKER_UI_SELECT=str(args.ui_select.resolve()))
(ROOT / "home").mkdir()

def executable(name, source):
    path = BIN / name
    path.write_text(f"#!{sys.executable}\n" + source)
    path.chmod(0o755)

executable("tmux", f"import os, sys\nos.execv({TMUX!r}, [{TMUX!r}, '-S', {str(ROOT / 't.sock')!r}, *sys.argv[1:]])\n")
executable("tmux-agent-plane", f"""import json, os, sys
from pathlib import Path
root = Path({str(ROOT)!r})
mode = (root / 'mode').read_text().strip()
if sys.argv[1] == 'focus':
    with (root / 'focus.calls').open('a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')
    if mode == 'focus-failure':
        print('injected focus failure', file=sys.stderr); sys.exit(9)
if sys.argv[1] == 'list':
    if mode == 'malformed': print('broken json'); sys.exit(0)
    if mode == 'object': print('{{}}'); sys.exit(0)
    if mode == 'failure': print('injected list failure', file=sys.stderr); sys.exit(8)
os.execv({str(args.binary.resolve())!r}, [{str(args.binary.resolve())!r}, *sys.argv[1:]])
""")

def tmux(*arguments):
    return subprocess.run([str(BIN / "tmux"), *arguments], env=ENV, check=True,
                          text=True, capture_output=True, timeout=5).stdout.strip()

def wait_for(predicate, label):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        if (ROOT / "failure").exists():
            raise AssertionError((ROOT / "failure").read_text())
        if predicate():
            return
        time.sleep(0.02)
    raise AssertionError("timed out: " + label)

clients = []
stop = threading.Event()

def attach(session):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
    process = subprocess.Popen([str(BIN / "tmux"), "attach-session", "-t", session],
                               env=ENV, stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    clients.append((process, master))
    def drain():
        while not stop.is_set():
            if select.select([master], [], [], 0.05)[0]:
                try:
                    os.read(master, 65536)
                except OSError:
                    return
    threading.Thread(target=drain, daemon=True).start()
    return process

try:
    for session in ("origin", "other", "destination"):
        tmux("-f", "/dev/null", "new-session", "-d", "-s", session, "/bin/sh")
    window = tmux("display-message", "-p", "-t", "destination", "#{window_id}")
    first = tmux("display-message", "-p", "-t", "destination", "#{pane_id}")
    second = tmux("split-window", "-d", "-P", "-F", "#{pane_id}", "-t", "destination", "/bin/sh")
    rows = []
    for name, title, pane, detail in (
        ("cedar-review", "Audit payments", first, None),
        ("quartz-build", "Ship compiler", second, "Approve cache refresh"),
    ):
        rows.append(dict(name=name, title=title, pane=pane, detail=detail, kind="codex",
                         state="blocked" if detail else "idle", location="destination:0.0",
                         session="destination", window=window, cwd=str(ROOT), parent=None,
                         session_id=None, state_since=0, completions=0, visible=False))
    (ROOT / "mode").write_text("normal")
    (ROOT / "ready").write_text("")
    def refresh_index():
        while not stop.is_set():
            mode = (ROOT / "mode").read_text().strip()
            value = dict(updated_at=0 if mode == "stale" else int(time.time() * 1000),
                         agents=[] if mode == "empty" else [dict(row) for row in rows])
            if mode == "bad-pane":
                value["agents"][0]["pane"] = "mutable-name"
            temp = STATE / "next.json"
            temp.write_text(json.dumps(value))
            temp.replace(STATE / "agents.json")
            (ROOT / "ready").write_text(mode)
            stop.wait(0.02)
    threading.Thread(target=refresh_index, daemon=True).start()
    origin_client = attach("origin")
    other_client = attach("other")
    wait_for(lambda: len(tmux("list-clients").splitlines()) == 2, "two clients attached")
    command = " ".join(shlex.quote(part) for part in [NVIM, "--headless", "-u", "NONE", "-l", str(SOURCE.with_name("picker.lua"))])
    # These are inert shell panes, established before publishing the synthetic index.
    tmux("respawn-pane", "-k", "-t", "origin:0.0", command + " >" + shlex.quote(str(ROOT / "nvim.log")) + " 2>&1; sleep 30")
    wait_for(lambda: (ROOT / "before-focus").exists(), "Telescope checks")
    before = tmux("list-clients", "-F", "#{client_pid} #{session_name}")
    assert f"{origin_client.pid} origin" in before and f"{other_client.pid} other" in before, before
    # Make the unrelated client most recently active before Neovim forks focus.
    os.write(clients[1][1], b"\x0c")
    time.sleep(0.1)
    (ROOT / "allow-focus").touch()
    wait_for(lambda: (ROOT / "done").exists(), "Enter focus")
    after = tmux("list-clients", "-F", "#{client_pid} #{session_name}")
    selected_pane = (ROOT / "selected-pane").read_text().strip()
    calls = [json.loads(line) for line in (ROOT / "focus.calls").read_text().splitlines()]
    assert calls == [["focus", second], ["focus", selected_pane]] and selected_pane == second, calls
    assert tmux("display-message", "-p", "-t", "destination", "#{pane_id}") == second
    routing_ok = f"{origin_client.pid} destination" in after and f"{other_client.pid} other" in after
    report = dict(origin_client=origin_client.pid, other_client=other_client.pid,
                  before=before.splitlines(), after=after.splitlines(), selected_pane=selected_pane,
                  focus_calls=calls, routing_ok=routing_ok)
    (ROOT / "routing.json").write_text(json.dumps(report, indent=2))
    print((ROOT / "checks").read_text().strip())
    if (ROOT / "config-checks").exists():
        print((ROOT / "config-checks").read_text().strip())
    print(json.dumps(report, indent=2))
    assert routing_ok, "CLI focus switched the wrong client; see routing.json (Rust unchanged)"
    print("PASS: Neovim subprocess focus routes the exact pane to its originating client with a second client present")
finally:
    stop.set()
    subprocess.run([str(BIN / "tmux"), "kill-server"], env=ENV, capture_output=True)
    for process, master in clients:
        process.wait(timeout=5)
        os.close(master)
    print("Artifacts:", ROOT)
