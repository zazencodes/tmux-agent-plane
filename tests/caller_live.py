#!/usr/bin/env python3
"""Check launch and Codex caller guards, and parent links, on an isolated tmux server.

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
# A person at a plain shell: no agent caller, so no parent.
PERSON = {k: v for k, v in ENV.items() if k not in ("CODEX_THREAD_ID", "TMUX_PANE")}


def plane(*args, check=True, env=ENV):
    result = subprocess.run([CLI, *args], env=env, text=True, capture_output=True, timeout=40)
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
    print(plane("spawn", "codex", "--in", "window", "--name", "lead", "--cwd", str(SCRATCH),
                env=PERSON).stdout.strip(), flush=True)
    owned.append("lead")
    rows = json.loads(plane("list", "--json").stdout)
    lead = next(agent for agent in rows if agent["name"] == "lead")
    assert lead["parent"] is None and not lead["parent_ended"], lead
    for parent, message in ((None, "must name itself as the parent"), ("nobody", "no agent has that name")):
        extra = ["--parent", parent] if parent else []
        result = plane("spawn", "codex", "--in", "window", "--cwd", str(SCRATCH), *extra, check=False)
        assert result.returncode and message in result.stderr, result
    for placement in ("window", "session"):
        name = "caller-" + placement
        print(plane("spawn", "codex", "--in", placement, "--name", name, "--parent", "lead",
                    "--cwd", str(SCRATCH)).stdout.strip(), flush=True)
        owned.append(name)
        rows = json.loads(plane("list", "--json").stdout)
        item = next(agent for agent in rows if agent["name"] == name)
        assert item["parent"] == lead["pane"] and all(not agent["self"] for agent in rows), item
        assert item["kind"] == "codex"

    def tmux(*args):
        return subprocess.run([TMUX, "-L", SERVER, *args], env=ENV, check=True, text=True,
                              capture_output=True).stdout.strip()

    def codex_pid(pane):
        shell = int(tmux("display-message", "-p", "-t", pane, "#{pane_pid}"))
        table = subprocess.run(["ps", "-A", "-o", "pid=,ppid=,comm="], capture_output=True,
                               text=True, check=True).stdout.split("\n")
        rows = [line.split(None, 2) for line in table if line.strip()]
        children = {}
        for pid, ppid, comm in rows:
            children.setdefault(int(ppid), []).append((int(pid), comm))
        pending = [shell]
        while pending:
            for pid, comm in children.get(pending.pop(), []):
                if os.path.basename(comm) == "codex":
                    return pid
                pending.append(pid)
        raise AssertionError(f"no codex under {pane}")

    def row(name=None, pane=None, deadline=15):
        end = time.time() + deadline
        while True:
            for agent in json.loads(plane("list", "--json").stdout):
                if agent["name"] == name or agent["pane"] == pane:
                    return agent
            if time.time() >= end:
                return None
            time.sleep(0.3)

    def until(check, what, deadline=15):
        end = time.time() + deadline
        while time.time() < end:
            if check():
                return
            time.sleep(0.3)
        raise AssertionError(what)

    child = row("caller-window")
    option = tmux("show-options", "-pqv", "-t", child["pane"], "@agent-plane-parent")
    assert option == f"{lead['pane']} {codex_pid(lead['pane'])} {codex_pid(child['pane'])}", option

    # The child exits and someone starts Codex by hand in the same pane: no inherited parent.
    os.kill(codex_pid(child["pane"]), 9)
    until(lambda: row(pane=child["pane"], deadline=0) is None, "the exited child stays listed")
    until(lambda: tmux("capture-pane", "-p", "-t", child["pane"]).rstrip().endswith("$"),
          "the login shell did not come back after the agent exited")
    tmux("send-keys", "-t", child["pane"], "codex", "Enter")
    until(lambda: row(pane=child["pane"], deadline=0) is not None, "hand-started Codex was not detected", 30)
    again = row(pane=child["pane"])
    assert again["parent"] is None and not again["parent_ended"], again
    until(lambda: tmux("show-options", "-pqv", "-t", child["pane"], "@agent-plane-parent") == "",
          "the stale link was not cleared")
    owned.remove("caller-window")
    owned.append(again["name"])

    # The parent exits: its remaining child reports the ended parent.
    os.kill(codex_pid(lead["pane"]), 9)
    until(lambda: row("caller-session", deadline=0)["parent_ended"], "the ended parent is not reported")
    assert row("caller-session")["parent"] is None
    assert "ended" in plane("list").stdout
    print("PASS: agy task preflight, list/JSON, self/inherited target guards, pane split guard, Codex --parent "
          "required and recorded, links bound to processes, pane reuse clears the link, ended parent", flush=True)
finally:
    for name in owned:
        print(plane("kill", name, check=False).stdout.strip(), flush=True)
    daemon.terminate()
    daemon.wait(timeout=5)
    log.close()
    subprocess.run([TMUX, "-L", SERVER, "kill-server"], capture_output=True)
    print("Artifacts:", WORK, flush=True)
