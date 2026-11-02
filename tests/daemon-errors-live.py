#!/usr/bin/env python3
"""Run against a built binary. Entirely private server/state/PTY, native fixture publishers.

python3 tests/daemon-errors-live.py target/debug/tmux-agent-plane [--serve]
--serve selects the lifecycle integration's internal foreground verb.
"""
import argparse
import json
import os
from pathlib import Path
import pty
import shlex
import shutil
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("binary", type=Path)
parser.add_argument("--serve", action="store_true")
args = parser.parse_args()
binary = args.binary.resolve()
real_tmux = shutil.which("tmux")
assert real_tmux, "tmux required"
root = Path(tempfile.mkdtemp(prefix="plane-daemon-errors-", dir="/tmp"))
print(f"Evidence: {root}", flush=True)
server = f"plane-errors-{os.getpid()}"
bin_dir = root / "bin"
bin_dir.mkdir()
state = root / "state" / "tmux-agent-plane"
(state / "pi").mkdir(parents=True)
claude = root / "claude"
(claude / "sessions").mkdir(parents=True)
env = dict(os.environ, XDG_STATE_HOME=str(root / "state"),
           CLAUDE_CONFIG_DIR=str(claude), GROK_HOME=str(root / "grok"),
           GROK_LEADER_SOCKET=str(root / "no-grok.sock"), PLANE_FIXTURE_ROOT=str(root),
           PATH=str(bin_dir) + os.pathsep + os.environ["PATH"], TERM="xterm-256color")
# Codex identity remains intact. Every agent operation uses explicit names/window placement.
env.pop("TMUX", None)
for tool in ("osascript", "notify-send"):
    path = bin_dir / tool
    path.write_text("#!/bin/sh\nif [ -e " + shlex.quote(str(root / "desktop.fail")) +
                    " ]; then echo 'injected desktop failure' >&2; exit 23; fi\n"
                    "echo delivered >> " + shlex.quote(str(root / "desktop.delivered")) + "\n")
    path.chmod(0o755)
wrapper = bin_dir / "tmux"
wrapper.write_text(
    "#!/bin/sh\n"
    f'if [ "$1" = refresh-client ] && [ -e {shlex.quote(str(root / "refresh.fail"))} ]; then\n'
    "  echo 'injected refresh-client failure' >&2; exit 29\nfi\n"
    f"exec {shlex.quote(real_tmux)} -L {shlex.quote(server)} \"$@\"\n"
)
wrapper.chmod(0o755)
fixture = root / "agent"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(Path(__file__).parent / "fixtures/daemon/agent.c"), "-o", str(fixture)], check=True)
for kind in ("claude", "pi"):
    (bin_dir / kind).symlink_to(fixture)
(root / "claude.command").write_text("busy 0")
(root / "pi.command").write_text("working 0")


def tmux(*command, check=True):
    return subprocess.run([str(wrapper), *command], env=env, text=True, capture_output=True, check=check).stdout.strip()


def plane(*command, check=True):
    result = subprocess.run([str(binary), *command], env=env, text=True, capture_output=True, check=check)
    if result.stdout:
        print(result.stdout.strip(), flush=True)
    return result


def wait_for(description, condition, timeout=15):
    until = time.monotonic() + timeout
    while time.monotonic() < until:
        try:
            result = condition()
            if result:
                return result
        except (FileNotFoundError, json.JSONDecodeError):
            pass
        time.sleep(0.1)
    raise AssertionError(f"timeout: {description}; evidence at {root}")


def snapshot():
    return json.loads((state / "agents.json").read_text())


def agents():
    return {a["name"]: a for a in snapshot()["agents"]}


def log():
    return (state / "daemon.log").read_text()


def status():
    return tmux("show-option", "-gqv", "@agent_plane_status")


def change(kind, value):
    path = root / f"{kind}.command"
    temp = path.with_suffix(".tmp")
    temp.write_text(value)
    temp.replace(path)


def start_daemon():
    stderr = open(root / f"stderr-{len(daemons)}.log", "w")
    process = subprocess.Popen([str(binary), "daemon", *(["serve"] if args.serve else [])], env=env, stdout=stderr, stderr=stderr)
    daemons.append(process)
    return process


def drain(fd):
    try:
        while os.read(fd, 65536):
            pass
    except OSError:
        pass


