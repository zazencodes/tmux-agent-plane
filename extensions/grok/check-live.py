#!/usr/bin/env python3
"""Real Grok smoke check against a locally built, registered plane binary (macOS)."""
import json
import os
import pty
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
import time

plane = Path(sys.argv[1]).resolve(strict=True)
root = Path(tempfile.mkdtemp(prefix="grok-plane-live-"))
print(f"Evidence: {root}", flush=True)
socket_name = root.name
home = root / "grok"
home.mkdir()
(home / "config.toml").write_text("[cli]\nuse_leader = true\n")
auth = Path(os.environ.get("GROK_HOME", str(Path.home() / ".grok"))) / "auth.json"
(home / "auth.json").symlink_to(auth.resolve(strict=True))
(root / "scratch").mkdir()
(root / "bin").mkdir()
tmux = shutil.which("tmux")
(root / "bin/tmux").write_text(f"#!/bin/sh\nexec {shlex.quote(tmux)} -L {shlex.quote(socket_name)} \"$@\"\n")
(root / "bin/tmux").chmod(0o755)
# Isolated integration must not display desktop notifications.
for command in ("osascript", "notify-send"):
    path = root / "bin" / command
    path.write_text("#!/bin/sh\nexit 0\n")
    path.chmod(0o755)
env = os.environ.copy()
for name in ("TMUX", "TMUX_PANE"):
    env.pop(name, None)
env.update(PATH=f"{root / 'bin'}:{env['PATH']}", XDG_STATE_HOME=str(root / "state"),
           GROK_HOME=str(home), GROK_LEADER_SOCKET=str(home / "leader.sock"),
           GROK_FOLDER_TRUST="0", GROK_DISABLE_AUTOUPDATER="1")
subprocess.run([tmux, "-L", socket_name, "-f", "/dev/null", "new-session", "-d", "-s", "check", "-c", str(root / "scratch")], env=env, check=True)
daemon_log = (root / "daemon.log").open("w")
daemon = subprocess.Popen([str(plane), "daemon"], env=env, stdout=daemon_log, stderr=subprocess.STDOUT)
owned = []

def call(*args, timeout=65, check=True):
    result = subprocess.run([str(plane), *args], env=env, text=True, capture_output=True, timeout=timeout)
    with (root / "commands.log").open("a") as log:
        log.write(f"{shlex.join(args)}\n{result.stdout}{result.stderr}\n")
    if check and result.returncode:
        raise RuntimeError(f"{args}: {result.stderr}")
    return result

def agents():
    if daemon.poll() is not None:
        raise RuntimeError((root / "daemon.log").read_text())
    return json.loads(call("list", "--json").stdout)

def until(predicate, seconds=40):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        rows = agents()
        with (root / "states.jsonl").open("a") as log:
            log.write(json.dumps(rows) + "\n")
        match = predicate(rows)
        if match:
            return match
        time.sleep(.15)
    raise TimeoutError("Expected native state missing; see retained evidence")

def named(rows, name):
    return next((row for row in rows if row["name"] == name), None)

