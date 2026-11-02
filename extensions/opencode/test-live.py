#!/usr/bin/env python3
"""Real OpenCode TUI integration against a local model, with no external inference.
Run from the repository root after building a binary with OpenCode registered:
  python3 extensions/opencode/test-live.py --binary target/debug/tmux-agent-plane
Artifacts (config, status files, model requests and daemon log) stay under the printed /tmp/ path.
"""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/debug/tmux-agent-plane")
    args = parser.parse_args()
    binary = Path(args.binary).resolve()
    repo = Path(__file__).resolve().parents[2]
    tmux = shutil.which("tmux")
    if not tmux or not shutil.which("opencode"):
        raise RuntimeError("tmux and OpenCode 1.19.3 must be installed")
    run = Path(tempfile.mkdtemp(prefix="opencode-live-", dir="/tmp"))
    print(f"Artifacts: {run}", flush=True)
    server_name = run.name
    env = dict(os.environ)
    for key in ("TMUX", "TMUX_PANE", "CODEX_THREAD_ID", "OPENCODE_CONFIG", "OPENCODE_CONFIG_CONTENT", "OPENCODE_CONFIG_DIR", "OPENCODE_TUI_CONFIG"):
        env.pop(key, None)
    for kind in ("STATE", "CONFIG", "DATA", "CACHE"):
        env[f"XDG_{kind}_HOME"] = str(run / kind.lower())
    (run / "home").mkdir()
    env["HOME"] = str(run / "home")
    env["OPENCODE_DISABLE_AUTOUPDATE"] = "1"
    env["OPENCODE_DISABLE_PROJECT_CONFIG"] = "1"
    shim = run / "bin"
    shim.mkdir()
    (shim / "tmux").write_text(f"#!/bin/sh\nexec {shlex.quote(tmux)} -L {shlex.quote(server_name)} \"$@\"\n")
    for command in ("osascript", "notify-send"):
        (shim / command).write_text("#!/bin/sh\nexit 0\n")
    for path in shim.iterdir():
        path.chmod(0o755)
    env["PATH"] = f"{shim}:{env['PATH']}"
    scratch = run / "scratch"
    scratch.mkdir()
    config = run / "config/opencode"
    config.mkdir(parents=True)
    (config / "tmux-agent-plane.js").symlink_to(repo / "extensions/opencode/tmux-agent-plane.js")
    (config / "tui.json").write_text(json.dumps({"plugin": ["./tmux-agent-plane.js"]}))

    class Model(BaseHTTPRequestHandler):
        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            with (run / "model-requests.jsonl").open("a") as log:
                log.write(json.dumps(body) + "\n")
            messages = body.get("messages", [])
            last = next((m for m in reversed(messages) if m.get("role") == "user"), {})
            text = str(last.get("content", ""))
            tool = None
            if messages[-1].get("role") != "tool":
                if "fixture-bash" in text:
                    tool = ("bash", {"command": "printf plane-ok", "description": "Print a fixture marker"})
                if "fixture-question" in text:
                    tool = ("question", {"questions": [{"header": "Fixture", "question": "Which fixture?", "options": [{"label": "One", "description": "First fixture"}, {"label": "Two", "description": "Second fixture"}]}]})
            if "fixture-slow" in text:
                time.sleep(3)
            self.send_response(200)
            if body.get("stream"):
                self.send_header("Content-Type", "text/event-stream")
                self.end_headers()
                delta = {"role": "assistant", "content": "Fixture complete."}
                if tool:
                    delta = {"role": "assistant", "tool_calls": [{"index": 0, "id": "call_fixture", "type": "function", "function": {"name": tool[0], "arguments": json.dumps(tool[1])}}]}
                for data, reason in [(delta, None), ({}, "tool_calls" if tool else "stop")]:
                    chunk = {"id": "chat_fixture", "object": "chat.completion.chunk", "created": int(time.time()), "model": "test", "choices": [{"index": 0, "delta": data, "finish_reason": reason}]}
                    self.wfile.write(("data: " + json.dumps(chunk) + "\n\n").encode())
                    self.wfile.flush()
                self.wfile.write(b"data: [DONE]\n\n")
            else:
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"id": "chat_fixture", "object": "chat.completion", "created": int(time.time()), "model": "test", "choices": [{"index": 0, "message": {"role": "assistant", "content": "Fixture title"}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}}).encode())

        def log_message(self, *_):
            pass

    model = ThreadingHTTPServer(("127.0.0.1", 0), Model)
    threading.Thread(target=model.serve_forever, daemon=True).start()
    (config / "opencode.json").write_text(json.dumps({
        "model": "plane/test", "small_model": "plane/test", "autoupdate": False, "share": "disabled",
        "permission": {"bash": "ask"},
        "provider": {"plane": {"npm": "@ai-sdk/openai-compatible", "name": "Plane fixture", "options": {"baseURL": f"http://127.0.0.1:{model.server_port}/v1", "apiKey": "test"}, "models": {"test": {"name": "Test", "limit": {"context": 100000, "output": 4096}}}}},
    }))
    daemon = client = None
    started = False
    names = set()

    def plane(*arguments, check=True):
        result = subprocess.run([str(binary), *arguments], env=env, text=True, capture_output=True, timeout=45)
        if check and result.returncode:
            raise RuntimeError(f"plane {' '.join(arguments)}: {result.stderr}")
        return result.stdout

    def agents():
        return json.loads(plane("list", "--json"))

    def until(predicate, description, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            if daemon and daemon.poll() is not None:
                raise RuntimeError(f"daemon exited: {(run / 'daemon.log').read_text()}")
            time.sleep(0.1)
        raise RuntimeError(f"timed out: {description}")

    def agent(name):
        return next((a for a in agents() if a["name"] == name), None)

    def settled(name, count):
        until(lambda: (a := agent(name)) and a["completions"] == count and a["state"] in ("done", "idle"), f"{name} completion {count}")

    try:
        subprocess.run([tmux, "-L", server_name, "-f", "/dev/null", "new-session", "-d", "-s", "research", "-c", str(scratch)], env=env, check=True)
        started = True
        for option in ("default-shell", "default-command"):
            subprocess.run([tmux, "-L", server_name, "set-option", "-g", option, "/bin/sh"], env=env, check=True)
        with (run / "daemon.log").open("w") as log:
            daemon = subprocess.Popen([str(binary), "daemon", "serve"], env=env, stdout=log, stderr=subprocess.STDOUT)
        until(lambda: (run / "state/tmux-agent-plane/agents.json").exists(), "daemon startup")
        for name in ("oc-fast", "oc-second"):
            names.add(name)
            print(plane("spawn", "opencode", "fixture-fast", "--in", "window", "--cwd", str(scratch), "--name", name, "--wait", "--timeout", "30"), end="", flush=True)
            settled(name, 1)
        first, second = agent("oc-fast"), agent("oc-second")
        assert first["cwd"] == second["cwd"]
        assert first["pane"] != second["pane"] and first["session_id"] != second["session_id"]
        snapshots = [json.loads(p.read_text()) for p in (run / "state/tmux-agent-plane/opencode").glob("*.json")]
        assert len({p["pid"] for p in snapshots}) == 2
        print("PASS: two panes, same cwd, distinct PID/pane/native session")
        count = 1
        for _ in range(3):
            print(plane("send", "oc-fast", "fixture-fast", "--wait", "--timeout", "15"), end="")
            count += 1
            settled("oc-fast", count)
        plane("send", "oc-fast", "fixture-slow")
        until(lambda: agent("oc-fast")["state"] == "working", "live working")
        print(plane("wait", "oc-fast", "--timeout", "15"), end="")
        count += 1
        settled("oc-fast", count)
        for prompt, detail in [("fixture-bash", "permission: bash"), ("fixture-question", "question: Which fixture?")]:
            print(plane("send", "oc-fast", prompt, "--wait", "--timeout", "15"), end="")
            assert agent("oc-fast")["state"] == "blocked" and agent("oc-fast")["detail"] == detail
            plane("keys", "oc-fast", "Enter")
            count += 1
            settled("oc-fast", count)
        print("PASS: working, permission, question, idle/done and both wait modes")
        with (run / "control-client.log").open("w") as log:
            client = subprocess.Popen([tmux, "-L", server_name, "-C", "attach-session", "-t", "research"], env=env, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT)
        until(lambda: subprocess.run([tmux, "-L", server_name, "list-clients"], env=env, capture_output=True).stdout.strip(), "control client attach")
        plane("focus", "oc-fast")
        until(lambda: agent("oc-fast")["state"] == "idle" and agent("oc-fast")["visible"], "viewed idle")
        assert agent("oc-fast")["completions"] == count
        print("PASS: unseen done -> viewed idle using control client; no user screen")
        plane("keys", "oc-second", "C-c", "C-c")
        until(lambda: agent("oc-second") is None, "interactive exit")
        names.remove("oc-second")
        print("PASS: interactive exit removes detection")
    finally:
        if started:
            for name in names:
                plane("kill", name, check=False)
        if client:
            client.terminate()
            client.wait(timeout=5)
        if daemon:
            daemon.terminate()
            daemon.wait(timeout=5)
        if started:
            subprocess.run([tmux, "-L", server_name, "kill-server"], env=env, check=True)
        model.shutdown()


if __name__ == "__main__":
    main()
