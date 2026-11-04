#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private proof of the sidebar's `n` picker; native Pi fixtures, no model use.

cargo build && uv run tests/sidebar_new_agent_live.py target/debug/tmux-agent-plane
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
ROOT = Path(tempfile.mkdtemp(prefix='tap-new-', dir='/tmp')).resolve()
print('Evidence:', ROOT, flush=True)
TMUX = shutil.which('tmux')
BIN = ROOT / 'bin'
WORK = ROOT / 'project-dir'
for folder in (BIN, WORK, ROOT / 'home', ROOT / 's/tmux-agent-plane/pi'):
    folder.mkdir(parents=True, exist_ok=True)
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'no.sock'),
           PLANE_NAV_ROOT=str(ROOT), TERM='xterm-256color',
           PATH=str(BIN) + os.pathsep + os.environ['PATH'], TMUX_PANE='%999999')
ENV.pop('TMUX', None)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(f'#!/bin/sh\nexec {TMUX} -S {ROOT / "t"} "$@"\n')
WRAPPER.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
for tool in ('osascript', 'notify-send'):
    (BIN / tool).write_text('#!/bin/sh\nexit 0\n')
    (BIN / tool).chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)


def plane(*args):
    p = subprocess.run([str(CLI), *map(str, args)], env=ENV, text=True, capture_output=True, timeout=40)
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.strip()


def tmux(*args):
    p = subprocess.run([str(WRAPPER), *map(str, args)], env=ENV, text=True, capture_output=True, timeout=10)
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.strip()


def agents():
    return json.loads(plane('list', '--json'))


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(110, 30)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 110, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', session], env=ENV,
                                        stdin=self.slave, stdout=self.slave, stderr=self.slave)

    def text(self):
        while select.select([self.master], [], [], 0)[0]:
            self.stream.feed(os.read(self.master, 65536))
        return '\n'.join(row[:38] for row in self.screen.display)

    def keys(self, value):
        os.write(self.master, value)

    def close(self):
        self.process.kill()
        self.process.wait(timeout=5)
        os.close(self.master)
        os.close(self.slave)


def wait(label, predicate, client, timeout=12):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if predicate():
            return
        client.text()
        time.sleep(.03)
    (ROOT / 'failure.txt').write_text(client.text())
    raise AssertionError((label, ROOT))


def windows(session):
    return tmux('list-windows', '-t', f'={session}:', '-F', '#{window_id}').splitlines()


STARTED = False
client = second = None
try:
    tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '110', '-y', '30', '/bin/sh')
    tmux('set-option', '-g', 'default-shell', '/bin/sh')
    plane('daemon', 'start')
    STARTED = True
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    plane('sessions', 'create', 'api', '--cwd', WORK)
    plane('spawn', 'pi', '--in', 'window', '--session', 'api', '--cwd', WORK, '--name', 'hovered')
    harness = tmux('display-message', '-p', '-t', '=harness:', '#{window_id}')
    plane('toggle-sidebar', harness)
    client = Client('harness')
    wait('sidebar', lambda: 'n new' in client.text(), client)
    client.keys(b'/hovered\r')
    wait('selected', lambda: 'hovered' in client.text(), client)
    before = windows('api')

    client.keys(b'n')
    wait('picker', lambda: 'new agent in api' in client.text() and 'grok' in client.text(), client)
    for kind in ('claude', 'codex', 'pi', 'opencode', 'grok'):
        assert kind in client.text(), kind
    assert 'agy' not in client.text()
    client.keys(b'x?q')
    time.sleep(.2)
    assert 'new agent in api' in client.text() and 'kill' not in client.text()
    client.keys(b'\x1b')
    wait('cancelled', lambda: 'new agent in api' not in client.text(), client)
    assert windows('api') == before
    print('PASS: picker offers the kinds, consumes other keys, cancel creates nothing', flush=True)

    client.keys(b'njj\r')
    wait('new window', lambda: len(windows('api')) == len(before) + 1, client)
    window = next(w for w in windows('api') if w not in before)
    wait('client moved', lambda: tmux('list-clients', '-F', '#{session_name} #{window_id}') == f'api {window}', client)
    wait('tracked', lambda: any(a['window'] == window and a['kind'] == 'pi' for a in agents()), client)
    new = next(a for a in agents() if a['window'] == window)
    assert new['session'] == 'api' and new['cwd'] == str(WORK), new
    panes = tmux('list-panes', '-t', window, '-F', '#{pane_id} #{pane_active} #{@agent-plane-sidebar}').splitlines()
    assert sum(row.endswith(' 1') and len(row.split()) == 3 for row in panes) == 1, panes
    assert f'{new["pane"]} 1' in panes, panes
    print('PASS: Enter starts the kind in the project and directory, with keyboard in the agent and one sidebar', flush=True)

    # This client returns to its other session; a second client now sits in the project.
    name = tmux('list-clients', '-F', '#{client_name}')
    tmux('switch-client', '-c', name, '-t', '=harness:')
    wait('back in harness', lambda: tmux('list-clients', '-F', '#{session_name}') == 'harness', client)
    home = tmux('list-panes', '-t', '=harness:', '-F', '#{pane_id} #{@agent-plane-sidebar}')
    tmux('select-pane', '-t', next(row.split()[0] for row in home.splitlines() if row.endswith(' 1')))
    second = Client('api')
    wait('second client', lambda: len(tmux('list-clients').splitlines()) == 2, client)
    wait('rows', lambda: 'hovered' in client.text() and 'n new' in client.text(), client)
    count = len(windows('api'))
    client.keys(b'n\r')
    wait('refusal', lambda: 'shared with another' in client.text(), client)
    assert len(windows('api')) == count
    print('PASS: a session used by another client is refused before anything starts', flush=True)
finally:
    for c in (client, second):
        if c:
            c.close()
    if STARTED:
        for a in agents():
            plane('kill', a['name'])
        plane('daemon', 'stop')
    subprocess.run([TMUX, '-S', str(ROOT / 't'), 'kill-server'], env=ENV, capture_output=True)
