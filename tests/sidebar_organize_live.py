#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private sidebar organize confirmation proof; native Pi fixtures, no model use.

cargo build && uv run tests/sidebar_organize_live.py target/debug/tmux-agent-plane
The selected fixture records the request; it never runs an AI/model or reorganizes user work.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

import pyte

SOURCE = Path(__file__).resolve().parents[1]
CLI = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-org-', dir='/tmp'))
print('Evidence:', ROOT, flush=True)
TMUX = shutil.which('tmux')
assert TMUX and CLI.exists()
BIN = ROOT / 'bin'
for folder in (BIN, ROOT / 'home', ROOT / 's/tmux-agent-plane/pi'):
    folder.mkdir(parents=True, exist_ok=True)
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'no.sock'),
           PLANE_NAV_ROOT=str(ROOT), TERM='xterm-256color',
           PATH=str(BIN) + os.pathsep + os.environ['PATH'], TMUX_PANE='%999999')
ENV.pop('TMUX', None)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(f'#!{sys.executable}\nimport json, os, sys\n'
    f'with open({str(ROOT / "calls.jsonl")!r}, "a") as log: log.write(json.dumps(sys.argv[1:]) + "\\n")\n'
    f'os.execv({TMUX!r}, [{TMUX!r}, "-S", {str(ROOT / "t")!r}, *sys.argv[1:]])\n')
WRAPPER.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
for tool in ('osascript', 'notify-send'):
    path = BIN / tool
    path.write_text('#!/bin/sh\nexit 0\n')
    path.chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)


def plane(*args):
    p = subprocess.run([str(CLI), *map(str, args)], env=ENV, text=True, capture_output=True, timeout=40)
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.strip()


def infra(*args):
    p = subprocess.run([str(WRAPPER), *map(str, args)], env=ENV, text=True, capture_output=True, timeout=10)
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.strip()


def agents():
    return json.loads(plane('list', '--json'))


def agent(name):
    return next((a for a in agents() if a['name'] == name), None)


def state(a, value, count=0, identity=0):
    tmp = ROOT / (a['pane'] + '.tmp')
    tmp.write_text(f'{value} {count} {identity}')
    tmp.replace(ROOT / (a['pane'] + '.command'))


def inputs():
    return {p.name: p.read_bytes() for p in ROOT.glob('*.input') if p.stat().st_size}


def input_calls():
    return [json.loads(row) for row in (ROOT / 'calls.jsonl').read_text().splitlines()
            if json.loads(row)[0] in ('load-buffer', 'paste-buffer', 'send-keys')]


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self):
        self.screen = Screen(110, 30)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 110, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', 'harness'], env=ENV,
                                       stdin=self.slave, stdout=self.slave, stderr=self.slave)
        self.raw = (ROOT / f'{self.process.pid}.raw').open('wb')
        CLIENTS.append(self)

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            data = os.read(self.master, 65536)
            self.raw.write(data)
            self.raw.flush()
            self.stream.feed(data)

    def text(self):
        return '\n'.join(row[:38] for row in self.screen.display)

    def keys(self, value):
        os.write(self.master, value)

    def snapshot(self, name):
        self.drain()
        (ROOT / (name + '.txt')).write_text(self.text())

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
        CLIENTS.remove(self)


def wait(label, predicate, timeout=12):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        if predicate():
            return
        time.sleep(.03)
    for client in CLIENTS:
        client.snapshot('failure')
    raise AssertionError((label, ROOT))


def proof(label):
    RESULTS.append(label)
    print('PASS:', label, flush=True)


