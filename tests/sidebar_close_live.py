#!/usr/bin/env python3
"""Prove event-driven sidebar cleanup on a private tmux server, with no daemon.

python3 tests/sidebar_close_live.py [path/to/tmux-agent-plane]
Panes run only sleep/FIFO shell fixtures. No agents, model calls or user settings.
"""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile
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
           XDG_CONFIG_HOME=str(ROOT / 'config'), PATH=str(BIN) + os.pathsep + os.environ['PATH'])
ENV.pop('TMUX', None)
(ROOT / 'home').mkdir()
RESULTS = []


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


try:
    tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'scratch', '-x', '100', '-y', '28', 'sleep 100')
    anchor = windows()[0]
    for _ in range(2):
        subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    hooks = tmux('show-hooks', '-gw')
    assert hooks.count('window-layout-changed[90]') == 1, 'reload must replace its own hook'
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
finally:
    tmux('kill-server', check=False)
    (ROOT / 'results.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
    print('Evidence:', ROOT)
