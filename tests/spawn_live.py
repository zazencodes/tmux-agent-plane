#!/usr/bin/env python3
"""Private spawn regression proof. Run after cargo build; requires tmux, zsh, cc.

python3 tests/spawn_live.py [--codex]
The optional Codex proof copies only auth and uses private config/HOME/state.
Agent operations use the CLI; raw tmux manages only the nonagent test server.
Artifacts remain in the printed /tmp directory, except the private auth copy.
"""
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
CLI = REPO / "target/debug/tmux-agent-plane"
ROOT = Path(tempfile.mkdtemp(prefix="sp-", dir="/tmp"))
print("Evidence:", ROOT, flush=True)
BIN, HOME = ROOT / "bin", ROOT / "home"
for path in (BIN, HOME, HOME / ".codex", HOME / ".claude/sessions", ROOT / "scratch"):
    path.mkdir(parents=True, exist_ok=True)
TMUX, ZSH, CODEX = shutil.which("tmux"), shutil.which("zsh"), shutil.which("codex")
assert TMUX and ZSH and CLI.exists(), "tmux, zsh and cargo build required"
SERVER = "spawn-" + str(os.getpid())
ENV = dict(os.environ, HOME=str(HOME), XDG_STATE_HOME=str(ROOT / "state"),
           CODEX_HOME=str(HOME / ".codex"), CLAUDE_CONFIG_DIR=str(HOME / ".claude"),
           GROK_HOME=str(HOME / ".grok"), GROK_LEADER_SOCKET=str(ROOT / "no.sock"),
           PLANE_SPAWN_ROOT=str(ROOT), TERM="xterm-256color",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
ENV.pop("TMUX", None)
ENV.pop("ZDOTDIR", None)
# Retain Codex caller identity, but never use its inherited pane.
ENV["TMUX_PANE"] = "%999999"
for name, body in {
    "tmux": "exec " + shlex.quote(TMUX) + " -L " + SERVER + ' "$@"',
    "osascript": "exit 0",
    "notify-send": "exit 0",
}.items():
    path = BIN / name
    path.write_text("#!/bin/sh\n" + body + "\n")
    path.chmod(0o755)
# Force the daemon to observe a fast agent before the name option is written.
# This wraps CLI-owned tmux calls; it never drives agent panes itself.
(BIN / "tmux").write_text(
    f"#!{sys.executable}\nimport json, os, sys, time\nfrom pathlib import Path\n"
    f"root = Path({str(ROOT)!r})\nargs = sys.argv[1:]\n"
    "if args[0] == 'set-option' and '@agent-plane-name' in args and (root / 'delay-name').exists():\n"
    "    pane = args[args.index('-t') + 1]\n"
    "    deadline = time.monotonic() + 15\n"
    "    while True:\n"
    "        rows = json.loads((root / 'state/tmux-agent-plane/agents.json').read_text())['agents']\n"
    "        row = next((row for row in rows if row['pane'] == pane), None)\n"
    "        if row:\n"
    "            assert row['name'].startswith('claude-'), row\n"
    "            (root / 'before-name.json').write_text(json.dumps(row, indent=2))\n"
    "            break\n"
    "        if time.monotonic() >= deadline: raise RuntimeError('agent never indexed before metadata')\n"
    "        time.sleep(0.05)\n"
    f"os.execv({TMUX!r}, [{TMUX!r}, '-L', {SERVER!r}, *args])\n")
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(REPO / "tests/fixtures/spawn/agent.c"),
                "-o", str(BIN / "claude")], check=True)
os.mkfifo(ROOT / "gate")
gate = os.open(ROOT / "gate", os.O_RDWR | os.O_NONBLOCK)
# A FIFO makes readiness deterministic: startup cannot finish until the test releases it.
(HOME / ".zshrc").write_text(
    "export PATH=" + shlex.quote(str(BIN)) + ':"$PATH"\n'
    'if [[ ! -e "$PLANE_SPAWN_ROOT/released" ]]; then\n'
    '  touch "$PLANE_SPAWN_ROOT/waiting"\n'
    '  IFS= read -r gate_value < "$PLANE_SPAWN_ROOT/gate"\n'
    'fi\n'
    'print -r -- started >> "$PLANE_SPAWN_ROOT/startups"\n'
    "alias claude=" + shlex.quote(str(BIN / "claude") + " --alias-flag") + "\n"
    "alias codex='codex --no-alt-screen'\n")


def run(args, **kwargs):
    return subprocess.run(args, env=ENV, text=True, capture_output=True, timeout=45, **kwargs)


def plane(*args, check=True):
    result = run([str(CLI), *args])
    if check and result.returncode:
        raise AssertionError(f"{args}: {result.stderr}")
    return result


