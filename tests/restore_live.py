#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Restore proof on isolated tmux servers that are killed and started again.

Usage: uv run tests/restore_live.py target/debug/tmux-agent-plane [--claude]

Without --claude no agent CLI or model is used: fixture Pi agents resume through `--session` and a
fixture Codex cannot come back. Three server lifetimes check the roster (sidebars left out, only
running tasks), `restore` listing and doing nothing, `restore --no-tasks` rebuilding sessions,
windows, panes, layouts, names and parent links, the sidebar's `b` (Esc cancels, Enter restores
agents and tasks while a sidebar hook races the rebuild) and `restore --discard`.

--claude also brings back a real Claude Code agent after a one-line turn, in a scratch folder under
this checkout's target/ (inside Alex's trusted ~/pro), with the real HOME: same session id, name,
session, window and layout. It never changes Claude settings or trust.
"""
import fcntl
import json
import os
import pty
import re
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time
from pathlib import Path

import pyte

SOURCE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1]).resolve()
CLAUDE = "--claude" in sys.argv[2:]
TMUX = shutil.which("tmux")
assert TMUX
RESULTS = {}


def check(label, condition):
    assert condition, label
    RESULTS[label] = True
    print("ok", label, flush=True)


class Harness:
    """One private state root and socket; servers on it come and go."""

    def __init__(self, prefix, home=None):
        self.root = Path(tempfile.mkdtemp(prefix=prefix, dir="/tmp"))
        self.socket = str(self.root / "t.sock")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.state = self.root / "s" / "tmux-agent-plane"
        (self.state / "pi").mkdir(parents=True)
        (self.root / "home").mkdir()
        self.env = dict(os.environ, HOME=home or str(self.root / "home"),
                        XDG_STATE_HOME=str(self.root / "s"),
                        XDG_CONFIG_HOME=str(self.root / "config"),
                        GROK_LEADER_SOCKET=str(self.root / "absent.sock"),
                        PLANE_NAV_ROOT=str(self.root), TERM="xterm-256color", SHELL="/bin/sh",
                        PATH=str(self.bin) + os.pathsep + os.environ["PATH"])
        if home is None:
            self.env.update(CLAUDE_CONFIG_DIR=str(self.root / "claude"),
                            GROK_HOME=str(self.root / "grok"))
        for name in ("TMUX", "TMUX_PANE", "CODEX_THREAD_ID", "CODEX_SESSION_ID"):
            self.env.pop(name, None)
        self.wrapper = self.bin / "tmux"
        self.wrapper.write_text(f"#!/bin/sh\nexec {TMUX} -S {self.socket} \"$@\"\n")
        self.wrapper.chmod(0o755)
        (self.bin / "tmux-agent-plane").symlink_to(BINARY)
        for tool in ("osascript", "notify-send"):
            (self.bin / tool).write_text("#!/bin/sh\n")
            (self.bin / tool).chmod(0o755)
        self.clients = []

    def tmux(self, *args, check=True):
        result = subprocess.run([str(self.wrapper), *args], env=self.env, text=True,
                                capture_output=True, timeout=8)
        if check and result.returncode != 0:
            raise AssertionError(f"tmux {args} failed: {result.stderr}")
        return result.stdout.strip()

    def plane(self, *args, check=True, env=None, timeout=60):
        result = subprocess.run([str(BINARY), *args], env=env or self.env, text=True,
                                capture_output=True, timeout=timeout)
        if check and result.returncode != 0:
            raise AssertionError(f"{args} failed: {result.stdout}\n{result.stderr}")
        return result

    def eventually(self, label, predicate, timeout=20):
        deadline = time.time() + timeout
        while time.time() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.2)
        for client in self.clients:
            print(client.text(), file=sys.stderr)
        print(self.tmux("list-panes", "-a", "-F", "#{session_name}:#{window_index} #{pane_id} "
                        "#{@agent-plane-sidebar} #{pane_current_command}", check=False),
              file=sys.stderr)
        print((self.state / "daemon.log").read_text()[-3000:], file=sys.stderr)
        raise AssertionError(f"timed out: {label}")

    def json(self, name):
        path = self.state / name
        return json.loads(path.read_text()) if path.exists() else None

    def index(self):
        return self.json("agents.json")

    def agent(self, name):
        return next((a for a in (self.index() or {"agents": []})["agents"]
                     if a["name"] == name), None)

    def status(self):
        return self.tmux("show-option", "-gqv", "@agent_plane_status", check=False)

    def server(self, session, cwd):
        """A fresh server whose first session holds a plain shell, and its daemon."""
        self.tmux("-f", "/dev/null", "new-session", "-d", "-s", session, "-x", "160", "-y", "45",
                  "-c", str(cwd), "/bin/sh")
        self.tmux("set-option", "-g", "default-size", "160x45")
        self.plane("daemon", "start")
        return tuple(self.tmux("display-message", "-p", "#{pid} #{start_time}").split())

    def close_clients(self):
        # An unread PTY blocks its client, and a dying server waits for every client.
        for client in self.clients:
            client.process.kill()
            client.process.wait()
            os.close(client.master)
        self.clients.clear()

    def kill_server(self):
        self.close_clients()
        pid = int(self.tmux("display-message", "-p", "#{pid}"))
        self.tmux("kill-server", check=False)

        def gone():
            try:
                os.kill(pid, 0)
            except ProcessLookupError:
                return True
            return False
        try:
            self.eventually("server gone", gone, 15)
        except AssertionError:
            print(subprocess.run(["ps", "-ax", "-o", "pid,ppid,stat,args"], text=True,
                                 capture_output=True).stdout, file=sys.stderr)
            raise
        # The daemon stops when it can no longer read tmux.
        self.eventually("daemon gone", lambda: not self.daemon_running(), 15)

    def daemon_running(self):
        lock = self.state / "daemon.lock"
        if not lock.exists():
            return False
        with open(lock) as file:
            try:
                fcntl.flock(file, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return True
            fcntl.flock(file, fcntl.LOCK_UN)
            return False

    def stop(self):
        self.plane("daemon", "stop", check=False)
        self.close_clients()
        subprocess.run([TMUX, "-S", self.socket, "kill-server"], env=self.env,
                       capture_output=True)

    def windows(self):
        """Every window: name, layout and its panes in order."""
        rows = self.tmux("list-panes", "-a", "-F", "\x1f".join([
            "#{session_name}", "#{window_index}", "#{window_name}", "#{window_layout}",
            "#{pane_id}", "#{pane_current_path}", "#{@agent-plane-sidebar}",
            "#{@agent-plane-placed}", "end"]))
        windows = {}
        for row in rows.splitlines():
            # Python strips \x1f as whitespace: a last literal field keeps empty options.
            session, index, name, layout, pane, cwd, sidebar, placed, _ = row.split("\x1f")
            w = windows.setdefault((session, int(index)), {"name": name, "layout": layout,
                                                           "panes": [], "sidebars": []})
            if sidebar == "1":
                w["sidebars"].append(pane)
            else:
                w["panes"].append({"id": pane, "cwd": cwd, "placed": placed == "1"})
        return windows


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    """A real attached client in a PTY, decoded with pyte. The sidebar is the left 38 columns."""

    def __init__(self, harness, session):
        self.harness = harness
        self.screen = Screen(140, 40)
        self.stream = pyte.ByteStream(self.screen)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 140, 0, 0))
        self.process = subprocess.Popen([str(harness.wrapper), "attach-session", "-t", session],
                                        env=harness.env, stdin=slave, stdout=slave, stderr=slave)
        harness.clients.append(self)

    def text(self):
        while select.select([self.master], [], [], 0.05)[0]:
            self.stream.feed(os.read(self.master, 65536))
        return "\n".join(line[:38] for line in self.screen.display)

    def key(self, data):
        os.write(self.master, data)
        time.sleep(0.4)


def shape(layout):
    """A layout without its checksum and pane ids: sizes, offsets and splits only."""
    return re.sub(r"(\d+x\d+,\d+,\d+),\d+", r"\1", layout.split(",", 1)[1])


def leaves(layout):
    return re.findall(r"\d+x\d+,\d+,\d+,(\d+)", layout)


def recorded_agents(roster):
    return [p["agent"] for w in roster["windows"] for p in w["panes"] if p["agent"]]


def fixtures():
    h = Harness("tap-rst-")
    try:
        for name, source in (("pi", "restore/pi.c"), ("codex", "sidebar/codex.c")):
            subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures" / source),
                            "-o", str(h.bin / name)], check=True)
        work = h.root / "work"
        pro, api = work / "pro", work / "api"
        for folder in (pro / "sub", api):
            folder.mkdir(parents=True)
        pro, api = pro.resolve(), api.resolve()

        # Server one: a project window of four panes plus the sidebar, and another project.
        first = h.server("pro", work)
        h.plane("spawn", "pi", "--in", "window", "--session", "pro", "--cwd", str(pro),
                "--name", "reviewer")
        reviewer = h.eventually("reviewer", lambda: h.agent("reviewer"))
        as_reviewer = dict(h.env, TMUX_PANE=reviewer["pane"])
        child = h.plane("spawn", "pi", "--in", "pane", "--cwd", str(pro),
                        env=as_reviewer).stdout.split()[0]
        child = h.eventually("child", lambda: h.agent(child))
        h.tmux("split-window", "-v", "-d", "-t", child["pane"], "-c", str(pro / "sub"), "/bin/sh")
        h.tmux("split-window", "-v", "-d", "-t", reviewer["pane"], "-c", str(pro), "codex")
        h.eventually("codex", lambda: any(a["kind"] == "codex" for a in h.index()["agents"]))
        h.plane("sessions", "create", "api", "--cwd", str(api))
        loner = h.plane("spawn", "pi", "--in", "window", "--session", "api",
                        "--cwd", str(api)).stdout.split()[0]
        loner = h.eventually("api agent", lambda: h.agent(loner))
        h.plane("task", "start", "web", "--cwd", str(pro), "--", "sleep", "100000", env=as_reviewer)
        h.plane("task", "start", "once", "--cwd", str(pro), "--", "true")
        h.plane("toggle-sidebar", reviewer["window"])
        before = h.windows()
        review_window = before[("pro", int(reviewer["location"].split(":")[1].split(".")[0]))]
        check("the project window has four panes and a sidebar",
              len(review_window["panes"]) == 4 and len(review_window["sidebars"]) == 1)

        def settled():
            roster = h.json("roster.json")
            return (roster and len(roster["windows"]) == 2 and len(recorded_agents(roster)) == 4
                    and [t["name"] for t in roster["tasks"]] == ["web"] and roster)
        roster = h.eventually("roster records both windows", settled)
        check("the roster belongs to this server",
              (str(roster["server"]["pid"]), str(roster["server"]["start_time"])) == first)
        window = next(w for w in roster["windows"] if w["session"] == "pro")
        sidebar = review_window["sidebars"][0].lstrip("%")
        check("the roster leaves the sidebar out of panes and layout",
              len(window["panes"]) == 4 and sidebar not in leaves(window["layout"])
              and len(leaves(window["layout"])) == 4)
        named = {a["name"]: a for a in recorded_agents(roster)}
        check("names, kinds, ids and parents are recorded",
              named["reviewer"]["named"] and not named[child["name"]]["named"]
              and named[child["name"]]["parent"] == reviewer["pane"]
              and named[child["name"]]["session_id"] == child["session_id"]
              and any(a["kind"] == "codex" and a["session_id"] is None for a in named.values()))
        check("only the running task is recorded, with its starter",
              roster["tasks"] == [{"name": "web", "cwd": str(pro), "command": ["sleep", "100000"],
                                   "started_by": "reviewer"}])
        ids = {name: named[name]["session_id"] for name in ("reviewer", child["name"], loner["name"])}

        # Server two: the roster becomes the pending restore before anything replaces it.
        h.kill_server()
        second = h.server("pro", work)
        check("a new server is a new identity", second != first)
        pending = h.eventually("pending restore", lambda: h.json("restore.json"))
        check("the pending restore is the old roster", pending == roster)
        h.eventually("status shows the waiting agents", lambda: "↺ 3" in h.status())
        check("the index publishes the count", h.index()["pending_restore"] == 3)
        check("the roster is now this server's",
              str(h.json("roster.json")["server"]["pid"]) == second[0])
        listing = h.plane("restore").stdout
        check("restore lists what comes back, what cannot and why, and the tasks",
              "3 agents from before tmux restarted can be brought back" in listing
              and re.search(r"reviewer\s+pi\s+pro", listing)
              and "Codex publishes no session id to resume" in listing
              and re.search(r"web\s+started by reviewer", listing)
              and "--no-tasks" in listing)
        check("restore alone does nothing", h.json("restore.json") == roster
              and len(h.index()["agents"]) == 0)
        done = h.plane("restore", "--no-tasks")
        check("restore --no-tasks reports each agent and what stays closed",
              f"reviewer (pi) is back in pro:{window['index']}" in done.stdout
              and "stays closed: Codex publishes no session id" in done.stdout)
        for name, session_id in ids.items():
            h.eventually(f"{name} resumed", lambda s=session_id: any(
                a["session_id"] == s for a in h.index()["agents"]))
        agents = {a["session_id"]: a for a in h.index()["agents"]}
        back = {name: agents[session_id] for name, session_id in ids.items()}
        for session_id in ids.values():
            check(f"{session_id} was started with --session",
                  (h.root / f"argv-{session_id}").read_text().splitlines()[-1]
                  == f"--session {session_id}")
        check("a given name comes back, a default name follows its new pane",
              back["reviewer"]["name"] == "reviewer"
              and back[child["name"]]["name"] == "pi-" + back[child["name"]]["pane"].lstrip("%"))
        check("the parent link points at the restored parent",
              back[child["name"]]["parent"] == back["reviewer"]["pane"])
        after = h.windows()
        rebuilt = after[("pro", window["index"])]
        check("the window has its name, panes in order and recorded layout",
              rebuilt["name"] == window["name"]
              and [p["cwd"] for p in rebuilt["panes"]] == [p["cwd"] for p in window["panes"]]
              and shape(rebuilt["layout"]) == shape(window["layout"]))
        check("the existing session keeps its own window", ("pro", 0) in after)
        api_window = next(w for w in roster["windows"] if w["session"] == "api")
        check("a missing session is created with the window at its index",
              ("api", api_window["index"]) in after
              and back[loner["name"]]["location"].startswith(f"api:{api_window['index']}."))
        check("restored agents count as placed", all(
            p["placed"] for w in after.values() for p in w["panes"]
            if p["id"] in {a["pane"] for a in back.values()}))
        check("no tasks without --tasks", h.index()["tasks"] == [])
        check("the pending restore is gone", h.json("restore.json") is None)
        h.eventually("status drops ↺", lambda: "↺" not in h.status())
        check("restore with nothing pending says so", h.plane("restore").stdout.strip()
              == "nothing to restore" and h.plane("restore", "--tasks", check=False).returncode != 0)
        check("no tracking faults", "⚠" not in h.status())
        # Codex starts again by hand in its pane, which came back as a shell.
        codex = next(i for i, p in enumerate(window["panes"])
                     if p["agent"] and p["agent"]["kind"] == "codex")
        h.tmux("respawn-pane", "-k", "-t", rebuilt["panes"][codex]["id"], "codex")

        # Server three: the sidebar's b, with a sidebar hook racing the rebuilt windows.
        h.plane("task", "start", "web", "--cwd", str(pro), "--", "sleep", "100000",
                env=dict(h.env, TMUX_PANE=back["reviewer"]["pane"]))
        h.eventually("roster of server two", lambda: (r := h.json("roster.json"))
                     and len(recorded_agents(r)) == 4 and r["tasks"] and r)
        h.kill_server()
        h.server("pro", work)
        h.tmux("set-hook", "-g", "window-linked[90]",
               "run-shell -b \"tmux-agent-plane follow-sidebar '#{hook_window}'\"")
        h.eventually("pending again", lambda: h.json("restore.json"))
        client = Client(h, "pro")
        h.plane("enter-sidebar", h.tmux("display-message", "-p", "-t", "pro:0", "#{window_id}"))
        h.eventually("top line", lambda: "↺ 3 agents from before tmux" in client.text()
                     and "restarted · b restore" in client.text())
        client.key(b"b")
        shown = h.eventually("confirmation", lambda: "bring back 3 agents?" in client.text()
                             and client.text())
        check("b lists agents, what cannot come back and why, and tasks with their starter",
              "▸ reviewer  pi  pro" in shown and "cannot come back" in shown
              and "Codex publishes no session" in shown and "▸ web  by reviewer" in shown
              and "⏎ agents + tasks  a agents only" in shown and "d discard  esc cancel" in shown)
        client.key(b"\x1b")
        h.eventually("esc closes", lambda: "bring back" not in client.text())
        check("esc keeps the pending restore", h.json("restore.json") is not None
              and len(h.index()["agents"]) == 0)
        client.key(b"b")
        h.eventually("asked again", lambda: "bring back 3 agents?" in client.text())
        client.key(b"\r")
        result = h.eventually("restore result", lambda: "any key closes" in client.text()
                              and client.text(), 40)
        check("the result says who is back", "reviewer (pi) is back" in result
              and "task web is running" in result)
        h.eventually("agents back", lambda: len(h.index()["agents"]) == 3)
        task = h.eventually("task back", lambda: next(
            (t for t in h.index()["tasks"] if t["name"] == "web"), None))
        check("Enter brought back the task with its starter",
              task["state"] == "running" and task["started_by"] == "reviewer")
        rebuilt = h.eventually("sidebars follow the rebuilt windows", lambda: (
            w := h.windows()) and all(len(x["sidebars"]) == 1 for x in w.values()) and w)
        review = rebuilt[("pro", window["index"])]
        check("a racing sidebar hook waits for the window: panes and order intact",
              [p["cwd"] for p in review["panes"]] == [p["cwd"] for p in window["panes"]])
        client.key(b"q")
        h.eventually("any key closes the result", lambda: "any key closes" not in client.text())
        h.eventually("top line gone", lambda: "↺" not in client.text())

        # Server four: discard.
            # Codex stayed closed this time.
        h.eventually("roster of server three", lambda: (r := h.json("roster.json"))
                     and len(recorded_agents(r)) == 3 and r)
        h.kill_server()
        h.server("pro", work)
        h.eventually("pending once more", lambda: h.json("restore.json"))
        out = h.plane("restore", "--discard").stdout
        check("restore --discard forgets it", out.strip()
              == "forgot 3 agents from before tmux restarted" and h.json("restore.json") is None)
        h.eventually("status drops ↺ after discard", lambda: "↺" not in h.status())
        check("discard brought nothing back", len(h.index()["agents"]) == 0)
    finally:
        h.stop()
    shutil.rmtree(h.root)


def claude():
    checkout = SOURCE.parent
    scratch = Path(tempfile.mkdtemp(prefix="restore-live-", dir=checkout / "target")).resolve()
    h = Harness("tap-rstc-", home=os.environ["HOME"])
    try:
        first = h.server("work", scratch)
        out = h.plane("spawn", "claude", "Reply with just the word ok.", "--in", "window",
                      "--session", "work", "--cwd", str(scratch), "--name", "restorer",
                      "--wait", "--timeout", "120", timeout=180).stdout
        agent = h.agent("restorer")
        check("claude finished its turn", agent and agent["session_id"] and "restorer" in out)
        h.tmux("split-window", "-h", "-d", "-t", agent["pane"], "-c", str(scratch), "/bin/sh")
        roster = h.eventually("claude in the roster", lambda: (r := h.json("roster.json"))
                              and recorded_agents(r) and len(r["windows"][0]["panes"]) == 2
                              and r)
        window = roster["windows"][0]
        h.kill_server()
        second = h.server("work", scratch)
        check("claude: a new server", second != first)
        h.eventually("claude pending", lambda: h.json("restore.json"))
        print(h.plane("restore", "--no-tasks").stdout, end="")
        back = h.eventually("claude resumed", lambda: next(
            (a for a in h.index()["agents"] if a["session_id"] == agent["session_id"]), None), 60)
        check("claude came back with the same session id and name",
              back["kind"] == "claude" and back["name"] == "restorer")
        rebuilt = h.windows()[("work", window["index"])]
        check("claude: same session, window, name and layout",
              back["location"].startswith(f"work:{window['index']}.")
              and rebuilt["name"] == window["name"]
              and shape(rebuilt["layout"]) == shape(window["layout"]))
        screen = h.eventually("the earlier turn on screen", lambda: re.search(
            r"\bok\b", h.plane("read", "restorer", "--lines", "200").stdout), 30)
        check("claude shows the resumed conversation", screen)
        h.plane("kill", "restorer")
    finally:
        h.stop()
        shutil.rmtree(h.root, ignore_errors=True)
        shutil.rmtree(scratch, ignore_errors=True)
        # The throwaway conversation Claude kept for the scratch folder.
        transcripts = Path(os.environ["HOME"]) / ".claude" / "projects" / re.sub(
            r"[^A-Za-z0-9]", "-", str(scratch))
        shutil.rmtree(transcripts, ignore_errors=True)


try:
    fixtures()
    if CLAUDE:
        claude()
    print(json.dumps({"passed": True, "checks": len(RESULTS)}))
except BaseException:
    for path in Path("/tmp").glob("tap-rst*"):
        print("evidence:", path, file=sys.stderr)
    raise
