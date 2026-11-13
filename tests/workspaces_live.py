#!/usr/bin/env python3
"""Workspaces on a private tmux server with native Pi fixtures.

cargo build && python3 tests/workspaces_live.py [path/to/tmux-agent-plane]
No real agent CLI, provider, account, user tmux server or user settings are used.
Raw tmux arranges clients and hand-made sessions; everything the plane does goes through its CLI
or its real sidebar.
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
ROOT = Path(tempfile.mkdtemp(prefix='tap-ws-', dir='/tmp'))
print('Evidence:', ROOT, flush=True)
TMUX = shutil.which('tmux')
assert TMUX and CLI.exists(), 'tmux, cc and cargo build required'
BIN = ROOT / 'bin'
for folder in (BIN, ROOT / 'home', ROOT / 's/tmux-agent-plane/pi', ROOT / 'scratch'):
    folder.mkdir(parents=True, exist_ok=True)
SOCKET = str(ROOT / 't')
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'no-grok.sock'),
           PLANE_NAV_ROOT=str(ROOT), PATH=str(BIN) + os.pathsep + os.environ['PATH'],
           TERM='xterm-256color', TMUX_PANE='%999999', ENV='/dev/null')
ENV.pop('TMUX', None)
for key in ('CODEX_THREAD_ID', 'CODEX_SESSION_ID'):
    ENV.pop(key, None)
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


def plane(*args, check=True, env=None):
    p = subprocess.run([str(CLI), *map(str, args)], env=env or ENV, text=True,
                       capture_output=True, timeout=40)
    if check:
        assert p.returncode == 0, (args, p.stdout, p.stderr)
    return p


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
    return json.loads(plane('list', '--json').stdout)


def agent(name):
    return next((a for a in agents() if a['name'] == name), None)


def agent_in(pane):
    return next((a for a in agents() if a['pane'] == pane), None)


def workspaces():
    return {w['name']: w for w in json.loads(plane('workspace', 'list', '--json').stdout)}


def label(session):
    return infra('display-message', '-p', '-t', f'={session}:', '#{@agent-plane-workspace}')


def status(session):
    return infra('display-message', '-p', '-t', f'={session}:', '#{@agent_plane_status}')


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
    return infra('list-clients', '-F', '#{client_name}')


def type_to_client(master, keys):
    """Mark the client as used now, as typing would."""
    os.write(master, keys)


def proof(label):
    print('PASS:', label, flush=True)
    RESULTS.append(label)


RESULTS = []
CLIENTS = []
DAEMON = False
try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '120', '-y', '40', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', '/bin/sh')
    infra('set-option', '-g', 'destroy-unattached', 'off')
    infra('new-session', '-d', '-s', 'before', '-c', folder('before'), '/bin/sh')
    plane('daemon', 'start')
    DAEMON = True
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    eventually('existing sessions join main', lambda: label('harness') == 'main' and label('before') == 'main')
    assert list(workspaces()) == ['main'], workspaces()
    assert workspaces()['main']['current']
    proof('every existing session joins the first workspace, main')

    plane('spawn', 'pi', '--in', 'window', '--session', 'harness', '--cwd', ROOT / 'scratch', '--name', 'main-lead')
    out = plane('sessions', 'create', 'api', '--cwd', ROOT / 'scratch', '--workspace', 'client-x').stdout
    assert out.strip().endswith('client-x'), out
    assert label('api') == 'client-x'
    plane('spawn', 'pi', '--in', 'window', '--session', 'api', '--cwd', ROOT / 'scratch', '--name', 'x-helper')
    plane('spawn', 'pi', '--in', 'session', '--session', 'lab-bench', '--workspace', 'research',
          '--cwd', ROOT / 'scratch', '--name', 'r-lead')
    assert label('lab-bench') == 'research'
    eventually('agents carry workspaces', lambda: {a['name']: a['workspace'] for a in agents()}
               == {'main-lead': 'main', 'x-helper': 'client-x', 'r-lead': 'research'})
    reject = plane('spawn', 'pi', '--in', 'window', '--session', 'api', '--workspace', 'other',
                   '--cwd', ROOT / 'scratch', check=False)
    assert reject.returncode and 'requires --in session' in reject.stderr, reject
    reject = plane('sessions', 'create', 'bad', '--workspace', 'has.dot', check=False)
    assert reject.returncode and 'workspace names' in reject.stderr, reject
    ws = workspaces()
    assert sorted(ws) == ['client-x', 'main', 'research']
    assert [s['name'] for s in ws['client-x']['sessions']] == ['api'] and ws['client-x']['agents'] == ['x-helper']
    assert 'WORKSPACE' in plane('list').stdout
    proof('sessions create and spawn --in session start sessions in a named workspace; agents carry it')

    # Each session's status bar counts only its own workspace and names it.
    eventually('per-workspace status', lambda: 'client-x' in status('api') and 'main' in status('harness'))
    assert '● 1' in status('api') or '○ 1' in status('api') or '✓ 1' in status('api'), status('api')
    for session, name in (('api', 'client-x'), ('harness', 'main'), ('lab-bench', 'research')):
        text = status(session)
        others = {'client-x', 'main', 'research'} - {name}
        assert name in text and not any(o in text for o in others), (session, text)
    proof('each session status names its workspace and counts only its agents')

    # A session made by hand joins the workspace the client used last was in.
    client = attach('harness')
    infra('new-session', '-d', '-s', 'hand-main', '-c', folder('hand'), '/bin/sh')
    eventually('hand session in main', lambda: label('hand-main') == 'main')
    infra('switch-client', '-c', client, '-t', '=api')
    eventually('client in client-x', lambda: workspaces()['client-x']['current'])
    infra('new-session', '-d', '-s', 'hand-x', '-c', folder('hand'), '/bin/sh')
    eventually('hand session in client-x', lambda: label('hand-x') == 'client-x')
    proof('a session made by hand joins the workspace you are in')

    # Navigation stays in the client's workspace.
    plane('spawn', 'pi', '--in', 'window', '--session', 'hand-x', '--cwd', ROOT / 'scratch', '--name', 'x-second')
    eventually('second client-x agent', lambda: agent('x-second'))
    infra('switch-client', '-c', client, '-t', agent('x-helper')['pane'])
    seen = []
    for _ in range(4):
        plane('cycle', 'next', '--client', client)
        seen.append(infra('list-clients', '-F', '#{pane_id}'))
    names = {agent_in(p)['name'] for p in seen}
    assert names == {'x-helper', 'x-second'}, (seen, names)
    proof('agent keys cycle only through the workspace of the client')

    # Switching shows the session used last in a workspace.
    out = plane('workspace', 'switch', 'research').stdout
    assert 'lab-bench' in out, out
    assert infra('list-clients', '-F', '#{client_session}') == 'lab-bench'
    plane('workspace', 'switch', 'client-x')
    assert infra('list-clients', '-F', '#{client_session}') == agent_in(seen[-1])['session']
    infra('switch-client', '-c', client, '-t', '=hand-x')
    reject = plane('workspace', 'switch', 'nowhere', check=False)
    assert reject.returncode and 'no workspace named' in reject.stderr, reject
    proof('workspace switch shows the session used last there; unknown names fail')

    # Whole sessions move between workspaces; their agents and clients stay put in tmux.
    before = agent('x-second')
    out = plane('workspace', 'move', 'hand-x', 'hand-main', '--to', 'research').stdout
    assert 'hand-x' in out and 'hand-main' in out
    eventually('moved agent', lambda: agent('x-second')['workspace'] == 'research')
    after = agent('x-second')
    for field in ('pane', 'session', 'window', 'location', 'state', 'cwd'):
        assert before[field] == after[field], field
    assert infra('list-clients', '-F', '#{client_session}') == 'hand-x'
    eventually('client follows its session', lambda: workspaces()['research']['current'])
    reject = plane('workspace', 'move', 'agent-plane-tasks', '--to', 'main', check=False)
    assert reject.returncode, reject
    proof('workspace move takes whole sessions, with agents and clients, into another workspace')

    # Rename keeps every session and refuses a name in use.
    plane('workspace', 'rename', 'research', 'lab')
    eventually('renamed', lambda: 'lab' in workspaces() and 'research' not in workspaces())
    assert label('lab-bench') == label('hand-x') == 'lab'
    reject = plane('workspace', 'rename', 'lab', 'main', check=False)
    assert reject.returncode and 'reserved' in reject.stderr, reject
    proof('workspace rename relabels its sessions and refuses an existing name')

    # Automatic folder projects stay inside the agent's workspace.
    plane('sessions', 'create', 'shared-folder', '--cwd', folder('shared-folder'), '--workspace', 'main')
    stay = infra('new-window', '-d', '-P', '-F', '#{pane_id}', '-t', '=api:', '-c', folder('shared-folder'),
                 '/bin/sh', '-c', 'pi')
    eventually('hand agent indexed', lambda: agent_in(stay))
    time.sleep(2.5)
    assert agent_in(stay)['session'] == 'api', agent_in(stay)
    moved = infra('new-window', '-d', '-P', '-F', '#{pane_id}', '-t', '=api:', '-c', folder('fresh-folder'),
                  '/bin/sh', '-c', 'pi')
    eventually('hand agent placed', lambda: (a := agent_in(moved)) and a['session'] == 'fresh-folder')
    assert label('fresh-folder') == 'client-x'
    proof("a hand-started agent's folder project is created in its own workspace, never joined across")

    # The sidebar lists its own workspace only and names it.
    plane('workspace', 'switch', 'client-x')
    plane('toggle-sidebar', agent('x-helper')['window'])
    sidebar = eventually('sidebar pane', lambda: next(
        (line.split()[0] for line in infra('list-panes', '-t', agent('x-helper')['window'], '-F',
                                           '#{pane_id} #{@agent-plane-sidebar}').splitlines()
         if line.endswith(' 1')), None))

    def screen():
        return infra('capture-pane', '-p', '-t', sidebar)
    eventually('sidebar names its workspace', lambda: 'agents · client-x' in screen())
    text = eventually('sidebar lists client-x agents', lambda: (s := screen()) and 'x-helper' in s and s)
    assert 'main-lead' not in text and 'r-lead' not in text and 'x-second' not in text, text
    proof('the sidebar lists only its own workspace and names it')

    # w opens the workspaces; Enter on another shows it.
    infra('switch-client', '-c', client, '-t', '=api')
    infra('select-pane', '-t', sidebar)
    infra('send-keys', '-t', sidebar, 'w')
    eventually('menu open', lambda: screen().startswith('workspaces') and '• client-x  here' in screen())
    menu = screen()
    for name in ('client-x', 'lab', 'main'):
        assert name in menu, menu
    infra('send-keys', '-t', sidebar, 'j')
    infra('send-keys', '-t', sidebar, 'Enter')
    eventually('client in lab', lambda: workspaces()['lab']['current'])
    proof('w lists every workspace and Enter shows the highlighted one')

    # Closing a session keeps its client in the workspace, though tmux picks another one's.
    plane('sessions', 'create', 'doomed', '--cwd', ROOT / 'scratch', '--workspace', 'client-x')
    infra('switch-client', '-c', client, '-t', '=lab-bench')
    time.sleep(1.1)
    infra('switch-client', '-c', client, '-t', '=doomed')
    eventually('client in doomed', lambda: workspaces()['client-x']['current'])
    infra('kill-session', '-t', '=doomed')
    landed = eventually('client back in client-x', lambda: (s := infra(
        'list-clients', '-F', '#{client_session}')) in ('api', 'fresh-folder') and s, timeout=4)
    proof(f'a client whose session closes stays in its workspace ({landed})')

    plane('daemon', 'stop')
    DAEMON = False
    plane('daemon', 'start')
    DAEMON = True
    eventually('restarted daemon keeps labels', lambda: sorted(workspaces()) == ['client-x', 'lab', 'main'])
    proof('labels live on tmux sessions and survive a daemon restart')

    # Close a workspace through the UI, preserving its sessions and agent identities.
    infra('switch-client', '-c', client, '-t', '=api')
    infra('select-pane', '-t', sidebar)
    infra('send-keys', '-t', sidebar, 'w')
    eventually('close menu', lambda: 'workspaces' in screen() and 'here' in screen())
    infra('send-keys', '-t', sidebar, 'C-c')
    eventually('ctrl-c exits menu', lambda: 'agents · client-x' in screen())
    infra('send-keys', '-t', sidebar, 'w', 'x')
    eventually('move prompt', lambda: 'close client-x?' in screen() and 'move to main' in screen())
    infra('send-keys', '-t', sidebar, 'n')
    eventually('terminate prompt', lambda: 'of client-x?' in screen() and 'Every agent in them stops' in screen())
    infra('send-keys', '-t', sidebar, 'C-c')
    eventually('cancelled close', lambda: '• client-x  here' in screen())
    before = agent('x-helper')
    infra('send-keys', '-t', sidebar, 'x', 'Enter')
    eventually('moved to main', lambda: 'client-x' not in workspaces() and label('api') == 'main')
    after = agent('x-helper')
    assert before['pane'] == after['pane'] and before['session'] == after['session']
    eventually('main named with workspace hint', lambda: 'w workspaces' in screen() and 'agents · main' in screen())
    assert plane('workspace', 'close', 'main', check=False).returncode
    assert plane('workspace', 'rename', 'main', 'other', check=False).returncode
    proof('close defaults to moving intact sessions; Ctrl-C cancels both screens; main is protected')

    # Terminate a workspace while viewing it, returning its client to main.
    plane('sessions', 'create', 'close-a', '--workspace', 'closing')
    plane('sessions', 'create', 'close-b', '--workspace', 'closing')
    infra('switch-client', '-c', client, '-t', '=close-a')
    eventually('closing workspace indexed', lambda: 'closing' in workspaces())
    window = infra('display-message', '-p', '-t', '=close-a', '#{window_id}')
    plane('enter-sidebar', window)
    sidebar = eventually('closing sidebar', lambda: next(
        (line.split()[0] for line in infra('list-panes', '-t', window, '-F',
                                           '#{pane_id} #{@agent-plane-sidebar}').splitlines()
         if line.endswith(' 1')), None))
    eventually('closing workspace UI', lambda: 'agents · closing' in screen())
    infra('select-pane', '-t', sidebar)
    infra('send-keys', '-t', sidebar, 'w', 'x')
    eventually('closing move prompt', lambda: 'close closing?' in screen() and 'Its 2 sessions move to main,' in screen())
    infra('send-keys', '-t', sidebar, 'n')
    eventually('closing terminate prompt', lambda: 'end 2 sessions of closing?' in screen())
    infra('send-keys', '-t', sidebar, 'Enter')
    eventually('terminated workspace gone', lambda: 'closing' not in workspaces())
    remaining = infra('list-sessions', '-F', '#{session_name}').splitlines()
    assert 'close-a' not in remaining and 'close-b' not in remaining
    assert label(infra('list-clients', '-F', '#{client_session}')) == 'main'
    proof('explicit termination closes every session and returns the attached client to main')

    (ROOT / 'results.json').write_text(json.dumps(dict(passed=True, cases=RESULTS), indent=2))
finally:
    if sys.exc_info()[0] is not None and 'sidebar' in globals():
        try:
            print('UI at failure:', screen(), flush=True)
            print('Clients:', infra('list-clients', '-F', '#{client_name} #{client_session}'), flush=True)
        except Exception as error:
            print('UI unavailable:', error, flush=True)
    if DAEMON:
        try:
            for a in agents():
                plane('kill', a['name'], check=False)
            plane('daemon', 'stop', check=False)
        except Exception as error:
            print('cleanup:', error, file=sys.stderr)
    subprocess.run([TMUX, '-S', SOCKET, 'kill-server'], env=ENV, capture_output=True)
    for client, master in CLIENTS:
        client.wait(timeout=5)
        os.close(master)
