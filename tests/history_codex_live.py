#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Real Codex close/revive, deterministic local Responses provider, no model quota.

uv run tests/history_codex_live.py target/debug/tmux-agent-plane
"""

import os, json, threading, hashlib, subprocess, signal
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from restore_live import Harness


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_POST(self):
        request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        print("local request", self.path, request.get("model"), flush=True)
        message = {
            "id": "msg_1",
            "type": "message",
            "role": "assistant",
            "status": "completed",
            "content": [
                {
                    "type": "output_text",
                    "text": "LOCAL_HISTORY_PROOF",
                    "annotations": [],
                }
            ],
        }
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        for e in [
            {
                "type": "response.created",
                "response": {"id": "resp_1", "status": "in_progress", "output": []},
            },
            {
                "type": "response.output_item.added",
                "output_index": 0,
                "item": dict(message, status="in_progress", content=[]),
            },
            {
                "type": "response.output_text.delta",
                "item_id": "msg_1",
                "output_index": 0,
                "content_index": 0,
                "delta": "LOCAL_HISTORY_PROOF",
            },
            {"type": "response.output_item.done", "output_index": 0, "item": message},
            {
                "type": "response.completed",
                "response": {
                    "id": "resp_1",
                    "status": "completed",
                    "output": [message],
                    "usage": {
                        "input_tokens": 10,
                        "output_tokens": 2,
                        "total_tokens": 12,
                    },
                },
            },
        ]:
            self.wfile.write(("data: " + json.dumps(e) + "\n\n").encode())
            self.wfile.flush()


server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
protected = [
    Path(os.environ["HOME"]) / p
    for p in (
        ".codex/config.toml",
        ".codex/auth.json",
        ".gemini/antigravity-cli/settings.json",
    )
]


def hashes():
    return {
        str(p): hashlib.sha256(p.read_bytes()).hexdigest() if p.exists() else None
        for p in protected
    }


before = hashes()
h = Harness("tap-hcodexlocal-")
codex = h.root / "codex"
codex.mkdir()
try:
    work = h.root / "work"
    work.mkdir()
    h.env["CODEX_HOME"] = str(codex)
    (codex / "config.toml").write_text(
        f'model = "local-proof"\nmodel_provider = "fixture"\n[model_providers.fixture]\nname = "Local proof"\nbase_url = "http://127.0.0.1:{server.server_port}/v1"\nwire_api = "responses"\nrequires_openai_auth = false\n[tui]\nterminal_title = ["activity", "session-id", "thread-name", "project-name"]\n[projects.'
        + json.dumps(str(work.resolve()))
        + ']\ntrust_level = "trusted"\n'
    )
    h.server("pro", work)
    print(
        h.plane(
            "spawn",
            "codex",
            "Reply with LOCAL_HISTORY_PROOF.",
            "--in",
            "window",
            "--session",
            "pro",
            "--cwd",
            str(work),
            "--name",
            "native-codex",
            timeout=60,
        ).stdout
    )
    a = h.eventually(
        "id", lambda: (a := h.agent("native-codex")) and a["session_id"] and a, 30
    )
    original_id = a["session_id"]
    assert original_id not in a["title"], a
    print("PASS real Codex publishes a resolvable title ID", flush=True)
    h.eventually(
        "finished",
        lambda: (
            h.agent("native-codex")["state"] != "working"
            and "• LOCAL_HISTORY_PROOF" in h.plane("read", "native-codex").stdout
        ),
        30,
    )
    h.plane("kill", "native-codex")
    entries = h.eventually(
        "closed", lambda: json.loads(h.plane("recent", "--json").stdout)
    )
    assert entries[0]["agent"]["session_id"] == original_id
    print(h.plane("revive", str(entries[0]["id"]), timeout=50).stdout)
    a = h.agent("native-codex")
    assert a["session_id"] == original_id, a
    screen = h.plane("read", "native-codex", "--lines", "60").stdout
    assert "LOCAL_HISTORY_PROOF" in screen, screen
    assert not json.loads(h.plane("recent", "--json").stdout)
    print(
        "PASS real Codex revives exact conversation with earlier messages", flush=True
    )
finally:
    for a in (h.index() or {}).get("agents", []):
        h.plane("kill", a["name"], check=False)
    h.stop()
    result = subprocess.run(
        ["codex", "app-server", "daemon", "stop"],
        env=h.env,
        capture_output=True,
        text=True,
        timeout=15,
    )
    assert result.returncode == 0, result.stderr
    updater = codex / "app-server-daemon/daemon-updater.pid"
    if updater.exists():
        pid = json.loads(updater.read_text())["pid"]
        command = subprocess.run(
            ["ps", "-p", str(pid), "-o", "args="], capture_output=True, text=True
        ).stdout
        if command.strip():
            assert (
                str(codex.resolve()) in command and "daemon pid-update-loop" in command
            )
            os.kill(pid, signal.SIGTERM)
    server.shutdown()
    assert hashes() == before, "real settings/auth changed"
    print("PASS real settings/auth unchanged")
    print("evidence:", h.root)
