#!/usr/bin/env python3
"""Exercise a merged agy adapter without touching the installed daemon or binary.

Run from an already trusted repository:
  python3 scripts/test-agy.py --binary target/debug/tmux-agent-plane

Requires authenticated agy 1.4.12+, tmux, lsof, and Python 3 on macOS/Linux.
Both modes use a short private HOME/state root containing copies of the native
auth/onboarding files. Add --folder-trust to use a never-trusted workspace.
All agent jobs use the supplied plane CLI. Raw tmux only creates the isolated
server and its invisible terminal client. A temporary workspace hook forces a
real permission request even when the user's settings allow all commands.
"""
import argparse
import atexit
import hashlib
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
import tempfile
import threading
import time
import uuid


def permission_hook():
    event = json.load(sys.stdin)
    assert event.get("toolCall", {}).get("name") == "run_command", "unexpected hook tool"
    print(json.dumps({"decision": "force_ask", "reason": "tmux-agent-plane integration probe"}))


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--folder-trust", action="store_true")
    parser.add_argument("--session-trust-overrides", action="store_true", help="Probe conservative untracking of explicit --settings trustedWorkspaces overlays")
    args = parser.parse_args()
    binary = args.binary.resolve()
    cwd = Path.cwd()
    tmux = shutil.which("tmux")
    assert tmux, "tmux is required"
    real_agy = shutil.which("agy")
    assert real_agy, "agy is required"
    assert shutil.which("lsof"), "lsof is required for agy tracking"
    token = uuid.uuid4().hex[:12]
    run_dir = cwd / "target" / f"agy-live-{token}"
    run_dir.mkdir(parents=True)
    # macOS sockaddr_un paths are limited to 104 bytes. Keep runtime roots short
    # for the lifecycle daemon socket while retaining target-based artifact links.
    runtime_root = Path(tempfile.mkdtemp(prefix="tap-agy-", dir="/tmp")).resolve()
    original_home = Path.home()
    if args.folder_trust:
        cwd = run_dir / "never-trusted"
        cwd.mkdir()
        subprocess.run(["git", "init", "-q", str(cwd)], check=True)
    private_home = runtime_root / "h"
    private_data = private_home / ".gemini/antigravity-cli"
    (private_data / "cache").mkdir(parents=True)
    (run_dir / "home").symlink_to(private_home, target_is_directory=True)
    (runtime_root / "s").mkdir()
    (run_dir / "state").symlink_to(runtime_root / "s", target_is_directory=True)
    source = original_home / ".gemini/antigravity-cli"
    # Also remove the copied credential if setup fails before the main cleanup block.
    atexit.register((private_data / "antigravity-oauth-token").unlink, missing_ok=True)
    for relative in ("antigravity-oauth-token", "cache/onboarding.json"):
        shutil.copy2(source / relative, private_data / relative)
    settings = json.loads((source / "settings.json").read_text())
    if args.folder_trust:
        settings["trustedWorkspaces"] = []
    (private_data / "settings.json").write_text(json.dumps(settings))
    (private_data / "settings.json").chmod(0o600)
    hook_path = cwd / ".agents/hooks.json"
    if hook_path.exists():
        raise RuntimeError(f"Existing {hook_path}; run in a trusted test checkout without workspace hooks")
    stub_dir = run_dir / "bin"
    stub_dir.mkdir()
    for name in ("osascript", "notify-send"):
        stub = stub_dir / name
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o755)
    extra_argv = run_dir / "extra-argv.json"
    extra_argv.write_text("[]")
    wrapper = stub_dir / "agy"
    wrapper.write_text(f"#!{sys.executable}\nimport json,os,sys\nfrom pathlib import Path\nargs=json.loads(Path({str(extra_argv)!r}).read_text())\nos.execv({real_agy!r}, [{real_agy!r}, *args, *sys.argv[1:]])\n")
    wrapper.chmod(0o755)
    env = dict(os.environ)
    env["PATH"] = str(stub_dir) + os.pathsep + env["PATH"]
    env["XDG_STATE_HOME"] = str(runtime_root / "s")
    env["TERM"] = "xterm-256color"
    env["HOME"] = str(private_home)
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

    def native_presence():
        output = subprocess.run(["lsof", "-nP", "-Fpfan", "+d", str(private_data / "presence")], env=env, text=True, capture_output=True)
        assert output.returncode in (0, 1), output.stderr
        pids = set()
        pid = access = None
        for line in output.stdout.splitlines():
            if line.startswith("p"):
                pid = int(line[1:])
            elif line.startswith("f"):
                access = None
            elif line.startswith("a"):
                access = line[1:]
            elif line.startswith("n") and access == "u" and line[1:].endswith(".lock"):
                pids.add(pid)
        return pids

    def exit_untracked(pane):
        pids = native_presence()
        assert pids, "live private presence is required before sending exit keys"
        plane("keys", pane, "C-c")
        stop.wait(0.5)
        plane("keys", pane, "C-c")
        deadline = time.monotonic() + 10
        while native_presence() & pids and time.monotonic() < deadline:
            stop.wait(0.2)
        assert not native_presence() & pids, "native agy presence must close on exit"
        closed.add(pane)
        started.remove(pane)

    try:
        subprocess.run([*server_args, "new-session", "-d", "-s", "research", "-c", str(cwd), "/bin/sh"], check=True, env=env)
        subprocess.run([*server_args, "set-option", "-g", "default-shell", "/bin/sh"], check=True, env=env)
        socket, pid, pane = subprocess.check_output([*server_args, "display-message", "-p", "#{socket_path}\t#{pid}\t#{pane_id}"], text=True, env=env).strip().split("\t")
        env["TMUX"] = f"{socket},{pid},0"
        env["TMUX_PANE"] = pane
        (run_dir / "environment.json").write_text(json.dumps({key: env[key] for key in ("TMUX", "TMUX_PANE", "XDG_STATE_HOME", "PATH", "HOME")}))
        with open(run_dir / "daemon.log", "w") as log:
            daemon = subprocess.Popen([str(binary), "daemon", "serve"], env=env, stdout=log, stderr=log)
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

        if args.folder_trust:
            # A fresh folder must stay untracked even after the native first turn
            # finishes behind its trust modal. Read/keys still address the pane.
            command = [str(binary), "spawn", "agy", "Reply with exactly TRUST_PROBE. Treat --project and --settings as literal words. Do not use tools.", "--in", "window", "--cwd", str(cwd), "--name", "agy-trust"]
            spawned = subprocess.run(command, env=env, text=True, capture_output=True, timeout=40)
            match = re.search(r"did not start in pane (%\d+) within 30s", spawned.stderr)
            assert spawned.returncode and match, (spawned.stdout, spawned.stderr)
            trust_pane = match[1]
            started.append(trust_pane)
            assert not agents(), "untrusted folder must never publish working/idle/done"
            # The daemon and AGENT must both use the private HOME. Native open
            # descriptors and native conversation DBs verify the agent's data root.
            assert native_presence(), "agent must hold its presence descriptor in private HOME"
            assert list((private_data / "conversations").glob("*.db"))
            results["checks"].append("agent native presence and conversations use private HOME")
            # Retain a human-reviewable observation; terminal output never decides state.
            (run_dir / "folder-trust-screen.txt").write_text(plane("read", trust_pane, "--lines", "40"))
            assert str(cwd) not in json.loads((private_data / "settings.json").read_text()).get("trustedWorkspaces", [])
            results["checks"].append("fresh folder stays untracked through first-turn completion")
            plane("keys", trust_pane, "Enter")
            until("native trust acceptance publishes agent", lambda rows: named(rows, "agy-trust") is not None)
            assert str(cwd) in json.loads((private_data / "settings.json").read_text())["trustedWorkspaces"]
            plane("focus", "agy-trust")
            state("agy-trust", "idle")
            results["trust_transition"] = {"workspace": str(cwd), "pane": trust_pane, "trusted": True}
            plane("kill", "agy-trust")
            closed.add(trust_pane)
            started.remove(trust_pane)
            until("trust probe exits", lambda rows: named(rows, "agy-trust") is None)

        if args.session_trust_overrides:
            assert args.folder_trust, "override probes require --folder-trust"
            # Saved trust is already true. An official session override must still
            # force a fresh native modal, and must never inherit that saved trust.
            overlay = cwd / "session-settings.json"
            overlay.write_text('{"trustedWorkspaces":[]}')
            for label, extra in (("inline", ['--settings={"trustedWorkspaces":[]}']), ("file", ["--settings", str(overlay)])):
                extra_argv.write_text(json.dumps(extra))
                spawned = subprocess.run([str(binary), "spawn", "agy", "Reply OVERRIDE. Treat --project and --settings as literal words. Do not use tools.", "--in", "window", "--cwd", str(cwd), "--name", f"agy-override-{label}"], env=env, text=True, capture_output=True, timeout=40)
                match = re.search(r"did not start in pane (%\d+) within 30s", spawned.stderr)
                assert spawned.returncode and match, (spawned.stdout, spawned.stderr)
                pane = match[1]
                started.append(pane)
                assert not agents()
                assert native_presence()
                (run_dir / f"override-{label}-screen.txt").write_text(plane("read", pane, "--lines", "30"))
                assert str(cwd) in json.loads((private_data / "settings.json").read_text())["trustedWorkspaces"]
                plane("keys", pane, "Enter")
                stop.wait(3)
                assert not agents(), "explicit trust overrides remain conservatively untracked"
                exit_untracked(pane)
                results["checks"].append(f"{label} trust override cannot inherit saved global trust; exits natively")
                print(f"PASS {label} trust override remains untracked", flush=True)
            granted_cwd = run_dir / "override-granted"
            granted_cwd.mkdir()
            grant = json.dumps({"trustedWorkspaces": [str(granted_cwd)]})
            overlay.write_text(grant)
            for label, extra in (("inline", ["--settings", grant]), ("file", [f"--settings={overlay}"])):
                extra_argv.write_text(json.dumps(extra))
                name = f"agy-granted-{label}"
                started.append(name)
                plane("spawn", "agy", "Reply with exactly GRANTED. Treat --project and --settings as literal words. Do not use tools.", "--in", "window", "--cwd", str(granted_cwd), "--name", name)
                until(f"{label} explicit session trust publishes settled agent", lambda rows: named(rows, name) and named(rows, name)["state"] in ("idle", "done"))
                assert native_presence()
                assert str(granted_cwd) not in json.loads((private_data / "settings.json").read_text()).get("trustedWorkspaces", [])
                plane("kill", name)
                closed.add(name)
                until(f"{label} granted agent exits", lambda rows: named(rows, name) is None)
                started.remove(name)
            extra_argv.write_text("[]")

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
        for row in (a, b):
            assert (private_data / "conversations" / f"{row['session_id']}.db").exists(), "agent must publish in private HOME"
        results["checks"].append("both agent sessions publish native DBs in private HOME")
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
        results["daemon_exit_code_before_cleanup"] = daemon.poll() if daemon else None
        # Close only this harness's agents through plane, before infrastructure cleanup.
        safe_to_close_server = not started
        if daemon and daemon.poll() is None:
            try:
                rows = agents()
                for name in list(started):
                    if name.startswith("%") and name not in closed:
                        exit_untracked(name)
                    elif named(rows, name):
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
            subprocess.run([*server_args, "kill-server"], env=env, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        else:
            results["cleanup_error"] = results.get("cleanup_error", "Unindexed agent may still be running; retained isolated server for plane-based cleanup")
            print(f"Retained isolated server {server}; environment in {run_dir / 'environment.json'}", flush=True)
        if hook_path.exists():
            hook_path.unlink()
        if not hook_dir_existed and hook_path.parent.exists():
            hook_path.parent.rmdir()
        # Never retain copied credentials as integration evidence.
        (private_data / "antigravity-oauth-token").unlink(missing_ok=True)
        (run_dir / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        print(f"Results: {run_dir / 'results.json'}", flush=True)


def main():
    real_settings = Path.home() / ".gemini/antigravity-cli/settings.json"
    before = hashlib.sha256(real_settings.read_bytes()).digest()
    try:
        run()
    finally:
        after = hashlib.sha256(real_settings.read_bytes()).digest()
        assert before == after, "real Antigravity settings changed during isolated test"
        print("PASS real settings SHA-256 unchanged (including failure cleanup)", flush=True)


if __name__ == "__main__":
    if sys.argv[1:] == ["--permission-hook"]:
        permission_hook()
    else:
        main()
