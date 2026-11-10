#!/usr/bin/env python3
"""Prove event-driven sidebar cleanup on a private tmux server, with no daemon.

python3 tests/sidebar_close_live.py [path/to/tmux-agent-plane]
Panes run sleep/FIFO fixtures and interactive shells. No agents, model calls or user settings.
"""
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import shlex
import shutil
import struct
import subprocess
import sys
import tempfile
import termios
import time

SOURCE = Path(__file__).resolve().parents[1]
BINARY = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-close-', dir='/tmp'))
TMUX = shutil.which('tmux')
assert TMUX and BINARY.exists(), 'tmux and cargo build required'
BIN = ROOT / 'bin'
BIN.mkdir()
wrapper = BIN / 'tmux'
wrapper.write_text('#!/bin/sh\nexec ' + shlex.quote(TMUX) + ' -S ' + shlex.quote(str(ROOT / 't')) + ' "$@"\n')
wrapper.chmod(0o755)
# Load the actual plugin without starting a watcher or follow-sidebar jobs. The
# cleanup hook must work entirely inside tmux, even with no agent-plane process.
cli = BIN / 'tmux-agent-plane'
cli.write_text('#!/bin/sh\nexit 0\n')
cli.chmod(0o755)
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), PATH=str(BIN) + os.pathsep + os.environ['PATH'],
           TERM='xterm-256color')
ENV.pop('TMUX', None)
(ROOT / 'home').mkdir()
RESULTS = []
CLIENT = None
MASTER, SLAVE = pty.openpty()
fcntl.ioctl(SLAVE, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 100, 0, 0))