CLIENTS, RESULTS = [], []
STARTED = False
try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '110', '-y', '30', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', '/bin/sh')
    plane('daemon', 'start')
    STARTED = True
    for name in ('organizer-fixture', 'unrelated-fixture'):
        plane('spawn', 'pi', '--in', 'window', '--cwd', ROOT, '--name', name)
    selected = agent('organizer-fixture')
    state(selected, 'idle')
    wait('ready organizer', lambda: agent('organizer-fixture')['state'] == 'idle')
    harness = infra('display-message', '-p', '-t', '=harness:', '#{window_id}')
    plane('toggle-sidebar', harness)
    client = Client()
    wait('sidebar controls', lambda: 'o organize' in client.text())
    sidebar = infra('list-panes', '-t', harness, '-F', '#{pane_id} #{@agent-plane-sidebar}')
    sidebar = next(row.split()[0] for row in sidebar.splitlines() if row.endswith(' 1'))
    client.keys(b'/organizer-fixture\r')
    wait('selected fixture', lambda: 'organizer-fixture' in client.text() and 'type to search' not in client.text())
    baseline_input, baseline_calls = inputs(), input_calls()
    for key in (b'\x1b', b'\r', b'q', b'x', b'c', b'j', b'o', b'O', b'A', b'?'):
        client.keys(b'o')
        wait('confirmation', lambda: 'Organize all projects?' in client.text())
        assert 'Organizer: organizer-fixture' in client.text()
        client.snapshot('confirmation')
        client.keys(key)
        wait('cancelled', lambda: 'Organize all projects?' not in client.text())
        assert inputs() == baseline_input and input_calls() == baseline_calls, key
        assert infra('display-message', '-p', '-t', harness, '#{pane_id}') == sidebar
    proof('o opens a named organizer confirmation; cancellation keys are consumed with no input or focus changes')

    client.keys(b'/o')
    wait('literal o search', lambda: 'organizer-fixtureo' in client.text())
    assert 'Organize all projects?' not in client.text()
    client.keys(b'\x1b')
    wait('search closed', lambda: '⏎ jump' in client.text())
    client.keys(b'ro')
    time.sleep(.15)
    client.drain()
    assert 'Organize all projects?' not in client.text()
    client.keys(b'\x1b')
    wait('rename closed', lambda: '⏎ jump' in client.text())
    proof('o remains literal in search and rename edits')

    for native, indexed in (('working', 'working'), ('blocked', 'blocked')):
        state(selected, native)
        wait('organizer ' + indexed, lambda: agent('organizer-fixture')['state'] == indexed)
        wait('sidebar readiness state', lambda: any('organizer-fixture' in row and indexed in row
                                                     for row in client.text().splitlines()))
        client.keys(b'o')
        wait('readiness refusal', lambda: 'select an idle or done organizer' in client.text())
        assert 'Organize all projects?' not in client.text()
        assert inputs() == baseline_input and input_calls() == baseline_calls
    state(selected, 'idle')
    wait('ready again', lambda: agent('organizer-fixture')['state'] == 'idle')
    wait('ready sidebar', lambda: any('organizer-fixture' in row and 'idle' in row
                                      for row in client.text().splitlines()))
    client.keys(b'o')
    wait('open before state change', lambda: 'Organize all projects?' in client.text())
    state(selected, 'blocked')
    wait('blocked while confirming', lambda: agent('organizer-fixture')['state'] == 'blocked')
    client.keys(b'a')
    wait('changed organizer refusal', lambda: 'organizer changed' in client.text())
    assert inputs() == baseline_input and input_calls() == baseline_calls
    proof('working/blocked organizers and state changes after opening cannot receive the request')

    state(selected, 'idle')
    wait('ready for identity test', lambda: agent('organizer-fixture')['state'] == 'idle')
    wait('ready sidebar for identity', lambda: any('organizer-fixture' in row and 'idle' in row
                                                   for row in client.text().splitlines()))
    client.keys(b'o')
    wait('open before identity change', lambda: 'Organize all projects?' in client.text())
    state(selected, 'idle', identity=1)
    old_id = selected['session_id']
    wait('new native identity', lambda: agent('organizer-fixture')['session_id'] != old_id)
    client.keys(b'a')
    wait('native identity refusal', lambda: 'organizer changed' in client.text())
    assert inputs() == baseline_input and input_calls() == baseline_calls
    proof('native identity changes cannot silently retarget a confirmed request')

    client.keys(b'o')
    wait('open before second client', lambda: 'Organize all projects?' in client.text())
    other_client = Client()
    wait('two clients', lambda: len(infra('list-clients', '-F', '#{client_name}').splitlines()) == 2)
    client.keys(b'a')
    wait('shared input refused', lambda: 'multiple clients in sidebar' in client.text())
    assert inputs() == baseline_input and input_calls() == baseline_calls
    other_client.close()
    wait('one client', lambda: len(infra('list-clients', '-F', '#{client_name}').splitlines()) == 1)
    proof('a changed/shared input client cannot dispatch a pending request')

    client.keys(b'o')
    wait('lowercase open', lambda: 'Organize all projects?' in client.text())
    client.keys(b'a')
    path = ROOT / (selected['pane'] + '.input')
    wait('request delivered once', lambda: path.exists() and path.read_bytes().endswith(b'\r'))
    wait('dispatch notice', lambda: 'asked organizer-fixture to organize' in client.text())
    client.snapshot('requested')
    client.keys(b'a')
    time.sleep(.15)
    client.drain()
    request = path.read_bytes()
    assert request.startswith(b'Organize the current tmux work into project sessions.')
    assert request.count(b'Organize the current tmux work into project sessions.') == 1
    assert request.count(b'\r') == 1 and b'"organizer-fixture"' in request
    assert b'tmux-agent-plane skill' in request and b'canonical' in request
    assert (ROOT / (agent('unrelated-fixture')['pane'] + '.input')).exists() is False
    assert len(input_calls()) == len(baseline_calls) + 3, input_calls()
    assert infra('display-message', '-p', '-t', harness, '#{pane_id}') == sidebar
    (ROOT / 'request.txt').write_bytes(request)
    proof('a sends exactly one complete organization task to the selected agent; other agents and client focus stay untouched')
    (ROOT / 'results.json').write_text(json.dumps(dict(passed=True, cases=RESULTS, model_calls=0), indent=2))
finally:
    for client in list(CLIENTS):
        client.close()
    if STARTED:
        for a in agents():
            plane('kill', a['name'])
        plane('daemon', 'stop')
    subprocess.run([TMUX, '-S', str(ROOT / 't'), 'kill-server'], env=ENV, capture_output=True)
