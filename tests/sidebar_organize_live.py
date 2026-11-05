#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private sidebar organize confirmation proof; native Pi fixtures, no model use.

cargo build && uv run tests/sidebar_organize_live.py target/debug/tmux-agent-plane
A fake configured agy records background requests; only private native fixtures move.
"""
import fcntl
import hashlib
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
    "if sys.argv[1] == 'display-popup': sys.exit(0)\n"
    f'os.execv({TMUX!r}, [{TMUX!r}, "-S", {str(ROOT / "t")!r}, *sys.argv[1:]])\n')
WRAPPER.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
for tool in ('osascript', 'notify-send'):
    path = BIN / tool
    path.write_text('#!/bin/sh\nexit 0\n')
    path.chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/codex.c'),
                '-o', str(BIN / 'codex')], check=True)

CONFIG = ROOT / 'config/tmux-agent-plane/config.json'
CONFIG.parent.mkdir(parents=True)
SETTINGS = {'auto_name': {'enabled': False, 'command': ['agy', '--model', 'fixture-model', '--effort', 'low', '--print', '{prompt}'], 'output': 'text'}}
CONFIG.write_text(json.dumps(SETTINGS))
PLANNER = BIN / 'agy'
PLANNER.write_text(f'''#!{sys.executable}
import json, os, sys, time
from pathlib import Path
root = Path({str(ROOT)!r})
args = sys.argv[1:]
inventory = json.loads(args[-1].split('Inventory: ', 1)[1])
kind = (root / 'planner-mode').read_text() if (root / 'planner-mode').exists() else 'noop'
record = dict(pid=os.getpid(), args=args, inventory=inventory, cwd=os.getcwd(), stdin=sys.stdin.read(), identity={{k:os.environ[k] for k in ['TMUX','TMUX_PANE','CODEX_THREAD_ID','CODEX_SESSION_ID','CLAUDECODE','CLAUDE_CODE_SESSION_ID'] if k in os.environ}})
with (root / 'planner.jsonl').open('a') as log: log.write(json.dumps(record) + '\\n')
if kind == 'delay':
    while not (root / 'release').exists(): time.sleep(.03)
if kind == 'failure':
    print('fixture model failure', file=sys.stderr); sys.exit(2)
if kind == 'bad-json': print('not JSON'); sys.exit(0)
projects = {{}}
for a in inventory['agents']:
    session = 'collected-project' if kind == 'regroup-all' or (kind == 'regroup' and a['name'] == 'organizer-fixture') else a['session']
    projects.setdefault(session, []).append(a['pane'])
if kind == 'incomplete': projects = {{}}
if kind == 'unknown': projects = {{'bad': ['%999999']}}
print(json.dumps({{'projects':[{{'session':session,'agents':panes}} for session,panes in projects.items()]}}))
''')
PLANNER.chmod(0o755)


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
    def __init__(self, session='harness'):
        self.screen = Screen(110, 30)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 110, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', session], env=ENV,
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


REAL_SETTINGS = Path.home() / ".gemini/antigravity-cli/settings.json"
SETTINGS_BEFORE = hashlib.sha256(REAL_SETTINGS.read_bytes()).hexdigest() if REAL_SETTINGS.exists() else None
CLIENTS, RESULTS = [], []
STARTED = False
try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '110', '-y', '30', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', '/bin/sh')
    plane('daemon', 'start')
    STARTED = True
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    for name in ('organizer-fixture', 'unrelated-fixture'):
        plane('spawn', 'pi', '--in', 'window', '--cwd', ROOT, '--name', name)
    plane('sessions', 'create', 'other-project', '--cwd', ROOT)
    plane('move', 'unrelated-fixture', '--session', 'other-project')
    wait('other project indexed', lambda: agent('unrelated-fixture')['session'] == 'other-project')
    selected = agent('organizer-fixture')
    state(selected, 'idle')
    wait('ready organizer', lambda: agent('organizer-fixture')['state'] == 'idle')
    harness = infra('display-message', '-p', '-t', '=harness:', '#{window_id}')
    plane('toggle-sidebar', harness)
    client = Client()
    wait('sidebar controls', lambda: 'o organize all' in client.text())
    sidebar = infra('list-panes', '-t', harness, '-F', '#{pane_id} #{@agent-plane-sidebar}')
    sidebar = next(row.split()[0] for row in sidebar.splitlines() if row.endswith(' 1'))
    client.keys(b'/organizer-fixture/')
    wait('selected fixture', lambda: 'organizer-fixture' in client.text() and 'type to search' not in client.text())
    # Freezing previews the selection: wait for the sidebar to land in the agent's window.
    wait('preview landed', lambda: infra('list-clients', '-F', '#{window_id}').strip() == agent('organizer-fixture')['window'])
    time.sleep(.3)
    baseline_input, baseline_calls = inputs(), input_calls()
    for key in (b'\x1b', b'n', b'a', b'q', b'x', b'c', b'j', b'o', b'O', b'A', b'?'):
        client.keys(b'o')
        wait('confirmation', lambda: 'organize all agents? [Y/n]' in client.text())
        assert 'via agy' in client.text()
        assert 'f all' in client.text(), 'agent list must stay visible'
        client.snapshot('confirmation')
        client.keys(key)
        wait('cancelled', lambda: 'organize all agents? [Y/n]' not in client.text())
        assert inputs() == baseline_input and input_calls() == baseline_calls, key
        assert infra('list-clients', '-F', '#{pane_id}').strip() == sidebar, 'the keyboard stays in the sidebar'
    proof('o shows a compact yes/no footer with the configured background agent; the agent list stays visible and cancel sends nothing')

    client.keys(b'/o')
    wait('literal o search', lambda: 'organizer-fixtureo' in client.text())
    assert 'organize all agents? [Y/n]' not in client.text()
    client.keys(b'\x1b')
    wait('search closed', lambda: '⏎ jump' in client.text())
    client.keys(b'ro')
    time.sleep(.15)
    client.drain()
    assert 'organize all agents? [Y/n]' not in client.text()
    client.keys(b'\x1b')
    wait('rename closed', lambda: '⏎ jump' in client.text())
    proof('o remains literal in search and rename edits')

    # A busy selected row and a blocked hidden row are both organized in the background.
    state(selected, 'working')
    hidden = agent('unrelated-fixture')
    state(hidden, 'blocked')
    wait('busy and blocked fixtures', lambda: agent('organizer-fixture')['state'] == 'working' and agent('unrelated-fixture')['state'] == 'blocked')
    def mode(kind):
        (ROOT / 'planner-mode').write_text(kind)
        (ROOT / 'release').unlink(missing_ok=True)
    def status():
        return json.loads((ROOT / 's/tmux-agent-plane/agents.json').read_text()).get('organize')
    def calls():
        return [json.loads(line) for line in (ROOT / 'planner.jsonl').read_text().splitlines()] if (ROOT / 'planner.jsonl').exists() else []
    def control(payload):
        import socket
        with socket.socket(socket.AF_UNIX) as sock:
            sock.settimeout(4)
            sock.connect(str(ROOT / 's/tmux-agent-plane/daemon.sock'))
            sock.sendall(payload.encode()); sock.shutdown(socket.SHUT_WR)
            return sock.recv(65536).decode()
    def confirm(key=b'\r'):
        client.keys(b'o')
        wait('default yes prompt', lambda: 'organize all agents? [Y/n]' in client.text())
        client.keys(key)
    def finished(count, failed=False):
        wait('background result', lambda: len(calls()) == count and status() and not status()['running'] and status()['failed'] == failed)
        return status()

    # Configuration is pinned in the prompt; no model is called on cancellation/change.
    client.keys(b'o')
    wait('configuration pin', lambda: 'organize all agents? [Y/n]' in client.text())
    changed = json.loads(CONFIG.read_text()); changed['auto_name']['command'][2] = 'changed-model'
    CONFIG.write_text(json.dumps(changed))
    client.keys(b'\r')
    wait('changed configuration refused', lambda: 'configuration chang' in client.text())
    assert not calls()
    CONFIG.write_text(json.dumps(SETTINGS))
    proof('confirmation pins the saved background command and model')

    client.keys(b'o')
    wait('open before shared client', lambda: 'organize all agents? [Y/n]' in client.text())
    other_client = Client()
    wait('two clients', lambda: len(infra('list-clients', '-F', '#{client_name}').splitlines()) == 2)
    client.keys(b' ')
    wait('shared input refused', lambda: 'multiple clients in sidebar' in client.text())
    assert not calls()
    other_client.close()
    proof('shared input cannot accept a pending confirmation')

    mode('regroup')
    before = {a['name']: a for a in agents()}
    confirm()
    result = finished(1)
    wait('project moves indexed', lambda: agent('organizer-fixture')['session'] == 'collected-project' and agent('unrelated-fixture')['session'] == 'other-project')
    for a in agents():
        for field in ('pane', 'name', 'kind', 'session_id', 'parent', 'completions', 'completion_epoch'):
            assert a[field] == before[a['name']][field], field
    first = calls()[0]
    assert first['args'][:4] == ['--model', 'fixture-model', '--effort', 'low']
    assert first['args'][4] == '--print' and first['stdin'] == ''
    assert first['cwd'].endswith('/tmux-agent-plane/organize-work') and not first['identity']
    assert {a['name'] for a in first['inventory']['agents']} == {'organizer-fixture', 'unrelated-fixture'}
    assert inputs() == baseline_input and input_calls() == baseline_calls
    client.snapshot('organized')
    proof('Enter invokes the exact configured command/model once; busy and hidden blocked agents are included, native moves preserve identity, zero pane input')

    # The previewed agent moved away and its window's sidebar closed with it: go back into one.
    window = infra('list-clients', '-F', '#{window_id}').strip()
    panes = infra('list-panes', '-t', window, '-F', '#{pane_id} #{@agent-plane-sidebar}')
    infra('select-pane', '-t', next(row.split()[0] for row in panes.splitlines() if row.endswith(' 1')))
    mode('delay')
    confirm(b' ')
    wait('delayed model started', lambda: len(calls()) == 2 and status()['running'])
    assert 'already running' in control('organize ' + json.dumps(SETTINGS))
    assert 'parsing organization settings' in control('organize invalid-json')
    assert plane('list', '--json')
    plane('toggle-sidebar', harness)
    (ROOT / 'release').touch()
    finished(2)
    assert len(calls()) == 2 and inputs() == baseline_input and input_calls() == baseline_calls
    plane('toggle-sidebar', harness)
    wait('new sidebar', lambda: 'organizing all agents' not in client.text() and 'o organize all' in client.text())
    proof('Space queues one job, duplicate/invalid requests preserve daemon health, closing sidebar does not cancel the background job')

    # Evacuating an attached project keeps its client alive with a native shell.
    plane('spawn', 'pi', '--in', 'session', '--session', 'solo-project', '--cwd', ROOT, '--name', 'solo-fixture')
    solo = agent('solo-fixture')
    state(solo, 'blocked')
    wait('solo blocked', lambda: agent('solo-fixture')['state'] == 'blocked')
    solo_client = Client('solo-project')
    wait('native attach focus event recorded', lambda: (ROOT / (solo['pane'] + '.input')).exists())
    baseline_input = inputs()  # tmux emits native focus-in on attach, before organization.
    mode('regroup-all')
    count = len(calls()) + 1
    confirm()
    finished(count)
    wait('all agents organized', lambda: all(a['session'] == 'collected-project' for a in agents()))
    assert len(agents()) == 3
    assert any(s['name'] == 'solo-project' and not s['agents'] and s['attached'] == 1 for s in json.loads(plane('sessions', 'list', '--json')))
    assert solo_client.process.poll() is None
    assert inputs() == baseline_input and input_calls() == baseline_calls
    solo_client.close()
    proof('all agents across projects move together; a final attached source retains a shell and its client, including blocked agents with zero input')

    # Real Codex runs nested `codex` helpers; only the outermost process is the agent.
    plane('spawn', 'codex', '--in', 'window', '--cwd', ROOT, '--name', 'codex-fixture')
    wait('nested codex helper running', lambda: (ROOT / 'codex-helper').exists())
    count = len(calls()) + 1
    confirm()
    assert 'organized all 4 agents' in finished(count)['message'], status()
    wait('codex organized', lambda: agent('codex-fixture')['session'] == 'collected-project')
    proof('a Codex agent with nested codex helpers is organized')

    # Full-roster validation rejects malformed output and identity changes before any moves.
    for kind in ('incomplete', 'unknown', 'bad-json', 'failure'):
        mode(kind)
        count = len(calls()) + 1
        confirm()
        result = finished(count, failed=True)
        assert agent('organizer-fixture')['session'] == 'collected-project'
        assert inputs() == baseline_input and input_calls() == baseline_calls
    proof('incomplete/unknown/malformed/failed model output changes no agent panes')

    mode('delay')
    count = len(calls()) + 1
    confirm()
    wait('model held for identity change', lambda: len(calls()) == count and status()['running'])
    selected = agent('organizer-fixture')
    state(selected, 'working', identity=1)
    wait('new identity indexed', lambda: agent('organizer-fixture')['session_id'] != selected['session_id'])
    (ROOT / 'release').touch()
    assert 'identity' in finished(count, failed=True)['message']
    proof('a replaced native agent invalidates the entire plan before application')

    mode('delay')
    count += 1
    confirm()
    wait('model held for config change', lambda: len(calls()) == count and status()['running'])
    CONFIG.write_text(json.dumps(changed))
    (ROOT / 'release').touch()
    assert 'configuration changed' in finished(count, failed=True)['message']
    CONFIG.write_text(json.dumps(SETTINGS))
    proof('configuration changes while planning prevent application')

    mode('delay')
    count += 1
    confirm()
    wait('model held for restart', lambda: len(calls()) == count and status()['running'])
    planner_pid = calls()[-1]['pid']
    plane('daemon', 'restart')
    wait('interrupted status', lambda: status() and status()['failed'] and 'interrupted' in status()['message'])
    try:
        os.kill(planner_pid, 0)
        raise AssertionError('background model survived daemon restart')
    except ProcessLookupError:
        pass
    assert inputs() == baseline_input and input_calls() == baseline_calls
    proof('daemon restart cancels/reaps the private model process and reports interruption without retry')
    CONFIG.unlink()
    client.keys(b'o')
    wait('unconfigured refusal', lambda: 'press c to configure' in client.text())
    assert len(calls()) == count and 'organize all agents? [Y/n]' not in client.text()
    CONFIG.write_text(json.dumps(SETTINGS))
    proof('no configured command fails visibly without choosing a provider or using a pane')
    (ROOT / 'results.json').write_text(json.dumps(dict(passed=True, cases=RESULTS, real_model_calls=0, fixture_model_calls=len(calls())), indent=2))
finally:
    try:
        for client in list(CLIENTS):
            client.close()
        if STARTED:
            for a in agents():
                plane('kill', a['name'])
            plane('daemon', 'stop')
        subprocess.run([TMUX, '-S', str(ROOT / 't'), 'kill-server'], env=ENV, capture_output=True)
    finally:
        assert (hashlib.sha256(REAL_SETTINGS.read_bytes()).hexdigest() if REAL_SETTINGS.exists() else None) == SETTINGS_BEFORE, "real Antigravity settings changed"
