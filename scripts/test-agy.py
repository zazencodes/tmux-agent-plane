#!/usr/bin/env python3
"""Exercise a merged agy adapter without touching the installed daemon or binary.

Run from an already trusted repository:
  python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane

Requires authenticated agy 1.4.12+, tmux, lsof, and Python 3 on macOS/Linux.
All agent jobs use the supplied plane CLI. Raw tmux only creates the isolated
server and its invisible terminal client. A temporary workspace hook forces a
real permission request even when the user's settings allow all commands.
"""
import argparse
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import subprocess
import sys
import threading
import time
import uuid


def permission_hook():
    event = json.load(sys.stdin)
    assert event.get("toolCall", {}).get("name") == "run_command", "unexpected hook tool"
    print(json.dumps({"decision": "force_ask", "reason": "tmux-agent-plane integration probe"}))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    cwd = Path.cwd()
    tmux = shutil.which("tmux")
    assert tmux, "tmux is required"
    assert shutil.which("agy"), "agy is required"
    assert shutil.which("lsof"), "lsof is required for agy tracking"
    hook_path = cwd / ".agents/hooks.json"
    if hook_path.exists():
        raise RuntimeError(f"Existing {hook_path}; run in a trusted test checkout without workspace hooks")
    token = uuid.uuid4().hex[:12]
    run_dir = cwd / "target" / f"agy-live-{token}"
    run_dir.mkdir(parents=True)
    stub_dir = run_dir / "bin"
    stub_dir.mkdir()
    for name in ("osascript", "notify-send"):
        stub = stub_dir / name
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o755)
    env = dict(os.environ)
    env["PATH"] = str(stub_dir) + os.pathsep + env["PATH"]
    env["XDG_STATE_HOME"] = str(run_dir / "state")
    env["TERM"] = "xterm-256color"
    daemon = client = None
    master = None
    started = []
    closed = set()
    stop = threading.Event()
    results = {"checks": [], "limits": [
        "No native durable completion counter: turns completed between daemon ticks can evade send/spawn --wait.",
        "This run validates the host OS only; run again on Linux to validate /proc argv and Linux lsof."
    ]}
    server = f"plane-agy-{token}"
    server_args = [tmux, "-L", server, "-f", "/dev/null"]
    hook_dir_existed = hook_path.parent.exists()

    def plane(*arguments, timeout=60, sampled_label=None):
        command = [str(binary), *arguments]
        result = subprocess.run(command, env=env, text=True, capture_output=True, timeout=timeout)
        if result.returncode:
            if sampled_label and re.match(r"^Error: timed out after \d+s; ", result.stderr):
                results[sampled_label] = {"observed": False, "stderr": result.stderr.strip()}
                results["limits"].append(f"{sampled_label}: {result.stderr.strip()}")
                print(f"GAP {sampled_label}: {result.stderr.strip()}", flush=True)
                return result.stdout
            raise RuntimeError(f"{arguments}: {result.stderr.strip()}\nDaemon log:\n{(run_dir / 'daemon.log').read_text()}")
        if sampled_label:
            results[sampled_label] = {"observed": True, "stdout": result.stdout.strip()}
            results["checks"].append(sampled_label)
            print(f"PASS {sampled_label}", flush=True)
        return result.stdout

    def agents():
        return json.loads(plane("list", "--json"))

    def until(label, predicate, timeout=45):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            last = agents()
            if predicate(last):
                results["checks"].append(label)
                print(f"PASS {label}", flush=True)
                return last
            if daemon.poll() is not None:
                raise RuntimeError((run_dir / "daemon.log").read_text())
            stop.wait(0.2)
        raise RuntimeError(f"Timed out: {label}; last index={last}")

    def named(rows, name):
        return next((item for item in rows if item["name"] == name), None)

    def state(name, expected):
        return until(f"{name}: {expected}", lambda rows: named(rows, name) and named(rows, name)["state"] == expected)

    try:
        subprocess.run([*server_args, "new-session", "-d", "-s", "research", "-c", str(cwd), "/bin/sh"], check=True)
        subprocess.run([*server_args, "set-option", "-g", "default-shell", "/bin/sh"], check=True)
        socket, pid, pane = subprocess.check_output([*server_args, "display-message", "-p", "#{socket_path}\t#{pid}\t#{pane_id}"], text=True).strip().split("\t")
        env["TMUX"] = f"{socket},{pid},0"
        env["TMUX_PANE"] = pane
        (run_dir / "environment.json").write_text(json.dumps({key: env[key] for key in ("TMUX", "TMUX_PANE", "XDG_STATE_HOME", "PATH")}))
        with open(run_dir / "daemon.log", "w") as log:
            daemon = subprocess.Popen([str(binary), "daemon"], env=env, stdout=log, stderr=log)
        deadline = time.monotonic() + 10
        index_path = run_dir / "state/tmux-agent-plane/agents.json"
        while not index_path.exists() and time.monotonic() < deadline:
            stop.wait(0.1)
        assert not agents(), "isolated server must start without agents"

        # A PTY client lets plane focus mark done seen without opening any user UI.
        master, slave = pty.openpty()
        client = subprocess.Popen([tmux, "-S", socket, "attach-session", "-t", "research"], stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True)
        os.close(slave)

        def drain():
            while not stop.is_set():
                try:
                    if select.select([master], [], [], 0.2)[0]:
                        os.read(master, 65536)
                except OSError:
                    return
        threading.Thread(target=drain, daemon=True).start()

        hook_path.parent.mkdir(exist_ok=True)
        command = f"{shlex.quote(sys.executable)} {shlex.quote(str(Path(__file__).resolve()))} --permission-hook"
        hook_path.write_text(json.dumps({"plane-integration-probe": {"PreToolUse": [{"matcher": "run_command", "hooks": [{"type": "command", "command": command, "timeout": 5}]}]}}, indent=2) + "\n")

        started.append("agy-live-a")
        output = plane("spawn", "agy", "Call ask_question exactly once, with exactly one single-select question and options Alpha and Beta, then wait. After my answer reply CHOSEN. Do not use any further tools or questions.", "--in", "window", "--cwd", str(cwd), "--name", started[0], "--wait", "--timeout", "40")
        results["question_spawn_wait"] = output.strip()
        row = named(state(started[0], "blocked"), started[0])
        assert row["detail"] == "question", row
        # Native WAITING can precede installation of the TUI input handler.
        # This pause only settles key delivery; native index data still supplies state.
        stop.wait(1.0)
        plane("keys", started[0], "Enter")
        state(started[0], "done")
        plane("focus", started[0])
        state(started[0], "idle")
        # Focus B after it starts so A can finish unseen in the hidden client.
        started.append("agy-live-b")
        plane("spawn", "agy", "Reply with exactly READY. Do not use tools.", "--in", "window", "--cwd", str(cwd), "--name", started[1], "--wait", "--timeout", "40", sampled_label="fast_first_spawn_wait")
        rows = agents()
        a, b = (named(rows, name) for name in started)
        assert a and b and a["cwd"] == b["cwd"]
        assert a["session_id"] and b["session_id"] and a["session_id"] != b["session_id"]
        results["identities"] = [{key: row[key] for key in ("name", "pane", "session_id", "cwd")} for row in (a, b)]
        results["checks"].append("two panes in one cwd have distinct native sessions")
        plane("focus", started[1])
        output = plane("send", started[0], "Run exactly sleep 12 using run_command, then reply FINISHED. Do not modify files or run other commands.", "--wait", "--timeout", "40")
        results["permission_send_wait"] = output.strip()
        row = named(state(started[0], "blocked"), started[0])
        assert row["detail"] == "permission required", row
        # Keep the request pending beyond initial state publication and SQLite checkpoints.
        stop.wait(3)
        state(started[0], "blocked")
        plane("keys", started[0], "Enter")
        state(started[0], "working")
        state(started[0], "done")
        plane("focus", started[0])
        state(started[0], "idle")
        plane("send", started[1], "Reply with exactly FAST_OK. Do not use tools.", "--wait", "--timeout", "40", sampled_label="fast_send_wait")
        plane("kill", started[1])
        until("killed agent disappears", lambda rows: named(rows, started[1]) is None)
        closed.add(started[1])
        plane("send", started[0], "/exit")
        until("native exit disappears", lambda rows: named(rows, started[0]) is None)
        closed.add(started[0])
        print("PASS native agy integration", flush=True)
    except Exception as error:
        results["error"] = str(error)
        raise
    finally:
        # Close only this harness's agents through plane, before infrastructure cleanup.
        safe_to_close_server = not started
        if daemon and daemon.poll() is None:
            try:
                rows = agents()
                for name in started:
                    if named(rows, name):
                        plane("kill", name)
                        closed.add(name)
                safe_to_close_server = all(name in closed for name in started)
            except Exception as error:
                results["cleanup_error"] = str(error)
        stop.set()
        if client:
            client.terminate()
            client.wait(timeout=5)
        if master is not None:
            os.close(master)
        if safe_to_close_server:
            if daemon and daemon.poll() is None:
                daemon.terminate()
                daemon.wait(timeout=5)
            subprocess.run([*server_args, "kill-server"], check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            results["cleanup_error"] = results.get("cleanup_error", "Unindexed agent may still be running; retained isolated server for plane-based cleanup")
            print(f"Retained isolated server {server}; environment in {run_dir / 'environment.json'}", flush=True)
        if hook_path.exists():
            hook_path.unlink()
        if not hook_dir_existed and hook_path.parent.exists():
            hook_path.parent.rmdir()
        (run_dir / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"Results: {run_dir / 'results.json'}", flush=True)


if __name__ == "__main__":
    if sys.argv[1:] == ["--permission-hook"]:
        permission_hook()
    else:
        main()
