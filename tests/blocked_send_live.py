#!/usr/bin/env python3
"""Private native-session/PTY proof; no agent CLI or model turns.

cargo build && python3 tests/blocked_send_live.py target/debug/tmux-agent-plane
Raw tmux creates only the base shell/server; agent jobs use the branch CLI.
Evidence stays in the printed short /tmp directory.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

binary = Path(sys.argv[1]).resolve()
root = Path(tempfile.mkdtemp(prefix="bs-", dir="/tmp"))
print(f"Evidence: {root}", flush=True)
server = f"bs-{os.getpid()}"
bin_dir = root / "bin"
bin_dir.mkdir()
(root / "home").mkdir()
(root / "claude/sessions").mkdir(parents=True)
(root / "status").write_text("waiting")
real_tmux = shutil.which("tmux")
assert real_tmux, "tmux required"
env = dict(os.environ, HOME=str(root / "home"), XDG_STATE_HOME=str(root / "state"),
           CLAUDE_CONFIG_DIR=str(root / "claude"), GROK_HOME=str(root / "grok"),
           GROK_LEADER_SOCKET=str(root / "no-grok.sock"), PLANE_SEND_ROOT=str(root),
           PATH=str(bin_dir) + os.pathsep + os.environ["PATH"], TERM="xterm-256color")
env.pop("TMUX", None)
wrapper = bin_dir / "tmux"
wrapper.write_text(f"#!{sys.executable}\nimport json, os, sys\n"
                   f"with open({str(root / 'tmux.jsonl')!r}, 'a') as log:\n"
                   "    log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
                   f"os.execv({real_tmux!r}, [{real_tmux!r}, '-L', {server!r}, *sys.argv[1:]])\n")
wrapper.chmod(0o755)
for tool in ("osascript", "notify-send"):
    path = bin_dir / tool
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o755)
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror",
                str(Path(__file__).parent / "fixtures/send/agent.c"),
                "-o", str(bin_dir / "claude")], check=True)


def plane(*args, check=True):
    return subprocess.run([str(binary), *args], env=env, text=True,
                          capture_output=True, check=check, timeout=40)


def wait_for(description, condition, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = condition()
        if result:
            return result
        time.sleep(0.05)
    raise AssertionError(f"timeout: {description}; evidence at {root}")


def agent():
    return next((a for a in json.loads(plane("list", "--json").stdout)
                 if a["name"] == "send-fixture"), None)


def state(value):
    path = root / "status.tmp"
    path.write_text(value)
    path.replace(root / "status")


def received():
    path = root / "received"
    return path.read_bytes() if path.exists() else b""


def mutations():
    return [json.loads(line) for line in (root / "tmux.jsonl").read_text().splitlines()
            if json.loads(line)[0] in ("load-buffer", "paste-buffer", "delete-buffer", "send-keys")]


def rejected(*args, expected):
    before_bytes, before_calls = received(), mutations()
    result = plane(*args, check=False)
    assert result.returncode and expected in result.stderr, result
    time.sleep(0.15)
    assert received() == before_bytes, "refusal sent terminal bytes"
    assert mutations() == before_calls, "refusal made a buffer/key call"
    return result.stderr


def delivered(text):
    # tmux bracketed paste converts LF to CR on the wire, preserving multiline input.
    return b"\x1b[200~" + text.encode().replace(b"\n", b"\r") + b"\x1b[201~\r"


processes = []
spawned = False
daemon_started = False
try:
    subprocess.run([str(wrapper), "-f", "/dev/null", "new-session", "-d", "-s", "scratch", "/bin/sh"], env=env, check=True)
    subprocess.run([str(wrapper), "set-option", "-g", "default-shell", "/bin/sh"], env=env, check=True)
    subprocess.run([str(wrapper), "set-option", "-g", "default-command", "/bin/sh"], env=env, check=True)
    plane("daemon", "start")
    daemon_started = True
    plane("spawn", "claude", "--in", "window", "--cwd", str(root), "--name", "send-fixture")
    spawned = True
    wait_for("native blocked session", lambda: agent()["state"] == "blocked")
    error = rejected("send", "send-fixture", "status update: finished", expected="waiting on a question or permission")
    assert all(value in error for value in ("send-fixture", "read", "keys", "--answer")), error
    (root / "refusal.txt").write_text(error)
    print("PASS blocked status refused before any bytes/buffer/key calls", flush=True)

    text = "First line\nSecond line 'quoted'"
    start = len(received())
    plane("send", "send-fixture", "--answer", text)
    wait_for("multiline answer bytes", lambda: received()[start:] == delivered(text))
    assert agent()["state"] == "blocked"
    print("PASS deliberate multiline answer delivered exactly once", flush=True)

    # The initial blocked state must not settle a reply wait.
    start = len(received())
    reply = subprocess.Popen([str(binary), "send", "send-fixture", "--answer", "reply and wait", "--wait", "--timeout", "10"],
                             env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    processes.append(reply)
    wait_for("reply delivered", lambda: received()[start:] == delivered("reply and wait"))
    time.sleep(1.2)
    assert reply.poll() is None, "wait settled on the original blocked question"
    state("busy")
    wait_for("reply working", lambda: agent()["state"] == "working")
    state("idle")
    stdout, stderr = reply.communicate(timeout=10)
    assert reply.returncode == 0 and "send-fixture done" in stdout, (stdout, stderr)
    print("PASS answer --wait follows the reply turn through completion", flush=True)

    for native, indexed in (("idle", "done"), ("busy", "working"), ("idle", "done")):
        state(native)
        wait_for(indexed, lambda: agent()["state"] == indexed)
        rejected("send", "send-fixture", "--answer", "outdated reply", expected="requires a blocked target")
        start = len(received())
        plane("send", "send-fixture", "normal " + indexed)
        wait_for("normal send", lambda: received()[start:] == delivered("normal " + indexed))
    # Idle requires a fresh session identity with no previously unseen completion.
    plane("kill", "send-fixture")
    spawned = False
    wait_for("fixture removed", lambda: agent() is None)
    state("idle")
    plane("spawn", "claude", "--in", "window", "--cwd", str(root), "--name", "send-fixture")
    spawned = True
    wait_for("idle", lambda: agent()["state"] == "idle")
    rejected("send", "send-fixture", "--answer", "outdated reply", expected="requires a blocked target")
    start = len(received())
    plane("send", "send-fixture", "normal idle")
    wait_for("normal idle send", lambda: received()[start:] == delivered("normal idle"))
    print("PASS normal Working/Idle/Done sends; --answer refuses each without mutation", flush=True)

    start = len(received())
    normal = subprocess.Popen([str(binary), "send", "send-fixture", "normal wait", "--wait", "--timeout", "10"],
                              env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    processes.append(normal)
    wait_for("normal wait delivered", lambda: received()[start:] == delivered("normal wait"))
    assert normal.poll() is None
    state("busy")
    wait_for("normal turn working", lambda: agent()["state"] == "working")
    state("waiting")
    wait_for("blocked again", lambda: agent()["state"] == "blocked")
    stdout, stderr = normal.communicate(timeout=10)
    assert normal.returncode == 0 and "send-fixture blocked" in stdout, (stdout, stderr)
    print("PASS ordinary send --wait follows a new turn until its question", flush=True)

    start = len(received())
    reply = subprocess.Popen([str(binary), "send", "send-fixture", "--answer", "another question", "--wait", "--timeout", "10"],
                             env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    processes.append(reply)
    wait_for("second reply delivered", lambda: received()[start:] == delivered("another question"))
    assert reply.poll() is None
    state("busy")
    wait_for("second reply working", lambda: agent()["state"] == "working")
    state("waiting")
    wait_for("new reply question", lambda: agent()["state"] == "blocked")
    stdout, stderr = reply.communicate(timeout=10)
    assert reply.returncode == 0 and "send-fixture blocked" in stdout, (stdout, stderr)
    print("PASS answer --wait follows a reply until its next question", flush=True)

    began = time.monotonic()
    result = plane("send", "send-fixture", "--answer", "timeout reply", "--wait", "--timeout", "1", check=False)
    assert result.returncode and "timed out after 1s" in result.stderr, result
    assert 1 <= time.monotonic() - began < 4
    print("PASS answer wait timeout while original question remains blocked", flush=True)

    plane("kill", "send-fixture")
    spawned = False
    plane("daemon", "stop")
    daemon_started = False
    time.sleep(5.2)
    for flags in ([], ["--answer"]):
        rejected("send", "send-fixture", *flags, "stale", expected="daemon is not running")
    print("PASS stale index still errors before any buffer/key calls", flush=True)
finally:
    for process in processes:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
    if spawned:
        plane("kill", "send-fixture", check=False)
    if daemon_started:
        plane("daemon", "stop", check=False)
    subprocess.run([real_tmux, "-L", server, "kill-server"], capture_output=True)
    print(f"Artifacts: {root}", flush=True)
