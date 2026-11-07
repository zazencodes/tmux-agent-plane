#!/usr/bin/env python3
"""A hand-started agent joins its folder's session, with the client viewing it, on a private server.

cargo build && python3 tests/auto_project_live.py [path/to/tmux-agent-plane]
Agents are native Pi fixtures. No real agent CLI, user tmux server or user settings are used.
"""
import json
import os
from pathlib import Path
import pty
import shutil
import subprocess
import sys
import tempfile
import threading
import time

SOURCE = Path(__file__).resolve().parents[1]
CLI = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-auto-', dir='/tmp'))
print('Evidence:', ROOT, flush=True)
TMUX = shutil.which('tmux')
assert TMUX and CLI.exists(), 'tmux, cc and cargo build required'
BIN = ROOT / 'bin'
for folder in (BIN, ROOT / 'home', ROOT / 's/tmux-agent-plane/pi'):
    folder.mkdir(parents=True, exist_ok=True)
SOCKET = str(ROOT / 't')
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'no-grok.sock'),
           PLANE_NAV_ROOT=str(ROOT), PATH=str(BIN) + os.pathsep + os.environ['PATH'],
           TERM='xterm-256color', TMUX_PANE='%999999', ENV='/dev/null')
ENV.pop('TMUX', None)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(f'#!/bin/sh\nexec {TMUX} -S {SOCKET} "$@"\n')
WRAPPER.chmod(0o755)
for tool in ('osascript', 'notify-send'):
    path = BIN / tool
    path.write_text('#!/bin/sh\nexit 0\n')
    path.chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)


def plane(*args):
    p = subprocess.run([str(CLI), *map(str, args)], env=ENV, text=True,
                       capture_output=True, timeout=40)
    assert p.returncode == 0, (args, p.stdout, p.stderr)
    return p.stdout


def infra(*args):
    p = subprocess.run([str(WRAPPER), *map(str, args)], env=ENV, text=True,
                       capture_output=True, timeout=10)
    assert p.returncode == 0, (args, p.stderr)
    return p.stdout.strip()


def eventually(label, predicate, timeout=12):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.05)
    raise AssertionError(('timeout', label, ROOT))


def folder(name):
    path = ROOT.resolve() / 'work' / name
    path.mkdir(parents=True, exist_ok=True)
    return path


def agents():
    return json.loads(plane('list', '--json'))


def agent_in(pane):
    return next((a for a in agents() if a['pane'] == pane), None)


def session_names():
    return [s['name'] for s in json.loads(plane('sessions', 'list', '--json'))]


def hand_start(session, name):
    """As if typed: a window in the session, then the agent runs in its folder."""
    return infra('new-window', '-d', '-P', '-F', '#{pane_id}', '-t', f'{session}:', '-c', folder(name),
                 '/bin/sh', '-c', 'pi')


def lands_in(pane, session):
    return eventually(f'{pane} in {session}', lambda: (a := agent_in(pane)) and a['session'] == session and a)


def stays(pane, session):
    time.sleep(2.5)
    assert agent_in(pane)['session'] == session, agent_in(pane)


def attach(session):
    master, slave = pty.openpty()
    client = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', session], env=ENV,
                              stdin=slave, stdout=slave, stderr=slave)
    os.close(slave)
    CLIENTS.append((client, master))
    def drain():
        try:
            while os.read(master, 65536):
                pass
        except OSError:
            pass
    threading.Thread(target=drain, daemon=True).start()
    eventually('client attached', lambda: infra('list-clients', '-F', '#{client_session}') == session)
    return client, master


def type_a_command(session, name):
    """A shell in the folder, on the client's screen, where the user types the agent's command."""
    infra('send-keys', '-t', f'{session}:', f'cd {folder(name)} && exec pi', 'Enter')
    return infra('display-message', '-p', '-t', f'{session}:', '#{pane_id}')


def proof(label):
    print('PASS:', label, flush=True)


