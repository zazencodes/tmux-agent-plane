#!/usr/bin/env python3
"""Prove arriving at a window resizes nothing, on a private tmux server with the real plugin,
daemon, sidebars and an attached client.

python3 tests/window_fit_live.py [path/to/tmux-agent-plane]
Panes run a fixture that logs each SIGWINCH. No agents, model calls or user settings.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time

SOURCE = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-fit-', dir='/tmp'))
TMUX = shutil.which('tmux')
assert TMUX and BINARY.exists(), 'tmux and cargo build required'
BIN = ROOT / 'bin'
BIN.mkdir()
COMMANDS = ROOT / 'commands.log'
wrapper = BIN / 'tmux'
wrapper.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> ' + shlex.quote(str(COMMANDS)) + '\nexec '
                   + shlex.quote(TMUX) + ' -S ' + shlex.quote(str(ROOT / 't')) + ' "$@"\n')
wrapper.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(BINARY)
FIXTURE = ROOT / 'winch.py'
FIXTURE.write_text('''
import fcntl, signal, struct, sys, termios
def note(*_):
    rows, cols = struct.unpack('HHHH', fcntl.ioctl(1, termios.TIOCGWINSZ, b'\\0' * 8))[:2]
    with open(sys.argv[1], 'a') as log:
        log.write(f'{cols}x{rows}\\n')
signal.signal(signal.SIGWINCH, note)
note()
while True:
    signal.pause()
''')
ENV = dict(os.environ, HOME=str(ROOT / 'h'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'c'), PATH=str(BIN) + os.pathsep + os.environ['PATH'])
ENV.pop('TMUX', None)
(ROOT / 'h').mkdir()
RESULTS = []
CLIENTS = []
# Long enough for a tick to see the size and the next to act on it.
SETTLE = 3.2


def tmux(*args, check=True):
    return subprocess.run([str(wrapper), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=5).stdout.strip()


def agent(name):
    return f'python3 {FIXTURE} {ROOT}/{name}.log'


def signals(name):
    path = ROOT / f'{name}.log'
    return path.read_text().splitlines() if path.exists() else []


def geometry(window):
    return tmux('display-message', '-p', '-t', window, '#{window_width}x#{window_height}')


def layout(window):
    """Pane widths in order, sidebar first."""
    return [int(w) for w in tmux('list-panes', '-t', window, '-F', '#{pane_width}').split()]


def sidebar_of(window):
    rows = tmux('list-panes', '-t', window, '-F', '#{pane_id} #{@agent-plane-sidebar}').splitlines()
    return next(r.split()[0] for r in rows if r.endswith(' 1'))


def wait(label, predicate, timeout=6):
    end = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < end, (label, 'timed out', ROOT)
        time.sleep(.05)


def passed(case, **evidence):
    RESULTS.append(dict(case=case, **evidence))
    print(f'PASS: {case}: {evidence}', flush=True)


class Client:
    def __init__(self, session, cols, rows):
        self.master, slave = pty.openpty()
        self.size(cols, rows)
        self.process = subprocess.Popen([str(wrapper), 'attach-session', '-t', session], env=ENV,
                                        stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        threading.Thread(target=self.drain, daemon=True).start()
        CLIENTS.append(self)

    def drain(self):
        try:
            while os.read(self.master, 65536):
                pass
        except OSError:
            pass

    def size(self, cols, rows):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack('HHHH', rows, cols, 0, 0))

    def close(self):
        self.process.terminate()
        self.process.wait(5)
        CLIENTS.remove(self)


def hidden(session, name):
    """A detached session running the fixture; returns its window."""
    tmux('new-session', '-d', '-s', session, agent(name))
    return tmux('display-message', '-p', '-t', f'={session}:', '#{window_id}')


def arrive(session, window, own, name):
    """What the sidebar runs for a preview: carry it to the window with the client."""
    before = len(signals(name))
    tmux('swap-pane', '-s', own, '-t', sidebar_of(window), ';',
         'switch-client', '-t', f'={session}:{window}.{own}')
    # tmux delays a second pane resize by 250ms; a late one would show up here.
    time.sleep(.5)
    return signals(name)[before:]


try:
    tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'home', '-x', '200', '-y', '50', agent('home'))
    tmux('set-option', '-g', 'aggressive-resize', 'on')
    client = Client('home', 200, 51)
    wait('client attached', lambda: tmux('list-clients', '-F', '#{client_name}'))
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    wait('index', lambda: (ROOT / 's/tmux-agent-plane/agents.json').exists())
    home = tmux('display-message', '-p', '-t', '=home:', '#{window_id}')
    tmux('run-shell', f'tmux-agent-plane toggle-sidebar {home}')
    wait('sidebar', lambda: len(layout(home)) == 2)
    own = sidebar_of(home)
    # A session created before the daemon has learned the client's size.
    old = hidden('old', 'old')
    wait('old gets a sidebar', lambda: len(layout(old)) == 2)
    wait('old window fits the client', lambda: geometry(old) == '200x50')
    time.sleep(1.2)
    assert layout(old) == [38, 161], layout(old)
    assert signals('old')[-1] == '161x50', signals('old')
    passed('a hidden window from before the daemon saw the client is fitted with its sidebar', old=signals('old'))
    assert tmux('show-options', '-gv', 'default-size') == '200x50'
    assert not tmux('show-options', '-w', '-t', old), 'fit must hand sizing back to tmux'

    assert arrive('old', old, own, 'old') == []
    passed('first arrival at a new window resizes nothing')
    assert arrive('home', home, own, 'home') == []
    passed('return arrival resizes nothing')

    # The terminal changes size while a window is hidden (a tiling window manager does this).
    before = len(signals('old'))
    client.size(120, 41)
    wait('hidden window follows the client', lambda: geometry(old) == '120x40')
    time.sleep(.6)
    changes = signals('old')[before:]
    assert changes == ['81x40'], changes
    assert layout(old) == [38, 81], layout(old)
    assert arrive('old', old, own, 'old') == []
    passed('a hidden window takes one resize, to its final size, and arrival takes none', changes=changes)

    # Drag: only the size it stops at wakes the hidden agent.
    before = len(signals('home'))
    for cols in range(120, 300, 5):
        client.size(cols, 41)
        time.sleep(.05)
    client.size(180, 41)
    wait('hidden window follows the drag', lambda: geometry(home) == '180x40')
    time.sleep(.6)
    changes = signals('home')[before:]
    assert changes == ['141x40'], changes
    passed('dragging the terminal wakes a hidden agent once', changes=changes)

    # A new window of a session nobody is attached to.
    tmux('new-window', '-d', '-t', '=home:', agent('fresh'))
    fresh = tmux('list-windows', '-t', '=home:', '-F', '#{window_id}').splitlines()[-1]
    wait('fresh gets a sidebar', lambda: len(layout(fresh)) == 2)
    assert geometry(fresh) == '180x40' and layout(fresh) == [38, 141]
    assert set(signals('fresh')) == {'180x40', '141x40'}, signals('fresh')
    passed('a new window never passes through a wrong size', signals=signals('fresh'))

    # A window that exits at once must not surface a hook failure over the pane on screen.
    tmux('new-window', '-d', '-t', '=home:', 'true')
    time.sleep(.5)
    assert tmux('display-message', '-p', '#{pane_in_mode}') == '0', 'hook failure opened view mode'
    passed('a window that exits at once is ignored')

    # Windows tmux or the user keeps at another size are left alone.
    client.size(150, 41)
    wait('settled at 150', lambda: geometry(home) == '150x40' or geometry(old) == '150x40')
    time.sleep(SETTLE)
    tmux('resize-pane', '-Z', '-t', tmux('list-panes', '-t', fresh, '-F', '#{pane_id}').split()[1])
    pinned = hidden('pinned', 'pinned')
    wait('pinned gets a sidebar', lambda: len(layout(pinned)) == 2)
    tmux('resize-window', '-t', pinned, '-x', '100', '-y', '30')
    client.size(170, 41)
    wait('hidden windows follow', lambda: geometry(home) == '170x40' or geometry(old) == '170x40')
    time.sleep(SETTLE)
    assert geometry(pinned) == '100x30' and tmux('show-options', '-wv', '-t', pinned, 'window-size') == 'manual'
    assert tmux('display-message', '-p', '-t', fresh, '#{window_zoomed_flag}') == '1'
    assert geometry(fresh) == '150x40', geometry(fresh)
    passed('zoomed and manually sized windows are left alone')
    tmux('resize-pane', '-Z', '-t', tmux('list-panes', '-t', fresh, '-F', '#{pane_id}').split()[1])

    # Two clients at different sizes: nothing says which one a hidden window will be shown at.
    wait('fresh fits again', lambda: geometry(fresh) == '170x40', timeout=SETTLE + 2)
    second = Client('pinned', 90, 31)
    wait('second client', lambda: len(tmux('list-clients', '-F', '#{client_name}').splitlines()) == 2)
    time.sleep(SETTLE)
    assert geometry(fresh) == '170x40', geometry(fresh)
    passed('hidden windows wait while clients disagree')
    second.close()

    # Idle costs no extra tmux process, and a fitted server runs no resize.
    wait('one client', lambda: len(tmux('list-clients', '-F', '#{client_name}').splitlines()) == 1)
    time.sleep(SETTLE)
    start = COMMANDS.read_text().count('\n')
    time.sleep(5)
    idle = COMMANDS.read_text().splitlines()[start:]
    assert not [c for c in idle if 'resize' in c or 'default-size' in c], idle
    assert len(idle) <= 6, idle
    passed('idle runs at most one tmux process a second and no resize', commands_in_5s=len(idle))
    log = ROOT / 's/tmux-agent-plane/daemon.log'
    faults = [line for line in log.read_text().splitlines() if '[fit]' in line]
    assert not faults, faults
finally:
    for client in list(CLIENTS):
        client.close()
    tmux('kill-server', check=False)
    (ROOT / 'results.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
    print('Evidence:', ROOT)
