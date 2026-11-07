#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private sidebar navigation proof: native Pi fixtures, no agent CLI/model calls.

uv run tests/sidebar_navigation_live.py target/debug/tmux-agent-plane
uv run tests/sidebar_navigation_live.py target/debug/tmux-agent-plane --search-only
uv run tests/sidebar_navigation_live.py target/debug/tmux-agent-plane --bold-only
PTY decoding asserts UI only. Agent state comes exclusively from native fixtures/index.
Filter and search editing pause preview; / freezes the filter and previews; Enter opens.
Evidence persists in /tmp/tap-nav-*. Agent jobs use the branch CLI.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

import pyte

ROOT = Path(tempfile.mkdtemp(prefix="tap-nav-", dir="/tmp"))
SOURCE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1]).resolve()
TMUX = shutil.which("tmux")
assert TMUX
BIN = ROOT / "bin"
BIN.mkdir()
STATE = ROOT / "s" / "tmux-agent-plane"
(STATE / "pi").mkdir(parents=True)
(ROOT / "home").mkdir()
(ROOT / "claude" / "sessions").mkdir(parents=True)
ENV = dict(os.environ, HOME=str(ROOT / "home"), XDG_STATE_HOME=str(ROOT / "s"),
           CLAUDE_CONFIG_DIR=str(ROOT / "claude"), GROK_HOME=str(ROOT / "grok"),
           GROK_LEADER_SOCKET=str(ROOT / "absent.sock"), PLANE_NAV_ROOT=str(ROOT), XDG_CONFIG_HOME=str(ROOT / "config"), TERM="xterm-256color",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
ENV.pop("TMUX", None)
ENV.pop("NO_COLOR", None)  # The agent tool environment disables colours; this PTY tests them.
# Keep Codex identity intact, use explicit names/window placements for agent jobs.
WRAPPER = BIN / "tmux"
WRAPPER.write_text(f"#!{sys.executable}\nimport json, os, sys\n"
    f"with open({str(ROOT / 'tmux.calls')!r}, 'a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
    f"if 'switch-client' in sys.argv and os.path.exists({str(ROOT / 'switch.fail')!r}):\n"
    "    print('injected switch failure', file=sys.stderr); sys.exit(23)\n"
    f"os.execv({TMUX!r}, [{TMUX!r}, '-S', {str(ROOT / 't.sock')!r}, *sys.argv[1:]])\n")
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    path = BIN / tool
    path.write_text("#!/bin/sh\necho delivered >> " + shlex.quote(str(ROOT / "desktop.delivered")) + "\n")
    path.chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/sidebar/agent.c"), "-o", str(fixture)], check=True)
(BIN / "pi").symlink_to(fixture)


def tmux(*args, check=True):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args):
    result = subprocess.run([str(BINARY), *args], env=ENV, check=True, text=True,
                            capture_output=True, timeout=20)
    output = result.stdout.strip()
    if args[0] == 'spawn':
        name, pane, _ = output.split()
        FIXTURES[name] = pane
    return output


def index():
    return json.loads((STATE / "agents.json").read_text())


def ranked_names():
    priority = {"blocked": 0, "done": 1, "working": 2, "idle": 3}
    return [a["name"] for a in sorted(index()["agents"], key=lambda a: (
        a["session"], priority[a["state"]], a["state_since"], a["location"], a["pane"]))]


def agent(name):
    return next(a for a in index()["agents"] if a["name"] == name)


def change(name, state, count, identity=0):
    path = ROOT / (agent(name)["pane"] + ".command")
    temporary = path.with_suffix(".tmp")
    temporary.write_text(f"{state} {count} {identity}")
    temporary.replace(path)


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(100, 28)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 28, 100, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), "attach-session", "-t", session], env=ENV,
            stdin=self.slave, stdout=self.slave, stderr=self.slave)
        self.raw = (ROOT / f"{session}.raw").open("wb")

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            data = os.read(self.master, 65536)
            self.raw.write(data)
            self.raw.flush()
            self.stream.feed(data)

    def snapshot(self, label):
        self.drain()
        (ROOT / f"{self.process.pid}-{label}.txt").write_text("\n".join(self.screen.display) + "\n")

    def footer(self):
        return self.screen.display[-1][:38]

    def context(self):
        rows = tmux("list-clients", "-F", "#{client_pid} #{client_name} #{pane_id} #{window_id} #{session_name}")
        row = next(line for line in rows.splitlines() if line.split()[0] == str(self.process.pid))
        return row.split()[1:]

    def selected(self):
        self.drain()
        return next((line[1:38].split()[1] for line in self.screen.display if line[1:2] == "▌" and len(line[1:38].split()) > 1), None)

    def text(self):
        self.drain()
        return "\n".join(line[:38] for line in self.screen.display)

    def key(self, value):
        os.write(self.master, value)

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.raw.close()
        os.close(self.master)
        os.close(self.slave)


CLIENTS = []