CLIENTS = []
DAEMON = False
try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '100', '-y', '30', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', '/bin/sh')
    infra('set-option', '-g', 'destroy-unattached', 'off')
    early = hand_start('harness', 'running-before-daemon')
    eventually('fixture published', lambda: list((ROOT / 's/tmux-agent-plane/pi').glob('*.json')))
    plane('daemon', 'start')
    DAEMON = True
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    eventually('existing agent indexed', lambda: agent_in(early))
    stays(early, 'harness')
    proof('an agent already running when the daemon starts stays where it is')

    first = hand_start('harness', 'alpha')
    lands_in(first, 'alpha')
    second = hand_start('harness', 'alpha')
    lands_in(second, 'alpha')
    assert session_names().count('alpha') == 1
    dotted = hand_start('harness', 'my.app')
    lands_in(dotted, 'my-app')
    proof('new agents get a session named after their folder, reuse it, and dots become dashes')

    infra('new-session', '-d', '-s', 'manual', '-c', folder('elsewhere'), '/bin/sh')
    plane('move', agent_in(first)['name'], '--session', 'manual')
    lands_in(first, 'manual')
    stays(first, 'manual')
    proof('a manual move sticks')

    plane('spawn', 'pi', '--in', 'window', '--session', 'manual', '--cwd', folder('spawned'), '--name', 'chosen')
    chosen = next(a for a in agents() if a['name'] == 'chosen')
    stays(chosen['pane'], 'manual')
    proof('a spawned agent keeps the session it was asked for')

    shell_beside = infra('split-window', '-d', '-P', '-F', '#{pane_id}', '-t', 'harness:', '-c', folder('split'),
                         '/bin/sh', '-c', 'pi')
    eventually('split agent indexed', lambda: agent_in(shell_beside))
    stays(shell_beside, 'harness')
    proof('an agent sharing a window with other work stays')

    # On screen, with other windows in the session: the client goes with the agent.
    infra('new-session', '-d', '-s', 'busy', '-c', folder('busy'), '/bin/sh')
    infra('new-window', '-d', '-t', 'busy:', '-c', folder('busy'), '/bin/sh')
    client, master = attach('busy')
    pane = type_a_command('busy', 'gamma')
    lands_in(pane, 'gamma')
    assert infra('list-clients', '-F', '#{client_session} #{pane_id}') == f'gamma {pane}'
    assert 'busy' in session_names()
    os.write(master, b'x')
    eventually('typing reaches the moved agent', lambda: b'x' in (ROOT / f'{pane}.input').read_bytes())
    proof('the viewing client moves with the agent and its keyboard still reaches it')
    infra('detach-client', '-s', 'gamma')
    client.wait(timeout=5)

    # The session's only window: the session ends, and the client must not.
    infra('new-session', '-d', '-s', 'solo', '-c', folder('solo'), '/bin/sh')
    client, master = attach('solo')
    pane = type_a_command('solo', 'delta')
    lands_in(pane, 'delta')
    eventually('source session gone', lambda: 'solo' not in session_names())
    assert client.poll() is None
    assert infra('list-clients', '-F', '#{client_session} #{pane_id}') == f'delta {pane}'
    proof('moving the last window of an attached session keeps the client on the agent')
    infra('detach-client', '-s', 'delta')
    client.wait(timeout=5)

    # The usual arrangement: sidebars open everywhere, keyboard in the content pane.
    infra('new-session', '-d', '-s', 'side', '-c', folder('side'), '/bin/sh')
    client, master = attach('side')
    window = infra('display-message', '-p', '-t', 'side:', '#{window_id}')
    plane('toggle-sidebar', window)
    eventually('sidebar open', lambda: '1' in infra('list-panes', '-t', 'side:', '-F', '#{@agent-plane-sidebar}'))
    infra('select-pane', '-t', infra('list-panes', '-t', 'side:', '-f', '#{!=:#{@agent-plane-sidebar},1}',
                                     '-F', '#{pane_id}'))
    pane = type_a_command('side', 'epsilon')
    lands_in(pane, 'epsilon')
    eventually('source session gone with its sidebar', lambda: 'side' not in session_names())
    eventually('sidebar follows', lambda: '1' in infra('list-panes', '-t', agent_in(pane)['window'],
                                                       '-F', '#{@agent-plane-sidebar}'))
    assert infra('list-clients', '-F', '#{client_session} #{pane_id}') == f'epsilon {pane}'
    assert client.poll() is None
    proof('with sidebars open the client stays on the agent and the sidebar follows')

    log = (ROOT / 's/tmux-agent-plane/daemon.log').read_text()
    assert 'could not place' not in log, log
    proof('no placement faults')
finally:
    if DAEMON:
        try:
            for a in agents():
                plane('kill', a['name'])
            plane('daemon', 'stop')
        except Exception as error:
            print('cleanup:', error, file=sys.stderr)
    subprocess.run([TMUX, '-S', SOCKET, 'kill-server'], env=ENV, capture_output=True)
    for client, master in CLIENTS:
        client.wait(timeout=5)
        os.close(master)
