#!/usr/bin/env python3
"""Check launch and Codex caller guards on an isolated tmux server.

Run after cargo build: python3 tests/caller_live.py. Requires Codex and tmux.
No prompt is submitted. Agent jobs use the branch CLI; raw tmux only manages
isolated infrastructure. Results remain under /tmp/caller-live-*/.
"""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
WORK = Path(tempfile.mkdtemp(prefix="caller-live-", dir="/tmp"))
BIN = WORK / "bin"
SCRATCH = WORK / "scratch"
BIN.mkdir()
SCRATCH.mkdir()
(WORK / "home").mkdir()
(WORK / "codex").mkdir()
SERVER = "plane-caller-" + str(os.getpid())
TMUX = shutil.which("tmux")
assert TMUX and shutil.which("codex"), "tmux and Codex must be installed"

for name, command in {
    "tmux": "exec " + shlex.quote(TMUX) + " -L " + shlex.quote(SERVER) + ' "$@"',
    "osascript": "exit 0",  # Suppress notifications only in the isolated environment.
    "notify-send": "exit 0",
}.items():
    path = BIN / name
    path.write_text("#!/bin/sh\n" + command + "\n")
    path.chmod(0o755)

ENV = os.environ.copy()
ENV.update(PATH=str(BIN) + os.pathsep + ENV["PATH"], XDG_STATE_HOME=str(WORK / "state"),
           HOME=str(WORK / "home"), CODEX_HOME=str(WORK / "codex"))
# Model the real Codex tool-shell identity shape with an intentionally stale pane.
ENV.update(CODEX_THREAD_ID="caller-fixture", TMUX_PANE="%999999")
CLI = str(ROOT / "target/debug/tmux-agent-plane")


def plane(*args, check=True):
    result = subprocess.run([CLI, *args], env=ENV, text=True, capture_output=True, timeout=40)
    if check and result.returncode:
        raise RuntimeError(f"{args}: {result.stderr}")
    return result


subprocess.run([
    TMUX, "-L", SERVER, "-f", "/dev/null", "new-session", "-d", "-s", "harness",
    "-c", str(SCRATCH),
], env=ENV, check=True)
for option in ("default-shell", "default-command"):
    subprocess.run([TMUX, "-L", SERVER, "set-option", "-g", option, "/bin/sh"], env=ENV, check=True)
log = (WORK / "daemon.log").open("w")
daemon = subprocess.Popen([CLI, "daemon", "serve"], env=ENV, stdout=log, stderr=log)
owned = []
try:
    for _ in range(40):
        if (WORK / "state/tmux-agent-plane/agents.json").exists():
            break
        time.sleep(0.1)
    plane("list")
    for task in (None, "", "  "):
        arguments = ["spawn", "agy", "--in", "window", "--cwd", str(SCRATCH)]
        if task is not None:
            arguments.extend(["--", task])
        result = plane(*arguments, check=False)
        assert result.returncode and "agy publishes no state until its first turn; give spawn a task" in result.stderr, result
    assert json.loads(plane("list", "--json").stdout) == [], "launch preflight must create no agent"
    for target in ("self", ENV["TMUX_PANE"]):
        result = plane("rename", target, "never-applied", check=False)
        assert result.returncode and "cannot identify" in result.stderr, result
    result = plane("spawn", "codex", "--in", "pane", "--cwd", str(SCRATCH), check=False)
    assert result.returncode and "use --in window or --in session" in result.stderr, result
    for placement in ("window", "session"):
        name = "caller-" + placement
        print(plane("spawn", "codex", "--in", placement, "--name", name,
                    "--cwd", str(SCRATCH)).stdout.strip(), flush=True)
        owned.append(name)
        rows = json.loads(plane("list", "--json").stdout)
        item = next(agent for agent in rows if agent["name"] == name)
        assert item["parent"] is None and all(not agent["self"] for agent in rows), item
        assert item["kind"] == "codex"
    print("PASS: agy task preflight, list/JSON, self/inherited target guards, pane split guard, window/session "
          "spawns with null parent", flush=True)
finally:
    for name in owned:
        print(plane("kill", name, check=False).stdout.strip(), flush=True)
    daemon.terminate()
    daemon.wait(timeout=5)
    log.close()
    subprocess.run([TMUX, "-L", SERVER, "kill-server"], capture_output=True)
    print("Artifacts:", WORK, flush=True)
