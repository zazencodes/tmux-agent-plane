#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Recent conversation lifecycle/UI on a private tmux server; no model calls.

uv run tests/history_live.py target/debug/tmux-agent-plane
"""

import json
import hashlib
import os
import shlex
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from restore_live import Harness, Client, SOURCE, BINARY, check

settings = Path(os.environ["HOME"]) / ".gemini/antigravity-cli/settings.json"


def settings_hash():
    return (
        hashlib.sha256(settings.read_bytes()).hexdigest() if settings.exists() else None
    )


before = settings_hash()
h = Harness("tap-history-")
try:
    subprocess.run(
        [
            "cc",
            "-Wall",
            "-Wextra",
            "-Werror",
            str(SOURCE / "fixtures/restore/pi.c"),
            "-o",
            str(h.bin / "pi"),
        ],
        check=True,
    )
    work = h.root / "work"
    work.mkdir()
    h.server("pro", work)

    def spawn(name="recent-worker", *extra):
        h.plane(
            "spawn",
            "pi",
            "--in",
            "window",
            "--session",
            "pro",
            "--cwd",
            str(work),
            "--name",
            name,
            *extra,
        )
        return h.agent(name)

    def closed():
        return json.loads(h.plane("recent", "--json").stdout)

    original = spawn()
    h.plane("keys", original["name"], "C-c")
    entry = h.eventually("recent entry after close", lambda: closed() and closed()[0])
    identity = entry["id"]
    check(
        "closed agent retains native conversation, title, folder and project",
        entry["agent"]["session_id"] == original["session_id"]
        and entry["agent"]["title"] == original["title"]
        and entry["agent"]["cwd"] == original["cwd"]
        and entry["agent"]["session"] == "pro",
    )
    h.plane("daemon", "restart")
    check("closed history survives daemon restart", closed()[0]["id"] == identity)
    h.plane("revive", str(identity))
    back = h.agent("recent-worker")
    check(
        "revives exact conversation under its name in a new pane",
        back["session_id"] == original["session_id"]
        and back["pane"] != original["pane"],
    )
    check("running conversations leave recent history", not closed())
    refused = h.plane("revive", str(identity), check=False)
    check("repeat revive is refused", refused.returncode != 0)
    args = (h.root / ("argv-" + original["session_id"])).read_text().splitlines()
    check(
        "native resume receives the saved id",
        args[-1] == "--session " + original["session_id"],
    )

    h.plane("kill", back["name"])
    again = h.eventually("closed again", lambda: closed() and closed()[0])
    check(
        "reclosing updates one history entry",
        again["id"] == identity and len(closed()) == 1,
    )
    with ThreadPoolExecutor(2) as executor:
        results = list(
            executor.map(
                lambda _: h.plane("revive", str(identity), check=False), range(2)
            )
        )
    check(
        "concurrent revives launch once",
        sum(r.returncode == 0 for r in results) == 1 and len(h.index()["agents"]) == 1,
    )
    h.plane("kill", "recent-worker")
    h.eventually("closed for workspace guard", closed)
    h.plane("workspace", "move", "pro", "--to", "other")
    refused = h.plane("revive", str(identity), check=False)
    check(
        "existing project in another workspace is refused",
        refused.returncode != 0 and "another workspace" in refused.stderr,
    )
    h.plane("workspace", "move", "pro", "--to", "main")

    h.plane(
        "spawn",
        "pi",
        "--in",
        "session",
        "--session",
        "vanished",
        "--workspace",
        "archive",
        "--cwd",
        str(work),
        "--name",
        "archived-worker",
    )
    archived = h.agent("archived-worker")
    h.plane("kill", "archived-worker")
    archived_entry = h.eventually(
        "archived entry",
        lambda: next(
            (e for e in closed() if e["agent"]["name"] == "archived-worker"), None
        ),
    )
    h.plane("revive", str(archived_entry["id"]))
    archived_back = h.agent("archived-worker")
    check(
        "missing project returns to original workspace",
        archived_back["session"] == "vanished"
        and archived_back["workspace"] == "archive"
        and archived_back["session_id"] == archived["session_id"],
    )
    h.plane("kill", "archived-worker")
    h.eventually("closed archive", lambda: len(closed()) == 2)

    # Real terminal UI in a private client; keys go only through its PTY.
    window = h.tmux("display-message", "-p", "-t", "=pro:", "#{window_id}")
    h.plane("toggle-sidebar", window)
    client = Client(h, "pro")
    h.eventually("sidebar draws", lambda: "w workspaces" in client.text())
    client.key(b"h")
    h.eventually("recent browser", lambda: "Recently closed" in client.text())
    (h.root / "recent-sidebar.txt").write_text(client.text())
    check(
        "sidebar browser is workspace scoped",
        "recent-worker" in client.text() and "archived-worker" not in client.text(),
    )
    client.key(b"\x03")
    h.eventually(
        "control-c returns one screen", lambda: "Recently closed" not in client.text()
    )
    client.key(b"mh\r")
    back = h.eventually("sidebar revives", lambda: h.agent("recent-worker"))
    check(
        "sidebar Enter resumes saved conversation",
        back["session_id"] == original["session_id"],
    )
    h.eventually(
        "revived worker is focused despite the live orchestrator filter",
        lambda: h.agent("recent-worker")["visible"],
    )
    h.plane("kill", "recent-worker")
    h.eventually(
        "closed before picker", lambda: any(e["id"] == identity for e in closed())
    )
    # Opening a popup is infrastructure; agent actions remain CLI-owned.
    client_name = h.tmux("list-clients", "-F", "#{client_name}")
    command = shlex.join([str(BINARY), "ui", "--client", client_name])
    popup = subprocess.Popen(
        [
            str(h.wrapper),
            "display-popup",
            "-E",
            "-w",
            "95%",
            "-h",
            "95%",
            "-c",
            client_name,
            command,
        ],
        env=h.env,
    )
    time.sleep(0.5)
    client.key(b"h")
    h.eventually("picker recent browser", lambda: "Recently closed" in client.text())
    client.key(b"\r")
    back = h.eventually("picker revives", lambda: h.agent("recent-worker"))
    check(
        "picker Enter resumes saved conversation",
        back["session_id"] == original["session_id"],
    )
    popup.wait(timeout=5)
    h.plane("kill", "recent-worker")
    h.eventually("history before server restart", lambda: len(closed()) == 2)
    h.kill_server()
    h.server("pro", work)
    check(
        "closed history survives tmux restart",
        {e["id"] for e in closed()} == {identity, archived_entry["id"]},
    )
    found = json.loads(h.plane("recent", "--json", "--project", "pro", "--search", "RECENT worker").stdout)
    check("project and chat search compose", [e["id"] for e in found] == [identity])
    h.plane("forget", str(identity))
    h.plane("daemon", "restart")
    check("forgotten entry stays removed after restart", [e["id"] for e in closed()] == [archived_entry["id"]])
    check("removing a stale selection fails", h.plane("forget", str(identity), check=False).returncode != 0)
    print("PASS recent history lifecycle and both UIs", flush=True)
finally:
    for agent in (h.index() or {}).get("agents", []):
        h.plane("kill", agent["name"], check=False)
    h.stop()
    assert settings_hash() == before, "real Antigravity settings changed"
    print("evidence:", h.root, flush=True)