def infra(*args):
    result = run([str(BIN / "tmux"), *args])
    assert result.returncode == 0, result.stderr
    return result.stdout.strip()


def eventually(description, condition, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = condition()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(f"timeout: {description}; {ROOT}")


def release():
    (ROOT / "released").touch()
    os.write(gate, b"ready\n")


def spawn(kind, name, task):
    process = subprocess.Popen([str(CLI), "spawn", kind, "--in", "window", "--cwd",
                                str(ROOT / "scratch"), "--name", name, "--", task],
                               env=ENV, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    pending.append(process)
    return process


# Hash real settings, including the lead's baseline paths, without reading secrets into output.
real_home = Path.home()
paths = [real_home / p for p in (".gemini/settings.json", ".gemini/config/config.json",
         ".gemini/antigravity-cli/settings.json", ".pi/agent/settings.json",
         ".grok/config.toml", ".claude.json", ".config/opencode/tui.json",
         ".config/opencode/opencode.json", ".config/opencode/opencode.jsonc",
         ".gemini/trustedFolders.json", ".gemini/config/mcp_config.json",
         ".gemini/antigravity-backup/mcp_config.json", ".gemini/antigravity-ide/mcp_config.json",
         ".pi/agent/models.json", ".pi/agent/models-store.json", ".grok/trusted_folders.toml",
         ".grok/models_cache.json", ".grok/settings_cache.json",
         ".codex/config.toml", ".codex/auth.json")]
paths.extend((real_home / ".grok/marketplace-cache").rglob("mcp_config.json"))

def hashes():
    return {str(path): hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
            for path in paths}

before = hashes()
(ROOT / "settings-before.json").write_text(json.dumps(before, indent=2))
owned, pending = [], []
daemon = None
try:
    infra("-f", "/dev/null", "new-session", "-d", "-s", "scratch", "-x", "120", "-y", "40", "/bin/sh")
    infra("set-option", "-g", "default-shell", ZSH)
    infra("set-option", "-g", "default-command", "")
    log = (ROOT / "daemon.log").open("w")
    daemon = subprocess.Popen([str(CLI), "daemon", "serve"], env=ENV, stdout=log, stderr=log)
    eventually("daemon", lambda: plane("list", check=False).returncode == 0)
    special = "quote ' double \" dollar $(touch INJECTED) `touch INJECTED` ; & | <> \\ tabs\t\nsecond line\n"
    for i, task in enumerate((special, (special + "é漢🙂") * 80)):
        for name in ("waiting", "released", "argv", "exit", "startups"):
            (ROOT / name).unlink(missing_ok=True)
        name = f"spawn-fixture-{i}"
        process = spawn("claude", name, task)
        def waiting():
            if process.poll() is not None:
                stdout, stderr = process.communicate()
                raise AssertionError(f"spawn exited before shell startup: {stdout} {stderr}")
            return (ROOT / "waiting").exists()
        eventually("shell blocked during startup", waiting)
        assert process.poll() is None and not (ROOT / "argv").exists(), "agent ran before shell was ready"
        release()
        stdout, stderr = process.communicate(timeout=40)
        assert process.returncode == 0, stderr
        owned.append(name)
        pane = stdout.split()[1]
        assert (ROOT / "argv").read_bytes() == b"--alias-flag\0" + task.encode() + b"\0"
        assert not (ROOT / "scratch/INJECTED").exists()
        (ROOT / "exit").touch()
        eventually("interactive shell after agent exit", lambda: (ROOT / "startups").read_text().count("started") == 2)
        plane("kill", pane)
        owned.remove(name)
        print(f"PASS delayed startup, alias flag, exact argv ({len(task.encode())} bytes), shell after exit", flush=True)
    (ROOT / "exit").unlink()
    (ROOT / "delay-name").touch()
    process = spawn("claude", "spawn-fast", "fast startup")
    stdout, stderr = process.communicate(timeout=40)
    assert process.returncode == 0 and stdout.split()[0] == "spawn-fast", (stdout, stderr)
    owned.append("spawn-fast")
    initial = json.loads((ROOT / "before-name.json").read_text())
    assert initial["name"].startswith("claude-")
    final = next(row for row in json.loads(plane("list", "--json").stdout)
                 if row["pane"] == initial["pane"])
    assert final["name"] == "spawn-fast"
    (ROOT / "after-name.json").write_text(json.dumps(final, indent=2))
    (ROOT / "delay-name").unlink()
    plane("kill", "spawn-fast")
    owned.remove("spawn-fast")
    print("PASS fast startup waits for assigned name after daemon indexed default name", flush=True)
    # A failed startup must report an error, including panes retained by tmux.
    rc = HOME / ".zshrc"
    startup = rc.read_text()
    rc.write_text("exit 71\n")
    infra("set-option", "-g", "remain-on-exit", "on")
    started = time.monotonic()
    result = plane("spawn", "claude", "--in", "window", "--cwd", str(ROOT / "scratch"),
                   "--name", "spawn-failed", "--", "never runs", check=False)
    assert result.returncode and "spawn shell or agent exited" in result.stderr, result
    assert time.monotonic() - started < 10, "startup failure waited for detection timeout"
    # This pane never became an agent; cleanup of nonagent infrastructure is permitted.
    infra("kill-window", "-t", "spawn-failed")
    infra("set-option", "-g", "remain-on-exit", "off")
    rc.write_text(startup)
    print("PASS shell startup failure exits loudly before detection timeout", flush=True)
    if "--codex" in sys.argv:
        assert CODEX, "Codex required"
        auth = real_home / ".codex/auth.json"
        assert auth.exists(), "Codex auth required"
        shutil.copyfile(auth, HOME / ".codex/auth.json")
        (HOME / ".codex/auth.json").chmod(0o600)
        (HOME / ".codex/config.toml").write_text(
            'model = "gpt-6.1-sol"\nmodel_reasoning_effort = "high"\n'
            '[tui]\nterminal_title = ["spinner", "project", "thread"]\n'
            '[projects.' + json.dumps(str((ROOT / "scratch").resolve())) + ']\ntrust_level = "trusted"\n')
        # Record the actual executable's argv, then replace the wrapper with Codex.
        recorder = BIN / "codex"
        recorder.write_text(f"#!{sys.executable}\nimport os, sys\n"
                            f"open({str(ROOT / 'codex-argv')!r}, 'wb').write(b'\\0'.join(x.encode() for x in sys.argv[1:]))\n"
                            f"os.execv({CODEX!r}, [{CODEX!r}, *sys.argv[1:]])\n")
        recorder.chmod(0o755)
        prefix = "Reply with SPAWN_OK only. Do not use tools. This is inert transport test data:\n"
        task = prefix + ("a'b\"$`;\\ line\n" * 300)[:3000 - len(prefix)]
        assert len(task) == 3000
        (ROOT / "codex-task.txt").write_text(task)
        process = spawn("codex", "spawn-real-codex", task)
        stdout, stderr = process.communicate(timeout=40)
        assert process.returncode == 0, stderr
        owned.append("spawn-real-codex")
        assert (ROOT / "codex-argv").read_bytes() == b"--no-alt-screen\0" + task.encode()
        screen = plane("read", "spawn-real-codex", "--lines", "200").stdout
        (ROOT / "codex-screen.txt").write_text(screen)
        # Native rollout proves the real TUI accepted the full prompt as one user message.
        def accepted():
            for path in (HOME / ".codex/sessions").rglob("*.jsonl"):
                for line in path.read_text().splitlines():
                    item = json.loads(line)
                    payload = item.get("payload", {})
                    if (item.get("type") == "response_item" and payload.get("role") == "user"
                            and any(part.get("type") == "input_text" and part.get("text") == task
                                    for part in payload.get("content", []))):
                        return str(path)
            return None
        rollout = eventually("Codex accepted complete 3000-character task", accepted, 30)
        (ROOT / "codex-proof.json").write_text(json.dumps({"spawn": stdout, "rollout": rollout,
                                                          "characters": len(task)}, indent=2))
        print("PASS actual Codex accepted exactly 3000 characters; alias flag preserved", flush=True)
finally:
    release()
    for process in pending:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
    # Includes a pane whose launch failed before it entered the index.
    result = plane("list", "--json", check=False)
    if result.returncode == 0:
        for row in json.loads(result.stdout):
            plane("kill", row["name"], check=False)
    if daemon:
        daemon.terminate()
        daemon.wait(timeout=5)
    run([TMUX, "-L", SERVER, "kill-server"])
    os.close(gate)
    if "--codex" in sys.argv and CODEX:
        result = run([CODEX, "app-server", "daemon", "stop"])
        assert result.returncode == 0, result.stderr
        # Codex 0.166 leaves its private updater alive after daemon stop. Address
        # only the recorded PID after checking its executable and subcommand.
        updater = HOME / ".codex/app-server-daemon/daemon-updater.pid"
        if updater.exists():
            pid = json.loads(updater.read_text())["pid"]
            command = run(["ps", "-p", str(pid), "-o", "args="]).stdout.strip()
            if command:
                assert str(HOME.resolve()) in command and "daemon pid-update-loop" in command
                os.kill(pid, signal.SIGTERM)
    (HOME / ".codex/auth.json").unlink(missing_ok=True)
    after = hashes()
    (ROOT / "settings-after.json").write_text(json.dumps(after, indent=2))
    assert before == after, "real user settings changed during test"
    print("PASS real user settings/auth hashes unchanged", flush=True)