try:
    startup = time.monotonic() + 5
    while not (root / "state/tmux-agent-plane/agents.json").exists():
        if daemon.poll() is not None or time.monotonic() > startup:
            raise RuntimeError((root / "daemon.log").read_text())
        time.sleep(.05)
    until(lambda _: True)
    prompt = "Use ask_user_question to ask me to choose tea or coffee. Wait for my answer."
    owned.append("grok-question")
    result = call("spawn", "grok", prompt, "--in", "window", "--cwd", str(root / "scratch"), "--name", "grok-question", "--wait", "--timeout", "45")
    question = until(lambda rows: (r if (r := named(rows, "grok-question")) and r["state"] == "blocked" else None))
    assert question["detail"] == "input needed"
    call("keys", "grok-question", "1", "Enter")
    until(lambda rows: (r if (r := named(rows, "grok-question")) and r["state"] in ("done", "idle") else None))
    result = call("send", "grok-question", 'Run python3 -c "import time; time.sleep(8); print(456)" in foreground. Do not use background tools.', "--wait", "--timeout", "35")
    assert "blocked" in result.stdout
    call("keys", "grok-question", "3", "Enter")
    until(lambda rows: (r if (r := named(rows, "grok-question")) and r["state"] == "working" else None))
    call("wait", "grok-question", "--timeout", "35")
    done = until(lambda rows: (r if (r := named(rows, "grok-question")) and r["state"] == "done" else None))
    assert done["completions"] > 0 and not done["visible"]
    # Attach an infrastructure-only client inside a private PTY; agent focus uses plane.
    master, slave = pty.openpty()
    client_env = dict(env, TERM="xterm-256color")
    client = subprocess.Popen([tmux, "-L", socket_name, "attach-session", "-t", "check"], stdin=slave, stdout=slave, stderr=slave, env=client_env)
    os.close(slave)
    try:
        time.sleep(.2)
        call("focus", "grok-question")
        until(lambda rows: (r if (r := named(rows, "grok-question")) and r["state"] == "idle" and r["visible"] else None))
    finally:
        client.terminate()
        client.wait(timeout=3)
        os.close(master)
    # A second pane in the exact same directory must publish a different identity.
    owned.append("grok-second")
    call("spawn", "grok", "--in", "window", "--cwd", str(root / "scratch"), "--name", "grok-second", "--wait", "--timeout", "35")
    second = until(lambda rows: named(rows, "grok-second"))
    assert second["pane"] != question["pane"] and second["session_id"] != question["session_id"]
    assert second["cwd"] == question["cwd"]
    # Prove '--help' arrives as a literal interactive task using native ACP history.
    owned.append("grok-option")
    call("spawn", "grok", "--in", "window", "--cwd", str(root / "scratch"), "--name", "grok-option", "--", "--help")
    option = until(lambda rows: named(rows, "grok-option"))
    def literal_prompt(_rows):
        for path in (home / "sessions").glob(f"*/{option['session_id']}/updates.jsonl"):
            for line in path.read_text().splitlines():
                update = json.loads(line).get("params", {}).get("update", {})
                if update.get("sessionUpdate") == "user_message_chunk" and update.get("content", {}).get("text") == "--help":
                    return True
        return False
    until(literal_prompt)
    call("kill", "grok-option")
    owned.remove("grok-option")
    # Exercise send --wait; a fully sampled turn settles, a fast missed turn may time out.
    fast = call("send", "grok-second", "Reply exactly READY without tools.", "--wait", "--timeout", "35", check=False)
    if fast.returncode:
        assert "timed out" in fast.stderr, fast.stderr
        print("Observed sampled fast-turn wait timeout; counter gap remains.")
    else:
        print("No-tool send happened to settle; this does not close the fast-turn gap.")
    call("kill", "grok-second")
    owned.remove("grok-second")
    until(lambda rows: not named(rows, "grok-second"))
    print("PASS: question, permission, working, unseen done -> viewed idle, same-cwd identity, option-like task, idle spawn, exit.")
    print("GAP: sampled fast turns can be missed by send/spawn --wait; no counter/epoch is published.")
finally:
    clean = True
    for name in owned:
        result = call("kill", name, check=False)
        clean = clean and result.returncode == 0
    if not clean:
        print(f"An agent could not be closed through plane; server {socket_name} and daemon retained for inspection. Logs: {root}", flush=True)
        sys.exit(1)
    daemon.terminate()
    daemon.wait(timeout=5)
    # Infrastructure only: all test agent panes were closed through plane first.
    subprocess.run([tmux, "-L", socket_name, "kill-server"], check=False)
    # Native backend infrastructure is isolated to this dedicated GROK_HOME.
    subprocess.run(["grok", "leader", "kill"], env=env, check=True, stdout=daemon_log, stderr=subprocess.STDOUT)
    daemon_log.close()
    print(f"Evidence retained: {root}")