def wait(label, condition, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        try:
            if condition():
                return
        except (FileNotFoundError, json.JSONDecodeError, StopIteration):
            pass
        time.sleep(0.05)
    for client in CLIENTS:
        client.snapshot("failure")
    raise AssertionError(f"timeout: {label}; evidence {ROOT}")


def sidebar_pane(window):
    # Sidebar infrastructure only; agent screens/state/input use plane/index/native fixtures.
    lines = tmux("list-panes", "-t", window, "-F", "#{pane_id} #{@agent-plane-sidebar}")
    return next(line.split()[0] for line in lines.splitlines() if line.endswith(" 1"))


def raw_input_bytes(pane):
    path = ROOT / (pane + ".input")
    return path.read_bytes() if path.exists() else b""


def typed(pane):
    # Fixtures report focus. Moving a client between agents legitimately produces these.
    return raw_input_bytes(pane).replace(b"\x1b[I", b"").replace(b"\x1b[O", b"")


def input_bytes(name):
    return typed(agent(name)["pane"])


def select_row(client, name):
    # Use sidebar navigation: a fuzzy query can retain several matching rows.
    wait("row available " + name, lambda: name in shown_names(client) and
         client.selected() in shown_names(client))
    names = shown_names(client)
    steps = (names.index(name) - names.index(client.selected())) % len(names)
    client.key(b"\x1b[B" * steps)
    selected(client, name)


def query(client, value, select=None):
    client.key(b"f\x15" + value.encode())
    wait("editing query " + value, lambda: ("f " + value + "▏") in client.text())
    if select is not None:
        select_row(client, select)
    client.key(b"/")
    wait("query " + value, lambda: ("f " + value) in client.text() and "▏" not in client.text())


def selected(client, name):
    wait("selected " + name, lambda: client.selected() == name)


def previewed(client, name):
    def arrived():
        target = sidebar_pane(agent(name)["window"])
        context = client.context()
        return (context[1] == target and context[2] == agent(name)["window"] and
                client.selected() == name)
    wait("preview " + name, arrived)


def assert_vim_search(client):
    names = shown_names(client)
    origin = client.context()
    start = client.selected()
    # Typing moves the selection, ignores case, hides nothing and stays in this pane.
    client.key(b"/NAV-DONE")
    wait("search selects its match", lambda: client.selected() == "nav-done" and "/ NAV-DONE▏" in client.text())
    assert shown_names(client) == names and client.context() == origin
    client.key(b"\x1b")
    wait("search cancel restores the row", lambda: client.selected() == start and "/ search" in client.text())
    assert client.context() == origin
    # Enter keeps the search and previews the match, with the keyboard here; n and N go on.
    client.key(b"/nav-\r")
    wait("search kept", lambda: "▏" not in client.text() and "/ nav-" in client.text() and "next/prev" in client.footer())
    first = client.selected()
    previewed(client, first)
    client.key(b"n")
    wait("n moves on", lambda: client.selected() != first)
    second = client.selected()
    previewed(client, second)
    assert shown_names(client) == names
    client.key(b"N")
    wait("N moves back", lambda: client.selected() == first)
    previewed(client, first)
    # Ctrl-C cancels typing like Esc; here that restores the kept search.
    client.key(b"/zzz")
    wait("second search typed", lambda: "/ zzz▏" in client.text())
    client.key(b"\x03")
    wait("Ctrl-C cancels the edit", lambda: "▏" not in client.text() and "/ nav-" in client.text())
    # Ctrl-C ends a kept search too, and n is the new-agent key again.
    client.key(b"\x03")
    wait("search cleared", lambda: "/ search" in client.text())
    client.key(b"n")
    wait("n opens the new agent picker", lambda: "new agent in" in client.text())
    client.key(b"\x1b")
    wait("new agent cancelled", lambda: "new agent in" not in client.text())
    # Shift-Right opens the selection like Enter, also mid-search; Shift-Left stays or returns.
    sidebar = client.context()
    client.key(b"\x1b[1;2D")
    settle(.3)
    assert client.context() == sidebar, "the in key must stay in the sidebar"
    client.key(b"/nav-work")
    wait("search before leaving", lambda: client.selected() == "nav-work")
    client.key(b"\x1b[1;2C")
    wait("Shift-Right leaves for the agent", lambda: client.context()[1] == agent("nav-work")["pane"])
    client.key(b"\x1bh")
    wait("the in key returns to the sidebar", lambda: "/ search" in client.text() and client.context()[1] != agent("nav-work")["pane"])
    client.key(b"\x1bl")
    wait("the out key leaves", lambda: client.context()[1] == agent("nav-work")["pane"])
    client.key(b"\x1bh")
    wait("back in the sidebar", lambda: client.context()[1] != agent("nav-work")["pane"])
    return {"vim_search": True, "in_out_keys": True}


def assert_cycle_keys(client):
    """Exercise actual prefixless key delivery, live cursor selection and client guards."""
    def order():
        snapshot = index()
        agents = {a['pane']: a for a in snapshot['agents']}
        return sorted([agents[pane] for pane in snapshot['order'] if pane in agents],
                      key=lambda a: a['session'])

    def here():
        return client.context()[1]


    default_keys = {'next': b'\x1bj', 'previous': b'\x1bk'}
    keys = default_keys

    def hop(direction):
        ordered = [a['pane'] for a in order()]
        origin = here()
        if origin in ordered:
            delta = 1 if direction == 'next' else -1
            expected = ordered[(ordered.index(origin) + delta) % len(ordered)]
        else:
            expected = ordered[0 if direction == 'next' else -1]
        client.key(keys[direction])
        wait('direct key ' + direction, lambda: here() == expected)
        return expected

    bindings = tmux('list-keys', '-T', 'root')
    assert 'M-k' in bindings and 'cycle previous --client' in bindings
    assert 'M-j' in bindings and 'cycle next --client' in bindings
    before = {name: input_bytes(name) for name in FIXTURES}
    visited = {hop('next') for _ in range(4)}
    assert visited == {a['pane'] for a in order()}
    hop('previous')
    ordered = [a['pane'] for a in order()]
    origin = here()
    expected = ordered[(ordered.index(origin) + 11) % len(ordered)]
    client.key(b"\x1bj" * 11)
    wait('rapid keys advance from live pane', lambda: here() == expected)
    settle(0.7)
    assert here() == expected
    for name in FIXTURES:
        assert input_bytes(name) == before[name], (name, input_bytes(name))
    # Unmodified arrows keep reaching the terminal application.
    selected_name = next(a['name'] for a in index()['agents'] if a['pane'] == here())
    client.key(b'\x1b[A\x1b[B')
    wait('ordinary arrows reach the agent', lambda: b'\x1b[A\x1b[B' in input_bytes(selected_name))
    # A second client keeps its session and pane. Shared target sessions are refused.
    away = agent('nav-away').copy()
    if here() == away['pane']:
        hop('next')
    secondary = Client(away['session'])
    CLIENTS.append(secondary)
    wait('second client attached', lambda: len(tmux('list-clients', '-F', '#{client_name}').splitlines()) == 2)
    secondary_origin = secondary.context()
    # Drive only the first client until the next step would enter the occupied session.
    ordered = [a['pane'] for a in order()]
    predecessor = ordered[(ordered.index(away['pane']) - 1) % len(ordered)]
    for _ in range(len(ordered)):
        if here() == predecessor:
            break
        # Previous avoids crossing the occupied destination from the other side.
        hop('previous')
    assert here() == predecessor
    first_origin = client.context()
    denied = subprocess.run([str(BINARY), 'cycle', 'next', '--client', client.context()[0]],
                            env=ENV, text=True, capture_output=True, timeout=10)
    assert denied.returncode != 0 and 'shared with another client' in denied.stderr, denied
    assert client.context() == first_origin and secondary.context() == secondary_origin
    missing = subprocess.run([str(BINARY), 'cycle', 'next', '--client', 'missing-client'],
                             env=ENV, text=True, capture_output=True, timeout=10)
    assert missing.returncode != 0 and 'no longer attached' in missing.stderr
    secondary.close()
    CLIENTS.remove(secondary)
    hop('next')
    # Departure between index updates fails without guessing a different target.
    plane('kill', 'nav-done')
    SPAWNED.remove('nav-done')
    wait('departed row withdrawn', lambda: not any(a['name'] == 'nav-done' for a in index()['agents']))
    hop('next')
    hop('previous')
    # Direct bindings work when the sidebar is open, and keep focus in an agent.
    plane('toggle-sidebar', agent('nav-work')['window'])
    wait('sidebars open', lambda: all(w['sidebar'] for w in index()['windows']))
    hop('next')
    assert here() in [a['pane'] for a in order()]
    # Rebinding on plugin reload removes its old keys and preserves user replacements.
    tmux('set-option', '-g', '@agent-plane-previous-key', 'C-M-Up')
    tmux('set-option', '-g', '@agent-plane-next-key', 'C-M-Down')
    subprocess.run(['bash', str(SOURCE.parent / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    bindings = tmux('list-keys', '-T', 'root')
    assert not any(' M-k ' in row or ' M-j ' in row for row in bindings.splitlines())
    keys = {'next': b'\x1b[1;7B', 'previous': b'\x1b[1;7A'}
    hop('next')
    hop('previous')
    tmux('bind-key', '-n', 'C-M-Up', 'display-message', 'user-owned-binding')
    tmux('set-option', '-gu', '@agent-plane-previous-key')
    tmux('set-option', '-gu', '@agent-plane-next-key')
    subprocess.run(['bash', str(SOURCE.parent / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    bindings = tmux('list-keys', '-T', 'root')
    assert any('C-M-Up' in row and 'user-owned-binding' in row for row in bindings.splitlines())
    assert not any('C-M-Down' in row for row in bindings.splitlines())
    keys = default_keys
    hop('next')
    client.snapshot('cycle-keys')
    return {'prefixless_option_keys': True, 'plain_arrows_preserved': True,
            'cross_session_wrap': True, 'rapid_keys': 11, 'no_modifier_input_leak': True,
            'explicit_client': True, 'shared_target_refused': True, 'departed_row': True,
            'works_with_sidebar_open': True, 'custom_keys_reload': True,
            'user_rebinding_preserved': True}


def bold_names(client):
    """The agents the sidebar shows in bold."""
    client.drain()
    found = []
    for row, line in enumerate(client.screen.display):
        if "─" in line[:38]:  # A session heading is always bold.
            continue
        for name in FIXTURES:
            column = line[:38].find(name)
            if column >= 0 and client.screen.buffer[row][column].bold:
                found.append(name)
    return found


def assert_bold_on_arrival(client):
    """Bold is right as soon as you land on an agent, well inside the daemon's tick."""
    limit = 0.25

    def here():
        return client.context()[1]

    def bold_soon(label, name):
        start = time.monotonic()
        wait(label, lambda: here() == agent(name)["pane"], timeout=5)
        arrived = time.monotonic()
        while bold_names(client) != [name]:
            assert time.monotonic() - arrived < limit, (label, bold_names(client), ROOT)
            time.sleep(0.005)
        return round((time.monotonic() - arrived) * 1000, 1), round((arrived - start) * 1000, 1)

    if not all(w["sidebar"] for w in index()["windows"]):
        plane("toggle-sidebar", agent("nav-work")["window"])
    wait("sidebars open", lambda: all(w["sidebar"] for w in index()["windows"]))
    plane("focus", "nav-work")
    timings = {}
    for _ in range(3):
        settle(2.0)  # Hidden sidebars have read the index, as they have in normal use.
        panes = {a["pane"]: a["name"] for a in index()["agents"]}
        snapshot = index()
        ordered = sorted([a for pane in snapshot["order"] for a in snapshot["agents"] if a["pane"] == pane],
                         key=lambda a: a["session"])
        position = [a["pane"] for a in ordered].index(here())
        name = panes[ordered[(position + 1) % len(ordered)]["pane"]]
        client.key(b"\x1bj")
        timings["alt-j " + name] = bold_soon("alt-j " + name, name)
    # Enter from the sidebar: the sidebar comes along and knows where it landed.
    settle(2.0)
    home = agent("nav-work")
    own = sidebar_pane(home["window"])
    tmux("switch-client", "-c", client.context()[0], "-t", f"={home['session']}:{home['window']}.{own}")
    wait("in the sidebar", lambda: here() == own)
    settle(2.0)
    assert bold_names(client) == [], bold_names(client)  # The keyboard is in the sidebar.
    before = client.selected()
    client.key(b"j")
    wait("preview moved", lambda: client.selected() != before)
    settle(0.3)
    assert bold_names(client) == [], bold_names(client)  # A preview keeps the keyboard here.
    name = client.selected()
    client.key(b"\r")
    timings["enter " + name] = bold_soon("enter " + name, name)
    print("bold after arrival, ms (bold, arrival):", timings, flush=True)
    return {"bold_on_arrival": timings}


def assert_fuzzy_search(client):
    baseline = shown_names(client)
    origin = client.context()
    def edit(value):
        client.key(b"\x15" + value.encode())
        wait("editing " + value, lambda: ("f " + value + "▏") in client.text())
    # The picker's search: name, project and description, in any word order, best first.
    client.key(b"f")
    edit("NPMTRVW NVWRK")  # Description, then name.
    wait("fuzzy best match first", lambda: shown_names(client)[0] == "nav-work" and client.selected() == "nav-work")
    edit("NAV-AWAY RVFXPRM")  # Name and the displayed blocked description.
    wait("fuzzy blocked description", lambda: shown_names(client) == ["nav-away"])
    edit(agent("nav-away")["session"])  # Project.
    wait("fuzzy project", lambda: "nav-away" in shown_names(client))
    assert client.context() == origin
    client.key(b"\x1b")
    wait("fuzzy cancel", lambda: "f filter" in client.text() and shown_names(client) == baseline)
    selected(client, "nav-work")
    # Freezing previews its selection; the filter then composes with the kept query.
    query(client, "NAV-AWAY RVFXPRM")
    previewed(client, "nav-away")
    client.key(b"t")
    wait("fuzzy blocked filter", lambda: "t blocked" in client.text())
    query(client, "NVWRK NPMTRVW")
    wait("fuzzy empty filter", lambda: "no matching agents" in client.text() and
         "t blocked" in client.text())
    client.key(b"t")
    wait("fuzzy skips empty states", lambda: "t working" in client.text() and
         shown_names(client) == ["nav-work"])
    previewed(client, "nav-work")
    client.key(b"t")
    wait("fuzzy filter all", lambda: "t all" in client.text())
    client.key(b"\x1b")
    wait("fuzzy Escape clears search", lambda: "f filter" in client.text() and
         shown_names(client) == baseline)
    selected(client, "nav-work")
    # Editing and browsing a search stay in this pane.
    editing_origin = client.context()
    client.key(b"f\x15NV NPMTRVW")
    wait("fuzzy editing", lambda: "f NV NPMTRVW▏" in client.text())
    before = client.selected()
    client.key(b"\x1b[B")
    wait("fuzzy browse", lambda: client.selected() != before)
    assert client.context() == editing_origin
    client.key("☃".encode())
    wait("fuzzy incremental miss", lambda: "no matching agents" in client.text())
    client.key(b"\x7f")
    wait("fuzzy backspace restores rows", lambda: sorted(shown_names(client)) == sorted(baseline))
    client.key(b"\x17")
    wait("fuzzy delete word", lambda: "f NV " in client.text())
    client.key(b"\x15")
    wait("fuzzy clear edit", lambda: "f ▏" in client.text())
    assert client.context() == editing_origin
    client.key(b"\x1b")
    wait("fuzzy cancel restores selection", lambda: "f filter" in client.text()
         and client.selected() == "nav-work")
    assert client.context() == editing_origin
    assert agent("nav-done")["state"] == "done"
    client.key(b"f\x15NVDN NPMTRVW")
    wait("fuzzy final edit stays local", lambda: "▏" in client.text() and client.selected() == "nav-done")
    assert client.context() == editing_origin
    client.key(b"/")
    previewed(client, "nav-done")
    assert agent("nav-done")["state"] == "done", "preview must leave completion unseen"
    client.snapshot("fuzzy-accepted-preview")
    client.key(b"\x1b")
    wait("fuzzy restores rows", lambda: "f filter" in client.text() and
         shown_names(client) == baseline)
    selected(client, "nav-done")
    client.key(b"f\x15NVWRK/")
    previewed(client, "nav-work")
    client.key(b"\x1b")
    wait("back on all rows", lambda: "f filter" in client.text() and shown_names(client) == baseline)
    selected(client, "nav-work")
    for pane in FIXTURES.values():
        assert typed(pane) == b"", "search sent input to an agent"
    return {"fuzzy_name_project_description": True, "fuzzy_any_word_order_best_first": True,
            "fuzzy_filter_composition": True,
            "fuzzy_incremental_edit_cancel_clear": True, "fuzzy_preview_pause_and_freeze": True,
            "unintended_agent_input": 0, "models_used": 0}


def frames_after(client, action, seconds=1.0):
    """Every screen the terminal is left on, as (window, selected). A frame ends when tmux pauses."""
    client.drain()
    frames = []
    action() if callable(action) else client.key(action)
    start = time.monotonic()
    while time.monotonic() < start + seconds:
        if not select.select([client.master], [], [], 0.05)[0]:
            continue
        while select.select([client.master], [], [], 0.003)[0]:
            client.stream.feed(os.read(client.master, 65536))
        frame = (client.screen.display[0].split()[-1], client.selected())
        if not frames or frames[-1][1] != frame:
            frames.append((round(time.monotonic() - start, 3), frame))
    return frames


def screens_during(client, action, seconds):
    """Every screen the terminal is left on while action runs, as (window, selected, rows)."""
    client.drain()
    screens = []
    finished = action()
    until = None
    while until is None or time.monotonic() < until:
        if until is None and finished():
            until = time.monotonic() + seconds
        if not select.select([client.master], [], [], 0.05)[0]:
            continue
        # tmux paints in pieces and can defer the status line while output is backed up, so a
        # read can land mid-paint. A wrong highlight stays until the next hop, so a silence
        # shorter than the hops separates the two.
        while select.select([client.master], [], [], 0.04)[0]:
            client.stream.feed(os.read(client.master, 65536))
        screen = (client.screen.display[0].split()[-1], client.selected(), shown_names(client))
        if not screens or screens[-1] != screen:
            screens.append(screen)
    return screens


def assert_quick_hops(client, names, rounds=8):
    # Plain tmux, as fast as it goes: no sidebar can have read anything new, and none needs to.
    # tmux's own copy of the sidebar is what it paints from, and unlike a terminal reading it can
    # never catch a half-finished paint, so this needs no timing and holds on a busy machine.
    settle(1.7)
    rows = shown_names(client)
    client_name = client.context()[0]
    socket = [TMUX, "-S", str(ROOT / "t.sock")]
    targets = [(name, agent(name)["window"], agent(name)["session"], agent(name)["pane"],
                sidebar_pane(agent(name)["window"])) for name in names]
    hops = 0
    for _ in range(rounds):
        for name, window, session, pane, sidebar in targets:
            subprocess.run([*socket, "switch-client", "-c", client_name, "-t", f"={session}:{window}.{pane}"],
                           env=ENV, check=True, capture_output=True, timeout=8)
            lines = subprocess.run([*socket, "capture-pane", "-p", "-t", sidebar], env=ENV, check=True,
                                   capture_output=True, text=True, timeout=8).stdout.splitlines()
            shown = next(line[1:38].split()[1] for line in lines if line[1:2] == "▌")
            assert shown == name, (window, shown, name)
            assert rows_in(lines) == rows, (rows, rows_in(lines))
            client.drain()
            hops += 1
    (ROOT / "quick-hops.json").write_text(json.dumps({"hops": hops, "rows": rows}) + "\n")
    return hops


def assert_left_windows_show_their_agents(client, names):
    # The daemon reports what tmux has: every window but the client's has its agent active.
    current = client.context()[2]
    def restored():
        views = {view["id"]: view for view in index()["windows"]}
        return all(views[agent(name)["window"]]["active_pane"] == agent(name)["pane"]
                   for name in names if agent(name)["window"] != current)
    wait("left windows have their agent active", restored)


def assert_lone_sidebar_closes():
    window = tmux("new-window", "-d", "-t", "=scratch:", "-P", "-F", "#{window_id}", "/bin/sh")
    plane("follow-sidebar", window)
    sidebar = sidebar_pane(window)
    panes = tmux("list-panes", "-t", window, "-F", "#{pane_id}").split()
    shell = next(pane for pane in panes if pane != sidebar)
    started = time.monotonic()
    tmux("kill-pane", "-t", shell)
    wait("lone sidebar's window closes",
         lambda: window not in tmux("list-windows", "-a", "-F", "#{window_id}").split(), timeout=0.3)
    assert time.monotonic() - started < 0.3, "layout hook closes it without a daemon tick"
    assert "sidebar:close" not in (STATE / "daemon.log").read_text()


def assert_sort_everywhere(client, names):
    # One press: on screen at once, never undone while the daemon catches up, then everywhere.
    sidebar = client.context()[1]
    selection = client.selected()
    change("nav-work", "blocked", 0)
    # `s` sorts what is on screen, so wait for the badge rather than the index.
    wait("new attention", lambda: any("nav-work" in line and "blocked" in line
                                      for line in client.text().splitlines()))
    before = shown_names(client)
    expected = ranked_names()
    assert expected != before, expected
    pressed = time.monotonic()

    def press():
        client.key(b"s")
        return lambda: time.monotonic() > pressed + 2.5

    screens = screens_during(client, press, 0)
    (ROOT / "sort-frames.json").write_text(json.dumps(screens, indent=2) + "\n")
    first = next(i for i, screen in enumerate(screens) if screen[2] == expected)
    assert all(screen[2] == expected for screen in screens[first:]), screens
    assert client.selected() == selection
    panes = [agent(name)["pane"] for name in expected]
    wait("daemon publishes the order", lambda: index()["order"] == panes)
    assert_sidebars_agree_on_arrival(client, names)
    tmux("switch-client", "-c", client.context()[0], "-t", sidebar)
    wait("back after sort", lambda: client.context()[1] == sidebar)
    assert shown_names(client) == expected
    # Back to the states the rest of this proof starts from.
    change("nav-work", "working", 0)
    wait("working again", lambda: any("nav-work" in line and "working" in line
                                      for line in client.text().splitlines()))
    client.key(b"s")
    wait("sorted again", lambda: shown_names(client) == ranked_names())


def assert_idle_runs_only_the_daemon(seconds=10):
    # Windows full of sidebars, nothing changing: all that runs is the daemon's one snapshot.
    settle(2.5)
    before = len((ROOT / "tmux.calls").read_text().splitlines())
    settle(seconds)
    calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()[before:]]
    (ROOT / "idle-calls.json").write_text(json.dumps(calls, indent=2) + "\n")
    assert all(call[:2] == ["list-panes", "-a"] for call in calls), calls
    assert seconds - 2 <= len(calls) <= seconds + 2, len(calls)
    return len(calls)


def assert_sidebars_agree_on_arrival(client, names):
    # Moving with plain tmux lands on other sidebars: same rows, current agent, in every frame.
    # That needs the agent active on arrival. A window whose keyboard was left in its sidebar
    # shows the last agent used, and selecting a pane on arrival is news to its sidebar.
    # Start outside them all: a move within the client's own window is no arrival.
    if client.context()[2] in [agent(name)["window"] for name in names]:
        tmux("switch-client", "-c", client.context()[0], "-t", "=scratch:0")
    for name in names:
        tmux("select-pane", "-t", agent(name)["pane"])
    settle(1.7)
    rows = shown_names(client)
    trace = []
    for name in names:
        window = agent(name)["window"]
        session = agent(name)["session"]
        client_name = client.context()[0]
        frames = frames_after(client, lambda: tmux("switch-client", "-c", client_name, "-t",
                                                   f"={session}:{window}.{agent(name)['pane']}"))
        trace.append({"name": name, "window": window, "rows": shown_names(client),
                      "frames": [[at, *frame] for at, frame in frames]})
        (ROOT / "arrival-frames.json").write_text(json.dumps(trace, indent=2) + "\n")
        for _, (shown_window, shown) in frames:
            assert shown_window != window or shown == name, (name, frames)
        assert frames[-1][1] == (window, name), frames
        assert shown_names(client) == rows, (rows, shown_names(client))
        wait("last agent used " + name, lambda name=name: index()["last_agent"] == agent(name)["pane"])


def assert_no_stale_preview_frame(client):
    # The selection on screen must always be the one you chose, never a stale row in the new window.
    names = shown_names(client)
    sidebar = client.context()[1]
    trace = []
    calls_before = len((ROOT / "tmux.calls").read_text().splitlines())
    for _ in names:
        before = client.selected()
        expected = names[(names.index(before) + 1) % len(names)]
        window = agent(expected)["window"]
        frames = frames_after(client, b"j")
        trace.append({"before": before, "expected": expected, "window": window,
                      "frames": [[at, *frame] for at, frame in frames]})
        (ROOT / "preview-frames.json").write_text(json.dumps(trace, indent=2) + "\n")
        for _, (shown_window, shown) in frames:
            assert shown_window != window or shown == expected, (
                f"stale frame: {shown} selected in {window} while previewing {expected}: {frames}")
        assert frames[-1][1] == (window, expected), frames
        previewed(client, expected)
        assert client.context()[1] == sidebar, "the sidebar pane itself must travel with the client"
    # Each tmux command costs a process: one preview is one combined read and one swap.
    calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()[calls_before:]]
    assert sum(call[0] == "swap-pane" for call in calls) == len(names), calls
    assert sum(call[0] == "list-clients" and ";" in call for call in calls) == len(names), calls
    assert not any(call[0] in ("display-message", "list-panes") and "-t" in call and
                   call[call.index("-t") + 1].startswith("=") for call in calls), calls


def settle(seconds=1.2):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        for client in CLIENTS:
            client.drain()
        time.sleep(0.03)


def rows_in(lines):
    # Decode full physical rows: tmux can scale a detached window's sidebar on attach.
    # Borders + icon + separate state/age fields exclude headings, controls and notices.
    return [match.group(1) for line in lines
            if (match := re.match(
                r'^│[ ▌][◆✓●○] (nav-[a-z]+)\s+(blocked|done|working|idle)\s+\S+\s*│', line))]


def shown_names(client):
    client.drain()
    return rows_in(client.screen.display)


def extra_checks(client):
    # Begin from a stable command mode.
    base_window = tmux('display-message', '-p', '-t', '=scratch:0', '#{window_id}')
    base_sidebar = sidebar_pane(base_window)
    tmux('switch-client', '-c', client.context()[0], '-t', base_sidebar)
    wait('stable base sidebar', lambda: client.context()[1] == base_sidebar and 't all' in client.text())
    query(client, '')
    client.key(b's')
    wait('initial attention ranking', lambda: shown_names(client) == ranked_names())
    initial = shown_names(client)
    # New blocked priority must update its badge while leaving its original row in place.
    change('nav-idle', 'blocked', 0)
    wait('live blocked badge latched', lambda: agent('nav-idle')['state'] == 'blocked' and
         any('nav-idle' in line and 'blocked' in line for line in client.text().splitlines()))
    assert shown_names(client) == initial
    def idle_badge():
        client.drain()
        return next(line for line in client.screen.display
                    if re.match(r'^│[ ▌]◆ nav-idle\s+blocked\s+', line))
    before_age = idle_badge()
    settle(1.1)
    assert shown_names(client) == initial and idle_badge() != before_age
    client.snapshot('sort-latched-live-badges')
    # Oldest blocked in this session ranks first, independent of tmux location.
    change('nav-done', 'blocked', 3)
    wait('second blocked', lambda: agent('nav-done')['state'] == 'blocked')
    assert shown_names(client) == initial
    selection = client.selected()
    client.key(b's')
    wait('explicit S ranks oldest blocked', lambda: shown_names(client) == ranked_names())
    assert client.selected() == selection
    expected = shown_names(client)
    assert expected.index('nav-idle') < expected.index('nav-done')
    change('nav-work', 'working', 0)
    wait('working badge stays latched', lambda: agent('nav-work')['state'] == 'working' and
         any('nav-work' in line and 'working' in line for line in client.text().splitlines()))
    assert shown_names(client) == expected
    # Leave for the nonagent base shell. Unfocused refresh and re-entry keep the order.
    tmux('select-pane', '-t', '=scratch:0.1')
    settle(0.7)
    assert shown_names(client) == expected
    tmux('switch-client', '-c', client.context()[0], '-t', base_sidebar)
    wait('fresh keyboard visit', lambda: client.context()[1] == base_sidebar)
    settle(0.7)
    assert shown_names(client) == expected
    client.snapshot('sort-unfocused-and-reentry')
    plane('spawn', 'pi', '--in', 'window', '--session', 'scratch', '--cwd', str(ROOT), '--name', 'nav-new')
    SPAWNED.append('nav-new')
    plane('follow-sidebar', agent('nav-new')['window'])
    change('nav-new', 'blocked', 0)
    wait('arrival appended within session', lambda: agent('nav-new')['state'] == 'blocked' and
         shown_names(client) == expected + ['nav-new'])
    plane('kill', 'nav-new')
    SPAWNED.remove('nav-new')
    wait('removed row disappears', lambda: shown_names(client) == expected)
    # Search acceptance and preview both keep the latched order.
    query(client, 'nav-', select=selection)
    wait('search acceptance keeps order', lambda: shown_names(client) == expected)
    previewed(client, selection)
    wait('preview keeps order', lambda: shown_names(client) == expected)
    client.key(b't')
    blocked = [name for name in expected if agent(name)['state'] == 'blocked']
    wait('V search and blocked filter compose', lambda: 't blocked' in client.text() and
         shown_names(client) == blocked and 'f nav-' in client.text())
    previewed(client, selection)
    client.key(b't')
    previewed(client, 'nav-work')
    wait('filtered preview carries query and working order', lambda: 't working' in client.text() and
         'f nav-' in client.text() and shown_names(client) == ['nav-work'])
    client.key(b't')  # Idle is empty; deliberate F skips it and returns to All.
    wait('filtered preview returns to all rows', lambda: 't all' in client.text() and
         shown_names(client) == expected)
    client.snapshot('sort-preview-order')
    assert client.context()[1] == base_sidebar, 'the previewing sidebar travels with the client'
    # The sidebar it swapped with now sits in the base window, with its own settings.
    base_sidebar = sidebar_pane(base_window)
    assert base_sidebar != client.context()[1]
    tmux('switch-client', '-c', client.context()[0], '-t', base_sidebar)
    wait('base sidebar', lambda: client.context()[1] == base_sidebar and 't all' in client.text())
    # Existing filter/navigation proof covers F/search composition in detail.
    query(client, '')
    settle(0.7)
    baseline = shown_names(client)
    selection = client.selected()
    context = client.context()
    # Every command class cancels and consumes its key. No movement, mode/filter/query,
    # rename or close action may be replayed after the confirmation disappears.
    for cancel in [b'n', b'\x1b', b'j', b'\x1b[B', b'G', b't', b'f', b's', b'r', b'q', b'x', b'Y', b'/', b'\x19']:
        client.key(b'x')
        wait('kill prompt', lambda: '? [Y/n]' in client.text())
        client.key(cancel)
        wait('cancel consumes key', lambda: '? [Y/n]' not in client.text())
        assert client.selected() == selection and client.context() == context
        assert shown_names(client) == baseline and 't all' in client.text()
        assert 'f nav-' not in client.text() and 'save' not in client.footer()
    client.snapshot('kill-cancel-no-replay')
    # Native state and identity are validated independently of daemon refresh.
    query(client, 'nav-idle', select='nav-idle')
    client.key(b'x')
    wait('state pin prompt', lambda: '? [Y/n]' in client.text())
    change('nav-idle', 'working', 0)
    settle(0.15)
    client.key(b'y')
    wait('state pin refused', lambda: 'changed' in client.footer())
    assert agent('nav-idle')
    wait('new state indexed', lambda: agent('nav-idle')['state'] == 'working')
    settle(0.7)
    client.key(b'x')
    wait('identity pin prompt', lambda: '? [Y/n]' in client.text())
    change('nav-idle', 'working', 0, identity=1)
    settle(0.15)
    client.key(b'y')
    wait('native identity refused', lambda: 'changed' in client.footer())
    wait('new identity indexed', lambda: agent('nav-idle')['session_id'].endswith('-1'))
    settle(0.7)
    client.key(b'x')
    wait('rename pin prompt', lambda: '? [Y/n]' in client.text())
    plane('rename', 'nav-idle', 'nav-renamed')
    SPAWNED[SPAWNED.index('nav-idle')] = 'nav-renamed'
    client.key(b'y')
    wait('rename pin refused', lambda: 'changed' in client.footer() or 'renamed' in client.footer())
    wait('rename indexed', lambda: agent('nav-renamed'))
    plane('rename', 'nav-renamed', 'nav-idle')
    SPAWNED[SPAWNED.index('nav-renamed')] = 'nav-idle'
    wait('rename restored', lambda: agent('nav-idle'))
    settle(0.7)
    client.snapshot('kill-state-identity-rename-refused')
    # A tty/PID may stay identical while its session changes through a linked window.
    # The confirmation belongs to its original client context, even on the same pane.
    query(client, 'nav-idle', select='nav-idle')
    client.key(b'x')
    wait('client context pin prompt', lambda: '? [Y/n]' in client.text())
    pinned_context = client.context()
    tmux('new-session', '-d', '-s', 'zzz-context', '-t', 'scratch')
    tmux('switch-client', '-c', pinned_context[0], '-t',
         f'=zzz-context:{pinned_context[2]}.{pinned_context[1]}')
    client.key(b'y')
    wait('client session change refused', lambda: 'client changed' in client.footer())
    assert agent('nav-idle')
    client.snapshot('kill-client-session-refused')
    tmux('switch-client', '-c', client.context()[0], '-t',
         f'=scratch:{pinned_context[2]}.{pinned_context[1]}')
    # No query: kill successive rows in one PTY burst; selection advances immediately,
    # even while the daemon still contains a just-closed pane.
    query(client, '')
    client.key(b's')
    settle(0.25)
    names = shown_names(client)
    # Keep nav-idle for the original rename-exit proof. Select an adjacent pair that
    # does not include it or the remote target (remote warning gets its own test).
    first = min('nav-work', 'nav-done', key=names.index)
    while client.selected() != first:
        client.key(b'j')
        settle(0.1)
    position = names.index(first)
    second = names[position + 1] if position + 1 < len(names) else names[position - 1]
    # Only assert/destructively exercise if both are the intended local pair.
    assert {first, second} == {'nav-work', 'nav-done'}, (names, first, second)
    panes = {name: agent(name)['pane'] for name in (first, second)}
    killing_from = client.context()[1]
    client.key(b'x\rx\r')
    wait('x Enter x Enter closes exactly both targets', lambda: all(
        not any(a['pane'] == pane for a in index()['agents']) for pane in panes.values()))
    for name in (first, second):
        SPAWNED.remove(name)
    assert client.selected() == 'nav-idle'
    assert client.context()[1] == killing_from, 'the keyboard stays in this sidebar'
    client.snapshot('kill-enter-next-visible')
    # Final session warning plus lowercase y confirmation.
    # A next row to move to, so the session you are in can end.
    query(client, 'nav-away', select='nav-away')
    client.key(b'\x1b')
    wait('next row back', lambda: 'f filter' in client.text())
    client.key(b'x')
    wait('named remote last-session warning', lambda: 'ends session nav-away? [Y/n]' in client.text())
    client.snapshot('kill-remote-session-warning')
    extra_pane = tmux('new-window', '-d', '-t', '=nav-away:', '-P', '-F', '#{pane_id}', '/bin/sh')
    client.key(b'y')
    wait('changed session ending invalidates confirmation', lambda: 'session/window changed' in client.footer())
    assert agent('nav-away')
    tmux('kill-pane', '-t', extra_pane)  # Private nonagent fixture infrastructure only.
    client.key(b'x')
    wait('remote warning after topology restored', lambda: 'ends session nav-away? [Y/n]' in client.text())
    client.key(b'y')
    wait('remote y close', lambda: not any(a['name'] == 'nav-away' for a in index()['agents']))
    SPAWNED.remove('nav-away')
    wait('remote final session disappears', lambda: 'nav-away' not in tmux('list-sessions', '-F', '#{session_name}').splitlines())
    # A disappearing target cannot redirect y to the remaining agent.
    plane('spawn', 'pi', '--in', 'window', '--cwd', str(ROOT), '--name', 'nav-exit')
    SPAWNED.append('nav-exit')
    plane('follow-sidebar', agent('nav-exit')['window'])
    query(client, 'nav-exit', select='nav-exit')
    selected(client, 'nav-exit')
    exiting_window = agent('nav-exit')['window']
    # A second pane keeps the window, and the sidebar in it, when the agent exits under the prompt.
    tmux('split-window', '-h', '-d', '-t', agent('nav-exit')['pane'], '/bin/sh')
    client.key(b'x')
    wait('exit pin prompt', lambda: '? [Y/n]' in client.text())
    plane('kill', 'nav-exit')
    SPAWNED.remove('nav-exit')
    calls_before_exit_confirmation = len((ROOT / 'tmux.calls').read_text().splitlines())
    client.key(b'y')
    wait('exited pin refused', lambda: '? [Y/n]' not in client.text() and
         ('exited' in client.footer() or 'changed' in client.footer() or
          ('tmux list-panes -t ' + exiting_window) in client.footer()))
    exit_confirmation_calls = [json.loads(line) for line in
        (ROOT / 'tmux.calls').read_text().splitlines()[calls_before_exit_confirmation:]]
    assert not any(call[0] == 'kill-pane' for call in exit_confirmation_calls)
    assert agent('nav-idle')
    # Linked final sessions: same physical pane appears in both session topologies.
    plane('spawn', 'pi', '--in', 'session', '--cwd', str(ROOT), '--name', 'nav-linked')
    SPAWNED.append('nav-linked')
    linked = agent('nav-linked').copy()
    plane('follow-sidebar', linked['window'])
    tmux('new-session', '-d', '-s', 'linked-copy', '-t', 'nav-linked')
    wait('linked pane tracked once at canonical location', lambda:
         agent('nav-linked')['session'] == 'linked-copy' and
         sum(a['pane'] == linked['pane'] for a in index()['agents']) == 1)
    linked_client = Client('nav-linked')
    CLIENTS.append(linked_client)
    wait('visible through noncanonical session link', lambda: agent('nav-linked')['visible'])
    linked_client.snapshot('linked-visible-through-other-session')
    assert agent('nav-linked')['session'] == 'linked-copy'
    linked_client.close()
    CLIENTS.remove(linked_client)
    settle(0.7)
    query(client, 'nav-linked', select='nav-linked')
    selected(client, 'nav-linked')
    client.key(b'\x1b')
    wait('next row back for linked', lambda: 'f filter' in client.text())
    client.key(b'x')
    wait('linked remote warning', lambda: 'ends session' in client.text() and '? [Y/n]' in client.text())
    client.snapshot('kill-linked-remote-warning')
    client.key(b'y')
    wait('linked remote close', lambda: not any(a['pane'] == linked['pane'] for a in index()['agents']))
    SPAWNED.remove('nav-linked')
    wait('both linked final sessions disappear', lambda: not {'nav-linked', 'linked-copy'}.intersection(
        tmux('list-sessions', '-F', '#{session_name}').splitlines()))
    # With no other row to move to, refuse the attached final session, including the
    # sidebar-alone consequence.
    plane('spawn', 'pi', '--in', 'session', '--cwd', str(ROOT), '--name', 'nav-final')
    SPAWNED.append('nav-final')
    final = agent('nav-final').copy()
    plane('follow-sidebar', final['window'])
    tmux('switch-client', '-c', client.context()[0], '-t', sidebar_pane(final['window']))
    tmux('new-session', '-d', '-s', 'zzz-final-link', '-t', 'nav-final')
    query(client, 'nav-final', select='nav-final')
    selected(client, 'nav-final')
    client.key(b'x')
    wait('attached final session refused', lambda: 'attached session' in client.footer())
    client.snapshot('kill-attached-final-refused')
    assert agent('nav-final')
    tmux('switch-client', '-c', client.context()[0], '-t', context[1])
    plane('kill', 'nav-final')
    SPAWNED.remove('nav-final')
    # Restore the remaining target and normal sidebar width for the original final tests.
    plane('spawn', 'pi', '--in', 'window', '--cwd', str(ROOT), '--name', 'nav-survivor')
    SPAWNED.append('nav-survivor')
    plane('follow-sidebar', agent('nav-survivor')['window'])
    query(client, 'nav-idle', select='nav-idle')
    tmux('resize-pane', '-t', client.context()[1], '-x', '20')
    client.key(b'x')
    wait('narrow confirmation readable', lambda: '(working)? [Y/n]' in client.text())
    client.snapshot('kill-narrow-confirmation')
    client.key(b'n')
    wait('narrow cancelled', lambda: '? [Y/n]' not in client.text())
    tmux('resize-pane', '-t', client.context()[1], '-x', '38')
    assert not any(json.loads(line)[0] == 'kill-session' for line in (ROOT / 'tmux.calls').read_text().splitlines())
    return {'latched_sort_live_badges': True, 'oldest_state_first': True,
            'arrivals_removals': True, 'unfocused_and_reentry_rank': True, 'S_preserves_selection': True, 'search_v_keeps_order': True, 'V_search_F_combination': True,
            'kill_cancel_without_replay': True, 'kill_native_state_identity_rename_exit_pin': True,
            'kill_x_Enter_x_Enter_next_visible': True, 'kill_remote_y': True, 'kill_topology_change_pin': True,
            'kill_linked_sessions': True, 'kill_client_session_change_pin': True, 'kill_attached_final_refusal': True, 'kill_session_commands': 0}


SPAWNED = []
FIXTURES = {}
RESULTS = {}
try:
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "scratch", "-x", "100", "-y", "28", "/bin/sh")
    tmux("set-option", "-g", "default-shell", "/bin/sh")
    tmux("set-option", "-g", "default-command", "/bin/sh")
    tmux("set-option", "-g", "status-position", "top")
    tmux("set-option", "-g", "status-format[0]", "TOP-STATUS #{window_id}")
    tmux("set-option", "-as", "terminal-features", ",xterm-256color:RGB")
    tmux("set-option", "-s", "focus-events", "on")  # So a stray focus event reaches a fixture.
    subprocess.run(["bash", str(SOURCE.parent / "tmux-agent-plane.tmux")], env=ENV, check=True)
    # This suite controls sidebar arrivals explicitly; retain the plugin's close hook.
    for hook in ("session-created", "window-linked"):
        tmux("set-hook", "-gu", hook + "[90]")
    wait("daemon index", lambda: (STATE / "agents.json").exists())
    for name, placement in [("nav-work", "window"), ("nav-done", "window"), ("nav-idle", "window"), ("nav-away", "session")]:
        plane("spawn", "pi", "--in", placement, "--cwd", str(ROOT), "--name", name)
        SPAWNED.append(name)
    wait("native fixtures", lambda: len(index()["agents"]) == 4)
    CLIENTS.append(Client("scratch"))
    client = CLIENTS[0]
    wait("client attached", lambda: tmux("list-clients", "-F", "#{client_name}"))
    client.key(b"\x1b[I")  # The terminal has focus.
    change("nav-idle", "idle", 0)
    plane("focus", "nav-idle")
    wait("idle seen", lambda: agent("nav-idle")["state"] == "idle")
    plane("focus", "nav-work")
    change("nav-done", "idle", 1)
    change("nav-away", "blocked", 0)
    wait("states", lambda: agent("nav-done")["state"] == "done" and agent("nav-away")["state"] == "blocked")
    if "--cycle-only" in sys.argv[2:]:
        RESULTS = {"passed": True, **assert_cycle_keys(client)}
        sys.exit(0)
    if "--bold-only" in sys.argv[2:]:
        RESULTS = {"passed": True, **assert_bold_on_arrival(client)}
        sys.exit(0)
    plane("toggle-sidebar", agent("nav-work")["window"])
    wait("sidebar controls", lambda: "t all" in client.text())
    selected(client, "nav-work")  # the agent it was opened from
    vim_results = assert_vim_search(client)
    selected(client, "nav-work")
    fuzzy_results = {**assert_fuzzy_search(client), **vim_results}
    if "--search-only" in sys.argv[2:]:
        RESULTS = {"passed": True, **fuzzy_results}
        sys.exit(0)
    start_sidebar = client.context()[1]
    client.key(b"s")
    wait("attention order", lambda: shown_names(client) == ranked_names())
    assert_sidebars_agree_on_arrival(client, ["nav-work", "nav-idle", "nav-away"])
    hop_screens = assert_quick_hops(client, ["nav-work", "nav-idle", "nav-away"])
    idle_calls = assert_idle_runs_only_the_daemon()
    tmux("switch-client", "-c", client.context()[0], "-t", start_sidebar)
    wait("in the first sidebar", lambda: client.context()[1] == start_sidebar)
    selected(client, "nav-away")  # the last agent used
    assert_sort_everywhere(client, ["nav-work", "nav-idle", "nav-away"])
    selected(client, "nav-away")  # the last agent used
    original = client.context()
    client.key(b"j")
    wait("normal move", lambda: client.selected() != "nav-away")
    query(client, "nav-work", select="nav-work")
    selected(client, "nav-work")
    tmux("refresh-client", "-f", "active-pane", "-t", client.context()[0])
    flags = tmux("list-clients", "-F", "#{client_pid} #{client_flags}")
    assert "active-pane" in next(line for line in flags.splitlines() if line.split()[0] == str(client.process.pid))
    previewed(client, "nav-work")  # same window, sidebar remains active
    client.snapshot("same-window")
    # Clear query while viewing, then fast jjj. Exact net movement, one final switch.
    client.key(b"\x1b")
    wait("query cleared before burst", lambda: "f filter" in client.text())
    selected(client, "nav-work")
    ordered = ranked_names()
    expected = ordered[(ordered.index("nav-work") + 3) % len(ordered)]
    client.key(b"jjj")
    previewed(client, expected)
    client.snapshot("burst-jjj")
    # Keys delivered while switch commands are still in flight must also survive.
    start = expected
    for _ in range(7):
        client.key(b"j")
        time.sleep(0.015)
    expected = ordered[(ordered.index(start) + 7) % len(ordered)]
    previewed(client, expected)
    settle(0.7)
    assert client.selected() == expected
    client.snapshot("timed-stream-seven-j")
    # Visiting an agent does reach it, so the comparison below can fail.
    assert b"\x1b[I" in raw_input_bytes(agent("nav-work")["pane"])
    input_before_previews = {name: raw_input_bytes(pane) for name, pane in FIXTURES.items()}
    assert_no_stale_preview_frame(client)
    client.snapshot("no-stale-preview-frame")
    # Giving a left window its pane back reaches no agent: no key, no focus event, no seen state.
    assert_left_windows_show_their_agents(client, ["nav-work", "nav-done", "nav-idle", "nav-away"])
    for name, pane in FIXTURES.items():
        assert raw_input_bytes(pane) == input_before_previews[name], (name, raw_input_bytes(pane))
    assert agent("nav-done")["state"] == "done" and not agent("nav-done")["visible"]
    client.snapshot("left-windows-restored")
    # So those windows are right on arrival, with no fresh data, however quick.
    travelling = client.context()[1]
    settle(1.7)
    left_behind = [name for name in ("nav-work", "nav-idle", "nav-away")
                   if agent(name)["window"] != client.context()[2]]
    hop_screens += assert_quick_hops(client, left_behind)
    tmux("switch-client", "-c", client.context()[0], "-t", travelling)
    wait("back after hops", lambda: client.context()[1] == travelling)
    settle(1.7)
    expected = client.selected()
    # Mix text, movement and editing across preview switches. Assert content/order,
    # rather than only the commutative count of j presses.
    editing_context = client.context()
    calls_before_edit = len((ROOT / "tmux.calls").read_text().splitlines())
    client.key(b"f")
    wait("timed mixed search starts", lambda: "freeze" in client.footer() and "▏" in client.text())
    stream = [bytes([byte]) for byte in b"nav-"] + [b"\x1b[B"] + [bytes([byte]) for byte in b"done"] + [b"\x1b[A", b"\x7f", b"e"]
    for key in stream:
        client.key(key)
        time.sleep(0.035)
    settle(1.5)
    client.snapshot("timed-mixed-search")
    observed = client.text().splitlines()[2]
    (ROOT / "mixed-order.json").write_text(json.dumps({"expected_query": "nav-done",
        "expected_selection": "nav-done", "query_row": observed}) + "\n")
    assert "f nav-done▏" in observed, observed
    assert client.context() == editing_context, "search must stay in its originating sidebar"
    edit_calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()[calls_before_edit:]]
    assert not any("switch-client" in call for call in edit_calls), edit_calls
    # The final edit selects the best match, whatever the arrows did before it.
    selected(client, "nav-done")
    client.key(b"/")
    previewed(client, "nav-done")
    wait("timed search frozen", lambda: "▏" not in client.text() and "f nav-done" in client.text())
    client.key(b"\x1b")
    wait("timed query cleared", lambda: "f filter" in client.text())
    expected = "nav-done"
    # Successive arrows continue from the same selection after each preview.
    expected = ordered[(ordered.index(expected) + 1) % len(ordered)]
    client.key(b"\x1b[B")
    previewed(client, expected)
    expected = ordered[(ordered.index(expected) - 1) % len(ordered)]
    client.key(b"\x1b[A")
    previewed(client, expected)
    # Incremental search and arrows stay local; freezing previews without agent focus.
    editing_context = client.context()
    client.key(b"fnav-do")
    wait("partial query stays local", lambda: "f nav-do▏" in client.text())
    assert client.context() == editing_context
    client.key(b"ne")
    wait("incremental query", lambda: "f nav-done" in client.text() and "freeze" in client.footer())
    assert client.context() == editing_context
    select_row(client, "nav-done")
    assert client.context() == editing_context
    client.key(b"/")
    previewed(client, "nav-done")
    wait("f freezes search without focus", lambda: "▏" not in client.text())
    settle(2.2)
    assert agent("nav-done")["state"] == "done" and not agent("nav-done")["visible"]
    assert all(input_bytes(name) == b"" for name in SPAWNED)
    client.snapshot("preview-done-unseen")
    client.key(b"\r")
    wait("Enter focuses and marks seen", lambda: agent("nav-done")["state"] == "idle" and agent("nav-done")["visible"] and client.context()[1] == agent("nav-done")["pane"])
    client.key(b"x")
    wait("deliberate agent input", lambda: input_bytes("nav-done") == b"x")
    # Return to sidebar infrastructure, without sending an agent key.
    tmux("switch-client", "-c", client.context()[0], "-t", sidebar_pane(agent("nav-done")["window"]))
    wait("back in sidebar", lambda: "t all" in client.text() and client.context()[1] == sidebar_pane(agent("nav-done")["window"]))
    client.key(b"\x1b")
    wait("frozen search cleared", lambda: "f filter" in client.text())
    # gg and G move like any other key: the selection is previewed.
    names = shown_names(client)
    client.key(b"G")
    previewed(client, names[-1])
    client.key(b"g")
    settle(0.3)
    assert client.selected() == names[-1]
    client.key(b"g")
    previewed(client, names[0])
    # Enter from a search opens at once, ends the search and carries this sidebar, so the new
    # window never shows a stale row.
    query(client, "nav-work", select="nav-work")
    previewed(client, "nav-work")
    client.key(b"\x1b")
    wait("back to all rows", lambda: "f filter" in client.text())
    sidebar = client.context()[1]
    client.key(b"f\x15nav-idle")
    selected(client, "nav-idle")
    window = agent("nav-idle")["window"]
    assert window != client.context()[2]
    frames = frames_after(client, b"\r")
    (ROOT / "enter-frames.json").write_text(json.dumps(frames) + "\n")
    assert all(shown == "nav-idle" for _, (shown_window, shown) in frames if shown_window == window), frames
    wait("Enter ended the search", lambda: "f filter" in client.text() and shown_names(client) == names)
    assert frames[-1][1] == (window, "nav-idle"), frames
    assert client.context()[1] == agent("nav-idle")["pane"] and sidebar_pane(window) == sidebar
    tmux("switch-client", "-c", client.context()[0], "-t", sidebar)
    wait("back in travelling sidebar", lambda: client.context()[1] == sidebar)
    # Restore all four states. Only F may skip categories/change the state filter.
    change("nav-done", "idle", 2)
    wait("done again", lambda: agent("nav-done")["state"] == "done" and "✓" in client.text())
    for state in ["blocked", "done", "working", "idle", "all"]:
        client.key(b"t")
        wait("filter " + state, lambda state=state: "t " + state in client.text())
        client.snapshot("filter-" + state)
    # This filter proof needs one match, including when the random cwd matches the name.
    query(client, "NVWRK NPMTRVW")
    client.key(b"t")
    wait("query skips empty categories", lambda: "t working" in client.text())
    change("nav-work", "blocked", 0)
    wait("filter stays empty", lambda: "t working" in client.text() and "no matching agents" in client.text())
    original = client.context()
    settle(1.2)
    assert "t working" in client.text() and client.context() == original
    client.key(b"t")
    wait("All reached", lambda: "t all" in client.text())
    client.key(b"t")
    wait("blocked query category", lambda: "t blocked" in client.text())
    # Search editing: V/F/r/q are text, Escape restores accepted query, second Escape clears.
    client.key(b"fqVFr")
    wait("empty literal query", lambda: "no matching agents" in client.text() and "freeze" in client.footer())
    client.key(b"\x1b")
    wait("cancel query", lambda: "f NVWRK NPMTRVW" in client.text() and "freeze" not in client.footer())
    client.key(b"\x1b")
    wait("query cleared", lambda: "f filter" in client.text())
    client.key(b"t")  # blocked -> done
    wait("done filter", lambda: "t done" in client.text())
    client.key(b"t")
    wait("skip working now empty", lambda: "t idle" in client.text())
    client.key(b"t")
    wait("all filter", lambda: "t all" in client.text())
    # A live event yields to search controls, rename controls and rename errors.
    change("nav-done", "idle", 3)
    wait("fresh event", lambda: (index().get("recent_event") or {}).get("name") == "nav-done" and agent("nav-done")["completions"] == 3)
    wait("bottom event", lambda: "nav-done done" in client.footer())
    client.key(b"f")
    wait("search controls over event", lambda: "freeze" in client.footer())
    client.snapshot("search-priority")
    client.key(b"\x1b")
    wait("search cancelled", lambda: "freeze" not in client.footer())
    client.key(b"r")
    wait("rename controls over event", lambda: "save" in client.footer())
    client.key(b"\x15\r")
    wait("rename error priority", lambda: "name" in client.footer() and "save" not in client.footer())
    client.snapshot("rename-error")
    client.key(b"\x1b")
    wait("event resumes", lambda: "nav-done done" in client.footer())
    # Independent observer stays fixed while first client previews another session.
    tmux("new-session", "-d", "-s", "observer", "-x", "100", "-y", "28", "/bin/sh")
    observer_window = tmux("list-windows", "-t", "observer", "-F", "#{window_id}")
    plane("follow-sidebar", observer_window)
    CLIENTS.append(Client("observer"))
    observer = CLIENTS[1]
    wait("observer", lambda: "t all" in observer.text())
    observer_context = observer.context()
    query(client, "nav-away", select="nav-away")
    previewed(client, "nav-away")
    assert observer.context() == observer_context
    client.snapshot("cross-session-independent")
    observer.snapshot("observer-unchanged")
    # Destination with another client must be refused before any pane or window changes.
    query(client, "nav-work", select="nav-work")
    previewed(client, "nav-work")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=nav-away:")
    observer_context = observer.context()
    source_context = client.context()
    query(client, "nav-away", select="nav-away")  # target shared; no client moves
    wait("shared target refusal", lambda: "target shared" in client.footer())
    assert client.context() == source_context and observer.context() == observer_context
    client.snapshot("shared-target-refused")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=observer:")
    # Ambiguous input origin: two clients share this sidebar; refuse focus and preview.
    tmux("switch-client", "-c", observer.context()[0], "-t", source_context[1])
    client.key(b"\x1b")
    wait("all rows before ambiguity test", lambda: "f filter" in client.text())
    client.key(b"j")
    wait("ambiguous client refusal", lambda: "multiple clients" in client.footer())
    client.snapshot("ambiguous-refused")
    tmux("switch-client", "-c", observer.context()[0], "-t", "=observer:")
    # A failed preview command leaves the panes and the client where they were.
    target_sidebar = sidebar_pane(agent("nav-done")["window"])
    (ROOT / "switch.fail").touch()
    query(client, "nav-done", select="nav-done")
    wait("injected switch failure", lambda: "swap-pane" in client.footer())
    assert sidebar_pane(agent("nav-done")["window"]) == target_sidebar
    assert client.context() == source_context
    client.snapshot("failed-switch-keeps-panes")
    (ROOT / "switch.fail").unlink()
    assert_lone_sidebar_closes()
    extended_results = extra_checks(client)
    # Rename cancellation when its original target leaves: never rename the next row.
    query(client, "nav-idle", select="nav-idle")
    # A second pane keeps the window, and the sidebar in it, when the agent exits under the rename.
    tmux("split-window", "-h", "-d", "-t", agent("nav-idle")["pane"], "/bin/sh")
    client.key(b"r")
    wait("rename active", lambda: "save" in client.footer())
    plane("kill", "nav-idle")
    SPAWNED.remove("nav-idle")
    wait("rename target vanished", lambda: "rename cancelled" in client.footer())
    client.snapshot("rename-exit")
    client.key(b"\x1b")
    query(client, "no-agent-matches")
    wait("empty", lambda: "no matching agents" in client.text())
    tmux("resize-pane", "-t", client.context()[1], "-x", "20")
    settle(0.5)
    client.snapshot("narrow-empty")
    assert "t all" in client.text()
    wait("event expired", lambda: index().get("recent_event") is None and "jump" in client.footer())
    for name, pane in FIXTURES.items():
        value = typed(pane)
        assert value == (b'x' if name == 'nav-done' else b''), (name, value)
    calls = [json.loads(line) for line in (ROOT / "tmux.calls").read_text().splitlines()]
    first_preview = next(i for i, call in enumerate(calls) if call[0] == "swap-pane")
    switches = [call for call in calls[first_preview:] if "switch-client" in call]
    assert all("-c" in call for call in switches), switches
    # Keys stay in one sidebar PTY, so no key is ever forwarded through tmux.
    first_sidebar = next(i for i, call in enumerate(calls)
                         if call[0] == "set-option" and "@agent-plane-sidebar" in call)
    assert not any(call[0] == "send-keys" for call in calls[first_sidebar:])
    assert not any(call[0] == 'kill-session' for call in calls)
    daemon_log = (STATE / "daemon.log").read_text()
    assert "[scan:" not in daemon_log, "closing an agent must not fault its kind: " + daemon_log
    assert not any(call[0] in ("copy-mode", "clock-mode") for call in calls)
    RESULTS = {**extended_results, **fuzzy_results, "passed": True, "native_fixtures": len(FIXTURES), "models_used": 0,
        "quick_hop_screens": hop_screens, "idle_commands_in_10s": idle_calls, "lone_sidebar_closes": True,
        "left_windows_restored_without_input": True, "sort_everywhere_without_snap_back": True,
        "same_window": True, "cross_window_and_session": True, "gg_and_G_preview": True, "Enter_opens_from_search": True,
        "burst_jjj_exact_net_movement": True, "timed_stream_seven_j_exact_net_movement": True,
        "arrow_selection_identity": True, "timed_mixed_text_arrows_backspace_order": True,
        "active_pane_client_flag": True, "search_edit_and_arrows_stay_local_until_frozen": True,
        "preview_done_stays_unseen": True, "Enter_marks_seen": True,
        "composed_query_filter_cycle": True, "refresh_keeps_empty_filter": True,
        "search_cancel_keep_clear_literal_keys": True, "footer_priority_and_expiry": True,
        "independent_clients_unchanged": True, "shared_target_and_ambiguous_origin_refused": True,
        "failed_switch_keeps_panes": True, "no_stale_preview_frame": True, "rename_exit_cannot_retarget": True,
        "narrow_empty": True, "unintended_agent_input": 0}
except Exception:
    for client in CLIENTS:
        client.snapshot('failure')
    raise
finally:
    for name in SPAWNED:
        plane("kill", name)
    plane("daemon", "stop")
    for client in CLIENTS:
        client.close()
    tmux("kill-server", check=False)
    (ROOT / "results.json").write_text(json.dumps(RESULTS, indent=2) + "\n")
    print(json.dumps(RESULTS, indent=2))
    print("Evidence:", ROOT)