def tmux(*args, check=True):
    return subprocess.run([str(wrapper), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=5).stdout.strip()


def windows():
    return tmux('list-windows', '-a', '-F', '#{window_id}').splitlines()


def window(command='sleep 100'):
    return tmux('new-window', '-d', '-P', '-F', '#{window_id}', command)


def pane(win):
    return tmux('list-panes', '-t', win, '-F', '#{pane_id}').splitlines()[0]


def sidebar(win):
    p = tmux('split-window', '-d', '-t', win, '-P', '-F', '#{pane_id}', 'sleep 100')
    tmux('set-option', '-p', '-t', p, '@agent-plane-sidebar', '1')
    return p


def closes(label, win, action):
    start = time.monotonic()
    action()
    while win in windows():
        assert time.monotonic() - start < .3, (label, 'sidebar survived 300ms', ROOT)
        time.sleep(.002)
    elapsed = round((time.monotonic() - start) * 1000, 1)
    RESULTS.append(dict(case=label, elapsed_ms=elapsed))
    print(f'PASS: {label}: {elapsed}ms', flush=True)


def mode(p):
    return tmux('display-message', '-p', '-t', p, '#{pane_mode}')


def leaves_copy_mode(label, p, *entry):
    start = time.monotonic()
    tmux('copy-mode', '-t', p, *entry)
    while mode(p):
        assert time.monotonic() - start < .3, (label, 'sidebar stayed in copy mode 300ms', ROOT)
        time.sleep(.002)
    elapsed = round((time.monotonic() - start) * 1000, 1)
    RESULTS.append(dict(case=label, elapsed_ms=elapsed))
    print(f'PASS: {label}: {elapsed}ms', flush=True)


def until(predicate):
    deadline = time.monotonic() + 3
    while not predicate():
        assert time.monotonic() < deadline, ('timed out', ROOT)
        # Keep the attached terminal draining while tmux redraws.
        if select.select([MASTER], [], [], .01)[0]:
            os.read(MASTER, 1000000)


def shell_session(name):
    p = tmux('new-session', '-d', '-P', '-F', '#{pane_id}', '-s', name,
             "env PS1='tap-ready> ' /bin/sh -i")
    until(lambda: 'tap-ready>' in tmux('capture-pane', '-p', '-t', p))
    return p


def client_session():
    return tmux('list-clients', '-F', '#{session_name}')


try:
    tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'scratch', '-x', '100', '-y', '28', 'sleep 100')
    anchor = windows()[0]
    for _ in range(2):
        subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    hooks = tmux('show-hooks', '-gw')
    assert hooks.count('window-layout-changed[90]') == 1, 'reload must replace its own hook'
    assert hooks.count('pane-mode-changed[90]') == 1, 'reload must replace its own hook'
    # Copy mode would freeze a sidebar's frame. Hidden windows prove the event's pane is pinned.
    w = window()
    shell = pane(w)
    s = sidebar(w)
    leaves_copy_mode('prefix [ in a sidebar', s)
    leaves_copy_mode('prefix PageUp in a sidebar', s, '-u')
    # Other panes keep copy mode, and a sidebar keeps the modes that do not outlive their use.
    tmux('copy-mode', '-t', shell)
    tmux('choose-tree', '-t', s)
    time.sleep(.2)
    assert mode(shell) == 'copy-mode', 'an ordinary pane must keep copy mode'
    assert mode(s) == 'tree-mode', 'a sidebar must keep tree mode'
    RESULTS.append(dict(case='ordinary pane copy mode and sidebar tree mode survive'))
    tmux('kill-window', '-t', w)
    # Hidden window and removed active pane: default hook context can be another window.
    w = window()
    shell = pane(w)
    s = sidebar(w)
    closes('kill hidden active content pane', w, lambda: tmux('kill-pane', '-t', shell))
    assert anchor in windows()
    # Sidebar active at removal, plus multi-pane windows that must remain alive.
    w = window()
    shell = pane(w)
    s = sidebar(w)
    extra = tmux('split-window', '-d', '-t', w, '-P', '-F', '#{pane_id}', 'sleep 100')
    tmux('select-pane', '-t', s)
    tmux('kill-pane', '-t', extra)
    assert w in windows() and len(tmux('list-panes', '-t', w, '-F', '#{pane_id}').splitlines()) == 2
    closes('kill final content with sidebar active', w, lambda: tmux('kill-pane', '-t', shell))
    # A genuine process exit (the shell waits on a FIFO, with no timing assumptions).
    fifo = ROOT / 'exit'
    os.mkfifo(fifo)
    w = window('read value < ' + shlex.quote(str(fifo)))
    sidebar(w)
    closes('natural shell exit', w, lambda: fifo.write_text('exit\n'))
    # Removing content by moving it must also close only its source sidebar.
    w = window()
    shell = pane(w)
    sidebar(w)
    closes('break content into new window', w, lambda: tmux('break-pane', '-d', '-s', shell))
    w = window()
    shell = pane(w)
    sidebar(w)
    closes('join content into another window', w, lambda: tmux('join-pane', '-d', '-s', shell, '-t', anchor))
    # Linked windows disappear in every session without disturbing other windows.
    tmux('new-session', '-d', '-s', 'linked', 'sleep 100')
    w = window()
    shell = pane(w)
    sidebar(w)
    tmux('link-window', '-s', w, '-t', 'linked:')
    closes('linked window closes everywhere', w, lambda: tmux('kill-pane', '-t', shell))
    # Removing a sidebar leaves the ordinary content alone.
    w = window()
    s = sidebar(w)
    tmux('kill-pane', '-t', s)
    tmux('resize-window', '-t', w, '-x', '90')
    assert w in windows() and anchor in windows()
    assert not (ROOT / 's/tmux-agent-plane/agents.json').exists(), 'proof must have no daemon'
    RESULTS.append(dict(case='ordinary panes survive sidebar removal and layout changes'))
    # A real terminal client proves Ctrl-D closes a shell and keeps the client attached.
    CLIENT = subprocess.Popen([str(wrapper), 'attach-session', '-t', 'scratch'],
                              env=ENV, stdin=SLAVE, stdout=SLAVE, stderr=SLAVE)
    until(lambda: client_session() == 'scratch')
    for with_sidebar in (False, True):
        shell = shell_session('closing')
        if with_sidebar:
            sidebar(tmux('display-message', '-p', '-t', shell, '#{window_id}'))
        tmux('switch-client', '-t', 'closing')
        until(lambda: client_session() == 'closing')
        os.write(MASTER, b'\x04')
        until(lambda: client_session() == 'scratch')
        assert CLIENT.poll() is None, 'client exited instead of switching sessions'
        assert 'closing' not in tmux('list-sessions', '-F', '#{session_name}').splitlines()
        label = 'Ctrl-D switches sessions ' + ('with sidebar' if with_sidebar else 'without sidebar')
        RESULTS.append(dict(case=label))
        print('PASS:', label, flush=True)
    # An explicit session close has the same native routing.
    shell_session('closing')
    tmux('switch-client', '-t', 'closing')
    tmux('kill-session', '-t', '=closing')
    until(lambda: client_session() == 'scratch')
    assert CLIENT.poll() is None
    RESULTS.append(dict(case='explicit session close keeps the client attached'))
    # Closing one window keeps the client in its session while another window remains.
    shell = tmux('new-window', '-t', 'scratch:', '-P', '-F', '#{pane_id}',
                 "env PS1='tap-ready> ' /bin/sh -i")
    until(lambda: 'tap-ready>' in tmux('capture-pane', '-p', '-t', shell))
    os.write(MASTER, b'\x04')
    until(lambda: shell not in tmux('list-panes', '-a', '-F', '#{pane_id}').splitlines())
    assert client_session() == 'scratch' and CLIENT.poll() is None
    RESULTS.append(dict(case='closing one window stays in the current session'))
    # The final session still closes normally, including its lone sidebar.
    shell = shell_session('final')
    sidebar(tmux('display-message', '-p', '-t', shell, '#{window_id}'))
    tmux('switch-client', '-t', 'final')
    tmux('kill-session', '-a', '-t', '=final')
    os.write(MASTER, b'\x04')
    until(lambda: CLIENT.poll() is not None)
    assert CLIENT.returncode == 0
    RESULTS.append(dict(case='Ctrl-D exits tmux when the final session closes'))
    print('PASS: final session exits tmux', flush=True)
finally:
    tmux('kill-server', check=False)
    if CLIENT is not None:
        CLIENT.wait(timeout=5)
    os.close(MASTER)
    os.close(SLAVE)
    (ROOT / 'results.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
    print('Evidence:', ROOT)
