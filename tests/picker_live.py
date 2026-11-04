#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private tmux picker proof with native Pi fixture agents; no model calls.

uv run tests/picker_live.py target/debug/tmux-agent-plane
"""
import fcntl
import hashlib
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
import time

import pyte

SOURCE = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve()
ROOT = Path(tempfile.mkdtemp(prefix='tap-pick-', dir='/tmp'))
TMUX = shutil.which('tmux')
assert TMUX and BINARY.exists()
BIN = ROOT / 'bin'
BIN.mkdir()
(ROOT / 'home').mkdir()
(ROOT / 's/tmux-agent-plane/pi').mkdir(parents=True)
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'absent.sock'),
           PLANE_NAV_ROOT=str(ROOT), TERM='xterm-256color',
           PATH=str(BIN) + os.pathsep + os.environ['PATH'])
ENV.pop('TMUX', None)
ENV.pop('NO_COLOR', None)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(f'#!{sys.executable}\nimport json, os, sys\n'
    f'with open({str(ROOT / "calls")!r}, "a") as log: log.write(json.dumps(sys.argv[1:]) + "\\n")\n'
    f'os.execv({TMUX!r}, [{TMUX!r}, "-S", {str(ROOT / "t")!r}, *sys.argv[1:]])\n')
WRAPPER.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(BINARY)
fixture = ROOT / 'fixture'
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'), '-o', str(fixture)], check=True)
(BIN / 'pi').symlink_to(fixture)
SETTINGS = Path.home() / '.gemini/antigravity-cli/settings.json'

def settings_hash():
    return hashlib.sha256(SETTINGS.read_bytes()).hexdigest() if SETTINGS.exists() else None

BASELINE = settings_hash()


def tmux(*args):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=True, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args):
    return subprocess.run([str(BINARY), *args], env=ENV, check=True, text=True,
                          capture_output=True, timeout=20).stdout.strip()


def agents():
    return json.loads(plane('list', '--json'))


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(120, 36)
        self.stream = pyte.ByteStream(self.screen)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 36, 120, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', session],
            env=ENV, stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)
        self.raw = (ROOT / (session + '.raw')).open('wb')

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            data = os.read(self.master, 65536)
            self.raw.write(data)
            self.raw.flush()
            self.stream.feed(data)

    def text(self):
        self.drain()
        return '\n'.join(self.screen.display)

    def key(self, data):
        os.write(self.master, data)

    def context(self):
        rows = tmux('list-clients', '-F', '#{client_pid} #{client_name} #{pane_id} #{session_name}')
        return next(row.split()[1:] for row in rows.splitlines() if row.split()[0] == str(self.process.pid))

    def selected(self):
        self.drain()
        return next((row.split('▌', 1)[1].split()[1] for row in self.screen.display if '▌' in row), None)

    def snapshot(self, label):
        (ROOT / (label + '.txt')).write_text(self.text())
        cells = [[dict(text=cell.data, fg=cell.fg, bg=cell.bg) for cell in row.values()]
                 for row in self.screen.buffer.values()]
        (ROOT / (label + '.json')).write_text(json.dumps(cells))

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        os.close(self.master)
        self.raw.close()


CLIENTS = []
SPAWNED = []


def wait(label, condition):
    end = time.monotonic() + 12
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        if condition():
            return
        time.sleep(.03)
    for client in CLIENTS:
        client.snapshot('failure-' + str(client.process.pid))
    raise AssertionError(f'{label}: {ROOT}')


def query(client, value, selected):
    client.key(b'\x15' + value.encode())
    wait('query ' + value, lambda: client.selected() == selected and '› ' + value in client.text())


try:
    for session in ('origin', 'other'):
        tmux('-f', '/dev/null', 'new-session', '-d', '-s', session, '/bin/sh')
    tmux('set-option', '-g', 'default-shell', '/bin/sh')
    tmux('set-option', '-g', 'default-command', '/bin/sh')
    tmux('set-option', '-g', 'prefix', 'C-a')
    tmux('set-option', '-g', 'popup-style', 'bg=#1e1e2e,fg=#cdd6f4')
    tmux('set-option', '-as', 'terminal-features', ',xterm-256color:RGB')
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    binding = next(line for line in tmux('list-keys', '-T', 'prefix').splitlines() if re.search(r'-T prefix f\s', line))
    assert 'bg=terminal,fg=terminal' in binding and '--client' in binding, binding
    client, other = Client('origin'), Client('other')
    CLIENTS += [client, other]
    wait('attached', lambda: len(tmux('list-clients').splitlines()) == 2)
    origin, untouched = client.context(), other.context()
    wait('index', lambda: (ROOT / 's/tmux-agent-plane/agents.json').exists())
    client.key(b'\x01f')
    wait('empty picker', lambda: 'no agents running' in client.text())
    client.key(b'\r')
    time.sleep(.15)
    assert 'search names / descriptions' in client.text()
    client.key(b'\x1b')
    wait('empty cancelled', lambda: 'search names / descriptions' not in client.text())
    for name in ('cedar-review', 'quartz-build', 'quartz-helper'):
        plane('spawn', 'pi', '--in', 'session', '--cwd', str(ROOT), '--name', name)
        SPAWNED.append(name)
    rows = {row['name']: row for row in agents()}
    (ROOT / (rows['quartz-build']['pane'] + '.command')).write_text('blocked 0 0')
    wait('blocked publisher', lambda: next(a for a in agents() if a['name'] == 'quartz-build')['state'] == 'blocked')
    client.key(b'\x01f')
    wait('picker', lambda: client.selected() == 'quartz-build')
    client.snapshot('all-agents')
    # Blank canvas and panel background must use terminal default, despite popup-style.
    assert client.screen.buffer[4][65].bg == 'default', client.screen.buffer[4][65]
    assert client.screen.buffer[20][65].bg == 'default', client.screen.buffer[20][65]
    query(client, 'CDRRV', 'cedar-review')
    query(client, 'qrtzbldnativepay', 'quartz-build')
    query(client, 'fxtperm', 'cedar-review')
    query(client, 'quartz', 'quartz-build')
    client.key(b'\x1b[B')
    wait('down', lambda: client.selected() == 'quartz-helper')
    # State refresh must preserve the exact browsed row.
    time.sleep(.7)
    assert client.selected() == 'quartz-helper'
    client.key(b'\x1b[A')
    wait('up', lambda: client.selected() == 'quartz-build')
    client.key(b'\n')
    wait('ctrl-j', lambda: client.selected() == 'quartz-helper')
    client.key(b'\x0b')
    wait('ctrl-k', lambda: client.selected() == 'quartz-build')
    client.snapshot('search')
    assert client.context() == origin and other.context() == untouched
    query(client, 'zzzzzz', None)
    assert 'no matching agents' in client.text()
    client.key(b'\r')
    time.sleep(.15)
    assert 'search names / descriptions' in client.text() and client.context() == origin
    client.key(b'\x1b')
    wait('cancelled', lambda: 'search names / descriptions' not in client.text())
    assert client.context() == origin
    client.key(b'\x01f')
    wait('reopen fresh', lambda: client.selected() == 'quartz-build' and '› quartz' not in client.text())
    client.key(b'qjk')
    wait('literal letters', lambda: '› qjk' in client.text())
    client.key(b'\x03')
    wait('ctrl-c cancel', lambda: 'search names / descriptions' not in client.text())
    assert client.context() == origin
    client.key(b'\x01f')
    wait('reopen for focus', lambda: client.selected() == 'quartz-build')
    query(client, 'cdrrv', 'cedar-review')
    # Another client remains attached and active while the picker focuses its own client.
    other.key(b'\x0c')
    client.key(b'\r')
    wait('exact cross-session focus', lambda: client.context()[1:] == [rows['cedar-review']['pane'], rows['cedar-review']['session']])
    assert other.context() == untouched
    calls = [json.loads(line) for line in (ROOT / 'calls').read_text().splitlines()]
    switches = [call for call in calls if call and call[0] == 'switch-client']
    assert len(switches) == 1 and switches[0][1:3] == ['-c', origin[0]], switches
    for row in rows.values():
        path = ROOT / (row['pane'] + '.input')
        assert not path.exists() or path.read_bytes().replace(b'\x1b[I', b'').replace(b'\x1b[O', b'') == b''
    print('PASS: lowercase binding; empty/no-match Enter; fuzzy name/title/detail and spanning queries; arrows; Ctrl-J/K; refresh selection; Esc/Ctrl-C; terminal background; exact originating-client cross-session focus; no leaked agent input', flush=True)
    print('Evidence:', ROOT, flush=True)
finally:
    for name in SPAWNED:
        try:
            plane('kill', name)
        except subprocess.CalledProcessError:
            pass
    for client in CLIENTS:
        client.close()
    subprocess.run([str(BINARY), 'daemon', 'stop'], env=ENV, capture_output=True, timeout=15)
    subprocess.run([str(WRAPPER), 'kill-server'], env=ENV, capture_output=True, timeout=5)
    assert settings_hash() == BASELINE, 'real Antigravity settings changed'
