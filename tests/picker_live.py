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


def open_picker():
    """A state change may have a notification popup up, which takes keys until it times out."""
    def opened():
        if '▌' not in client.text():
            client.key(b'\x01a')
            time.sleep(.4)
        return '▌' in client.text()
    wait('picker opens', opened)


def query(client, value, selected):
    """Edit the search, entering search mode the first time."""
    if '› ' not in client.text():
        client.key(b'/')
        wait('search opens', lambda: '› ' in client.text())
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
    binding = next(line for line in tmux('list-keys', '-T', 'prefix').splitlines() if re.search(r'-T prefix a\s', line))
    assert 'bg=terminal,fg=terminal' in binding and '--client' in binding, binding
    client, other = Client('origin'), Client('other')
    CLIENTS += [client, other]
    wait('attached', lambda: len(tmux('list-clients').splitlines()) == 2)
    origin, untouched = client.context(), other.context()
    wait('index', lambda: (ROOT / 's/tmux-agent-plane/agents.json').exists())
    client.key(b'\x01a')
    wait('empty picker', lambda: 'no agents running' in client.text())
    client.key(b'\r')
    time.sleep(.15)
    assert 'j/k browse' in client.text()
    client.key(b'\x1b')
    wait('empty cancelled', lambda: 'j/k browse' not in client.text())
    for name in ('cedar-review', 'quartz-build', 'quartz-helper'):
        plane('spawn', 'pi', '--in', 'session', '--cwd', str(ROOT), '--name', name)
        SPAWNED.append(name)
    rows = {row['name']: row for row in agents()}
    (ROOT / (rows['quartz-build']['pane'] + '.command')).write_text('blocked 0 0')
    wait('blocked publisher', lambda: next(a for a in agents() if a['name'] == 'quartz-build')['state'] == 'blocked')
    client.key(b'\x01a')
    wait('picker', lambda: client.selected() == 'quartz-build')
    client.snapshot('all-agents')
    # The list opens in command mode: no search box, and letters are commands.
    assert 'search names / descriptions' not in client.text() and 'j/k browse' in client.text()
    # Blank canvas and panel background must use terminal default, despite popup-style.
    assert client.screen.buffer[4][65].bg == 'default', client.screen.buffer[4][65]
    assert client.screen.buffer[20][65].bg == 'default', client.screen.buffer[20][65]
    # Sessions sit in their own coloured column, separate from window and pane.
    def group_cell(session):
        client.drain()
        for y, line in enumerate(client.screen.display):
            found = re.search(r'(?<=\s)' + session + r'(?=\s+\d+\.\d+\s)', line)
            if found:
                return client.screen.buffer[y][found.start()].fg
        raise AssertionError(client.text())
    assert group_cell('cedar-review') != group_cell('quartz-build')
    # Plain and control vim keys browse from the list, and a state refresh keeps the row.
    browse = {}
    for key, label in ((b'j', 'j'), (b'k', 'k'), (b'\n', 'ctrl-j'), (b'\x0b', 'ctrl-k'), (b'\x1b[B', 'down'), (b'\x1b[A', 'up')):
        before = client.selected()
        client.key(key)
        wait(label, lambda: client.selected() != before)
        browse[label] = client.selected()
    assert browse['j'] == browse['ctrl-j'] == browse['down'] != 'quartz-build', browse
    assert browse['k'] == browse['ctrl-k'] == browse['up'] == 'quartz-build', browse
    # Search appears on / only; its letters are text, and Ctrl-J/K still browse.
    query(client, 'CDRRV', 'cedar-review')
    query(client, 'qrtzbldnativepay', 'quartz-build')
    query(client, 'fxtperm', 'cedar-review')
    query(client, 'quartz', 'quartz-build')
    client.key(b'\n')
    wait('search ctrl-j', lambda: client.selected() == 'quartz-helper')
    time.sleep(.7)
    assert client.selected() == 'quartz-helper'
    client.key(b'\x0b')
    wait('search ctrl-k', lambda: client.selected() == 'quartz-build')
    client.key(b'\x1b[B')
    wait('search down', lambda: client.selected() == 'quartz-helper')
    client.key(b'\x1b[A')
    wait('search up', lambda: client.selected() == 'quartz-build')
    client.snapshot('search')
    assert client.context() == origin and other.context() == untouched
    query(client, 'zzzzzz', None)
    assert 'no matching agents' in client.text()
    client.key(b'\r')
    wait('kept search', lambda: 'j/k browse' in client.text() and '› zzzzzz' in client.text())
    client.key(b'\r')
    time.sleep(.15)
    assert '› zzzzzz' in client.text() and client.context() == origin
    # Esc clears a kept search, then closes.
    client.key(b'\x1b')
    wait('search cleared', lambda: 'search names / descriptions' not in client.text() and client.selected() == 'quartz-build')
    client.key(b'\x1b')
    wait('cancelled', lambda: 'j/k browse' not in client.text())
    assert client.context() == origin
    # Esc in the search box cancels the edit and keeps the picker.
    client.key(b'\x01a')
    wait('reopen fresh', lambda: client.selected() == 'quartz-build' and '› ' not in client.text())
    client.key(b'/qjk')
    wait('literal letters', lambda: '› qjk' in client.text())
    client.key(b'\x1b')
    wait('edit cancelled', lambda: '› ' not in client.text() and 'j/k browse' in client.text())
    # Sidebar actions, on the selection, with the sidebar's own prompts.
    client.key(b'?')
    wait('help', lambda: 'Keyboard shortcuts' in client.text())
    client.key(b'q')
    wait('help closed, picker open', lambda: 'j/k browse' in client.text())
    client.key(b'f')
    wait('filter', lambda: 'agents · blocked' in client.text())
    # f skips states with no agents, so press until the cycle reaches All.
    for _ in range(5):
        if 'agents · ' not in client.text():
            break
        client.key(b'f')
        time.sleep(.15)
    wait('filter off', lambda: 'agents · ' not in client.text())
    client.key(b's')
    time.sleep(.2)
    assert 'j/k browse' in client.text()
    client.key(b'o')
    wait('organize refuses without configured AI', lambda: 'j/k browse' not in client.text())
    assert '[Y/n]' not in client.text(), client.text()
    client.key(b'\x03')
    wait('ctrl-c cancel', lambda: 'search names / descriptions' not in client.text() and '▌' not in client.text())
    assert client.context() == origin
    client.key(b'\x01a')
    wait('reopen for rename', lambda: client.selected() == 'quartz-build')
    query(client, 'qrtzhlpr', 'quartz-helper')
    client.key(b'\r')
    wait('kept', lambda: 'j/k browse' in client.text())
    client.key(b'rq\r')
    wait('renamed', lambda: any(a['name'] == 'quartz-helperq' for a in agents()))
    client.key(b'\x1b')
    wait('cleared', lambda: '› ' not in client.text())
    wait('renamed row', lambda: 'quartz-helperq' in client.text())
    SPAWNED.remove('quartz-helper')
    SPAWNED.append('quartz-helperq')
    # Kill: another key cancels with no kill, then y kills exactly that pane.
    # / freezes the search, so the same letters are commands on its matches.
    query(client, 'quartzhelperq', 'quartz-helperq')
    client.key(b'/')
    wait('frozen search', lambda: 'j/k browse' in client.text() and '› quartzhelperq' in client.text())
    client.key(b'n')
    wait('new from frozen search', lambda: 'new agent in quartz-helper' in client.text())
    client.key(b'\x1b')
    wait('new cancelled, search kept', lambda: 'new agent in' not in client.text() and '› quartzhelperq' in client.text())
    client.key(b'x')
    wait('kill prompt', lambda: 'kill quartz-helperq' in client.text() and '[Y/n]' in client.text())
    client.key(b'n')
    wait('kill cancelled', lambda: '[Y/n]' not in client.text())
    assert any(a['name'] == 'quartz-helperq' for a in agents())
    client.key(b'x')
    wait('kill prompt again', lambda: '[Y/n]' in client.text())
    client.key(b'y')
    wait('killed', lambda: not any(a['name'] == 'quartz-helperq' for a in agents()))
    SPAWNED.remove('quartz-helperq')
    assert {a['name'] for a in agents()} == {'cedar-review', 'quartz-build'}
    client.key(b'\x1b')
    wait('search cleared after kill', lambda: '› ' not in client.text())
    # A third / resumes editing a frozen search.
    query(client, 'cedar', 'cedar-review')
    client.key(b'/')
    wait('frozen again', lambda: 'j/k browse' in client.text())
    client.key(b'/r')
    wait('resumed', lambda: '› cedarr' in client.text() and 'j/k browse' not in client.text())
    client.key(b'\x1b')
    wait('resume cancelled, frozen query back', lambda: '› cedar' in client.text() and '› cedarr' not in client.text())
    client.key(b'\x1b')
    wait('frozen query cleared', lambda: '› ' not in client.text())
    assert client.context() == origin and other.context() == untouched
    client.key(b'\x1b')
    wait('closed after kill', lambda: 'j/k browse' not in client.text())
    client.key(b'\x01a')
    wait('reopen for focus', lambda: client.selected() == 'quartz-build')
    query(client, 'cdrrv', 'cedar-review')
    # Another client remains attached and active while the picker focuses its own client.
    other.key(b'\x0c')
    client.key(b'\r\r')
    wait('exact cross-session focus', lambda: client.context()[1:] == [rows['cedar-review']['pane'], rows['cedar-review']['session']])
    assert other.context() == untouched
    calls = [json.loads(line) for line in (ROOT / 'calls').read_text().splitlines()]
    switches = [call for call in calls if call and call[0] == 'switch-client']
    assert len(switches) == 1 and switches[0][1:3] == ['-c', origin[0]], switches
    # n from the popup: a real window in the selected project, the client taken to it,
    # the popup gone. A plain shell stands in for the agent: only the placement is under test.
    (BIN / 'claude').write_text('#!/bin/sh\nexec sleep 600\n')
    (BIN / 'claude').chmod(0o755)
    windows = lambda: tmux('list-windows', '-t', '=' + rows['quartz-build']['session'], '-F', '#{window_id}').split()
    before = windows()
    client.key(b'\x01a')
    wait('reopen for new', lambda: client.selected() == 'quartz-build')
    client.key(b'n')
    wait('new prompt', lambda: 'new agent in ' + rows['quartz-build']['session'] in client.text())
    client.key(b'\x1b')
    wait('new cancelled', lambda: 'new agent in' not in client.text() and 'j/k browse' in client.text())
    assert windows() == before
    client.key(b'n\r')
    wait('new window', lambda: len(windows()) == len(before) + 1)
    created = next(w for w in windows() if w not in before)
    wait('client moved to new agent', lambda: tmux('list-clients', '-F', '#{client_name} #{window_id}').find(origin[0] + ' ' + created) >= 0)
    wait('popup closed', lambda: 'j/k browse' not in client.text())
    assert other.context() == untouched
    tmux('kill-window', '-t', created)
    # o from the popup: n cancels; Space queues the daemon job. A fake command returns no plan,
    # so nothing moves and no model is called.
    (BIN / 'agy').write_text('#!/bin/sh\nexit 1\n')
    (BIN / 'agy').chmod(0o755)
    config = ROOT / 'config/tmux-agent-plane/config.json'
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text(json.dumps({'auto_name': {'enabled': False, 'output': 'text',
        'command': ['agy', '--model', 'fixture-model', '--effort', 'low', '--print', '{prompt}']}}))
    placed = {a['name']: a['session'] for a in agents()}
    status = lambda: json.loads((ROOT / 's/tmux-agent-plane/agents.json').read_text()).get('organize')
    assert status() is None
    client.key(b'\x01a')
    wait('reopen for organize', lambda: 'j/k browse' in client.text())
    client.key(b'o')
    wait('organize prompt', lambda: 'organize all agents? [Y/n]' in client.text() and 'via agy' in client.text())
    client.key(b'n')
    wait('organize cancelled', lambda: '[Y/n]' not in client.text() and 'j/k browse' in client.text())
    assert status() is None
    client.key(b'o')
    wait('organize prompt again', lambda: '[Y/n]' in client.text())
    client.key(b' ')
    wait('organize ran', lambda: status() is not None and not status()['running'])
    assert {a['name']: a['session'] for a in agents()} == placed
    client.key(b'\x1b')
    wait('closed after organize', lambda: 'via agy' not in client.text() and '▌' not in client.text())
    untouched_client = client.context()
    # Tab: keys reach exactly the selected agent, as typed, until Tab again.
    plane('spawn', 'pi', '--in', 'session', '--cwd', str(ROOT), '--name', 'talk-target')
    SPAWNED.append('talk-target')
    target = next(a for a in agents() if a['name'] == 'talk-target')['pane']
    received = lambda: (ROOT / (target + '.input')).read_bytes().replace(b'\x1b[I', b'').replace(b'\x1b[O', b'') if (ROOT / (target + '.input')).exists() else b''
    open_picker()
    query(client, 'talk', 'talk-target')
    client.key(b'\t')
    wait('talking from search', lambda: 'typing here' in client.text() and 'back to the menu' in client.text())
    # The agent's cursor shows where it really is, inside the preview.
    client.drain()
    ready = 'Native Pi fixture ' + target
    row, column = next((y, line.index(ready)) for y, line in enumerate(client.screen.display) if ready in line)
    flag, x, y = tmux('display-message', '-p', '-t', target, '#{cursor_flag} #{cursor_x} #{cursor_y}').split()
    wait('cursor in the preview', lambda: (client.screen.cursor.x, client.screen.cursor.y) == (column + int(x), row + int(y)))
    keys = [(b'hi;\xc3\xa9', b'hi;\xc3\xa9'), (b'\r', b'\r'), (b'\x1b[A', b'\x1b[A'), (b'\x1b[Z', b'\x1b[Z'),
            (b'\x03', b'\x03'), (b'\x1b;', b'\x1b;'), (b'xqj/n', b'xqj/n'), (b'\x7f', b'\x7f'), (b'\x1b', b'\x1b')]
    expected = b''
    for sent, arrives in keys:
        client.key(sent)
        expected += arrives
        wait('agent received ' + repr(sent), lambda: received() == expected)
        time.sleep(.6)  # Let a lone Escape end before the next key.
    assert 'typing here' in client.text() and client.context() == untouched_client
    client.key(b'\t')
    wait('back to the menu', lambda: 'typing here' not in client.text() and 'back to the menu' not in client.text())
    # Back in the frozen-or-live search: letters are search text again and reach no agent.
    client.key(b'z')
    wait('search text again', lambda: '› talkz' in client.text())
    time.sleep(.3)
    assert received() == expected
    client.key(b'\x1b')
    wait('search cancelled', lambda: '› ' not in client.text())
    client.key(b'\x1b')
    wait('closed after talking', lambda: 'j/k browse' not in client.text())
    # A pane that exits while you are typing to it ends the mode, with no retargeting.
    open_picker()
    query(client, 'talk', 'talk-target')
    client.key(b'\t')
    wait('talking again', lambda: 'typing here' in client.text())
    plane('kill', 'talk-target')
    SPAWNED.remove('talk-target')
    wait('left when the agent exited', lambda: 'typing here' not in client.text())
    client.key(b'\x03')
    wait('closed after exit', lambda: 'search names' not in client.text() and 'j/k browse' not in client.text())
    for row in rows.values():
        path = ROOT / (row['pane'] + '.input')
        assert not path.exists() or path.read_bytes().replace(b'\x1b[I', b'').replace(b'\x1b[O', b'') == b''
    print('PASS: lowercase binding; command mode by default; j/k, Ctrl-J/K and arrows; / search only; empty/no-match Enter; fuzzy name/title/detail and spanning queries; refresh selection; Esc/Ctrl-C/q; group column; help, filter, sort, organize, rename, kill and new from the popup; Tab typing to the pinned agent with exact bytes, cursor and exit; terminal background; exact originating-client cross-session focus; no leaked agent input', flush=True)
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