daemons = []
client = None
master = slave = None
try:
    # Only the scratch base shell is created with raw tmux; fixtures use plane spawn/kill.
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "scratch", "/bin/sh")
    tmux("set-option", "-g", "default-shell", "/bin/sh")
    tmux("set-option", "-g", "default-command", "/bin/sh")
    master, slave = pty.openpty()
    client = subprocess.Popen([str(wrapper), "attach-session", "-t", "scratch"], env=env, stdin=slave, stdout=slave, stderr=slave)
    threading.Thread(target=drain, args=(master,), daemon=True).start()
    wait_for("private client", lambda: tmux("list-clients", "-F", "#{client_name}"))
    (root / "refresh.fail").touch()
    daemon = start_daemon()
    wait_for("initial index", lambda: (state / "agents.json").exists())
    for kind in ("claude", "pi"):
        plane("spawn", kind, "--in", "window", "--cwd", str(root), "--name", f"fixture-{kind}")
    wait_for("both working", lambda: len(agents()) == 2 and all(a["state"] == "working" for a in agents().values()))

    wait_for("refresh failure visible", lambda: "presentation:refresh:" in status())
    assert "injected refresh-client failure" in log()
    assert daemon.poll() is None
    refresh_fault_tick = snapshot()["updated_at"]
    time.sleep(2.2)
    assert snapshot()["updated_at"] > refresh_fault_tick
    assert len(agents()) == 2
    assert log().count("] ERROR: tmux refresh-client") == 1
    (root / "refresh.fail").unlink()
    wait_for("refresh recovery", lambda: "presentation:refresh:" not in status() and
             any("[presentation:refresh:" in line and "] recovered" in line for line in log().splitlines()))
    assert daemon.poll() is None
    assert "[fatal]" not in log()
    print("PASS refresh-client failure remains degraded without stopping either agent/index; retry recovers", flush=True)

    change("claude", "unknown-fixture-state 0")
    wait_for("kind isolation", lambda: set(agents()) == {"fixture-pi"} and "scan:claude" in status())
    assert daemon.poll() is None
    first = snapshot()["updated_at"]
    time.sleep(2.2)
    assert snapshot()["updated_at"] > first
    assert log().count("[scan:claude] ERROR:") == 1
    assert "unknown-fixture-state" in log()
    change("pi", "unknown-fixture-state 0")
    wait_for("all kinds faulted still visible", lambda: not agents() and "scan:claude" in status() and "scan:pi" in status())
    assert daemon.poll() is None
    change("pi", "working 0")
    wait_for("Pi recovery while Claude remains faulted", lambda: set(agents()) == {"fixture-pi"} and "scan:pi" not in status())
    change("claude", "busy 0")
    wait_for("scan recovery", lambda: len(agents()) == 2 and "[scan:claude] recovered" in log() and "scan:claude" not in status())
    print("PASS native kind fault isolation, fresh index, deduplication and recovery", flush=True)

    (root / "desktop.fail").touch()
    change("pi", "idle 5")
    wait_for("desktop fault", lambda: agents()["fixture-pi"]["completions"] == 5 and "notification:desktop" in status())
    change("claude", "idle 0")
    wait_for("other completion during notification failure", lambda: agents()["fixture-claude"]["completions"] == 1)
    private_client = tmux("list-clients", "-F", "#{client_name}").splitlines()[0]
    messages = tmux("show-messages", "-t", private_client)
    assert "fixture-pi finished" in messages and "fixture-claude finished" in messages, messages
    assert daemon.poll() is None
    assert "injected desktop failure" in log()
    assert log().count("[notification:desktop] ERROR:") == 1
    (root / "desktop.fail").unlink()
    wait_for("notification recovery", lambda: "[notification:desktop] recovered" in log() and "notification:desktop" not in status())
    assert (root / "desktop.delivered").read_text().count("delivered") == 2
    print("PASS failed desktop delivery keeps both agents/index/tmux notifications alive; only failed deliveries retried", flush=True)

    change("pi", "idle 3")
    wait_for("counter regression", lambda: "fixture-pi" not in agents() and "advance:pi:" in status())
    time.sleep(2.2)
    assert "fixture-pi" not in agents()
    assert set(agents()) == {"fixture-claude"}
    change("pi", "idle 5")
    wait_for("counter recovery", lambda: "fixture-pi" in agents() and "advance:pi:" not in status())
    assert agents()["fixture-pi"]["completions"] == 5
    assert agents()["fixture-pi"]["state"] == "done"
    assert (root / "desktop.delivered").read_text().count("delivered") == 2
    print("PASS per-observation regression stays faulted across ticks; unseen state/counter preserved on recovery", flush=True)

    # Native fatal filesystem error; index writer must exit and persist the whole chain.
    (state / "agents.json.tmp").mkdir()
    wait_for("fatal daemon exit", lambda: daemon.poll() is not None)
    assert daemon.returncode != 0
    assert "[fatal] writing agent index:" in log()
    assert "daemon stopped" in status()
    assert str(state / "daemon.log") in (root / "stderr-0.log").read_text()
    print("PASS fatal index error persisted, visible stopped status, stderr log path", flush=True)
    (state / "agents.json.tmp").rmdir()

    # A broken logger is fatal too; stderr must explain inability to persist and name the log.
    (state / "daemon.log").rename(state / "saved-daemon.log")
    (state / "daemon.log").mkdir()
    broken_log_daemon = start_daemon()
    wait_for("log failure exits", lambda: broken_log_daemon.poll() is not None)
    assert broken_log_daemon.returncode != 0
    stderr = (root / "stderr-1.log").read_text()
    assert "could not persist reason" in stderr and str(state / "daemon.log") in stderr
    print("PASS log-write failure exits with full reason/path on stderr", flush=True)
    (state / "daemon.log").rmdir()
    (state / "saved-daemon.log").rename(state / "daemon.log")
    daemon = start_daemon()
    wait_for("restart index fresh", lambda: snapshot()["updated_at"] > first and len(agents()) == 2 and "daemon stopped" not in status())
    for kind in ("claude", "pi"):
        plane("kill", f"fixture-{kind}")
    (root / "messages.log").write_text(messages)
    print("All daemon diagnostics live checks passed; fixtures cleaned up.", flush=True)
finally:
    for process in daemons:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
    if client and client.poll() is None:
        client.terminate()
        client.wait(timeout=5)
    tmux("kill-server", check=False)
    for fd in (slave, master):
        if fd is not None:
            os.close(fd)
