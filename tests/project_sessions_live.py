#!/usr/bin/env python3
"""Session/project operations on a private tmux server with native Pi fixtures.

cargo build && python3 tests/project_sessions_live.py [path/to/tmux-agent-plane]
No real agent CLI, provider, account, user tmux server or user settings are used.
Agent actions use the branch CLI. Raw tmux arranges synthetic topology and clients.
Every fixture records terminal bytes and publishes its PID/native identity.
"""
import json
import os
from pathlib import Path
import pty
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time

SOURCE = Path(__file__).resolve().parents[1]
CLI = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-proj-', dir='/tmp'))
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
           TERM='xterm-256color', TMUX_PANE='%999999')
ENV.pop('TMUX', None)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(
    f'#!{sys.executable}\nimport json, os, subprocess, sys\nfrom pathlib import Path\n'
    f'root = Path({str(ROOT)!r})\nargs = sys.argv[1:]\n'
    "with (root / 'calls.jsonl').open('a') as log: log.write(json.dumps(args) + '\\n')\n"
    "arm = root / 'race.json'\n"
    "if args[0] == 'if-shell' and any('break-pane' in a for a in args) and arm.exists():\n"
    "    change = json.loads(arm.read_text()); arm.unlink()\n"
    f"    subprocess.run([{TMUX!r}, '-S', {SOCKET!r}, *change], check=True, capture_output=True)\n"
    f'os.execv({TMUX!r}, [{TMUX!r}, "-S", {SOCKET!r}, *args])\n')
WRAPPER.chmod(0o755)
for tool in ('osascript', 'notify-send'):
    path = BIN / tool
    path.write_text('#!/bin/sh\nexit 0\n')
    path.chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)


def plane(*args, check=True):
    p = subprocess.run([str(CLI), *map(str, args)], env=ENV, text=True,
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


def agents():
    return json.loads(plane('list', '--json').stdout)


def agent(name):
    return next((a for a in agents() if a['name'] == name), None)


def sessions():
    return json.loads(plane('sessions', 'list', '--json').stdout)


def session(name):
    return next((s for s in sessions() if s['name'] == name), None)


def spawn(name, project, placement='window'):
    plane('spawn', 'pi', '--in', placement, '--session', project,
          '--cwd', ROOT / 'scratch', '--name', name)
    return agent(name)


def inputs():
    return {p.name: p.read_bytes() for p in ROOT.glob('*.input') if p.stat().st_size}


def mutation_calls():
    return [json.loads(line) for line in (ROOT / 'calls.jsonl').read_text().splitlines()
            if json.loads(line)[0] in ('load-buffer', 'paste-buffer', 'send-keys', 'respawn-pane')]


def move(name, project):
    before = agent(name)
    publisher = next(p for p in (ROOT / 's/tmux-agent-plane/pi').glob('*.json')
                     if json.loads(p.read_text())['pane_id'] == before['pane'])
    pid = int(publisher.stem)
    before_input, before_calls = inputs(), mutation_calls()
    output = plane('move', name, '--session', project).stdout
    expected = session(project)['name'] if not project.startswith('$') else next(s['name'] for s in sessions() if s['id'] == project)
    after = eventually('move indexed', lambda: (a if (a := agent(name)) and a['session'] == expected else None))
    os.kill(pid, 0)
    assert int(next(p for p in (ROOT / 's/tmux-agent-plane/pi').glob('*.json')
                    if json.loads(p.read_text())['pane_id'] == after['pane']).stem) == pid
    for field in ('pane', 'name', 'kind', 'parent', 'session_id', 'completion_epoch', 'completions',
                  'reported_completions', 'state', 'state_since', 'title', 'cwd'):
        assert after[field] == before[field], (field, before, after)
    assert inputs() == before_input, 'move leaked terminal input'
    assert mutation_calls() == before_calls, 'move injected input or respawned a pane'
    assert name in output and after['pane'] in output, output
    return after


def reject(*args, contains):
    before_input, before_calls = inputs(), mutation_calls()
    before_agents = {a['pane']: (a['session'], a['window']) for a in agents()}
    p = plane(*args, check=False)
    assert p.returncode and contains in p.stderr, (args, p)
    assert inputs() == before_input and mutation_calls() == before_calls
    assert {a['pane']: (a['session'], a['window']) for a in agents()} == before_agents
    return p.stderr


def proof(label):
    print('PASS:', label, flush=True)
    RESULTS.append(label)


def attach(project):
    master, slave = pty.openpty()
    client = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', project], env=ENV,
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
    eventually('client attached', lambda: session(project)['attached'] == 1)
    return client


RESULTS = []
CLIENTS = []
DAEMON = False
try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '100', '-y', '30', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', 'exit 79')
    plane('daemon', 'start')
    DAEMON = True
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    for project in ('api', 'web', 'archive', "quoted '$project"):
        plane('sessions', 'create', project, '--cwd', ROOT / 'scratch')
        assert session(project) and not session(project)['agents']
    names = [s['name'] for s in sessions()]
    assert names == sorted(names) and 'harness' in names
    reject('sessions', 'create', 'api', contains='duplicate session')
    reject('sessions', 'create', 'bad.name', contains='session name')
    reject('spawn', 'pi', '--in', 'window', '--session', 'ap', contains='exact name or id')
    reject('spawn', 'pi', '--session', 'api', contains='--session requires')
    proof('create/list include shell-only projects, exact names, duplicates and invalid placement fail')

    # Explicit session destination wins over tmux current session; its shell override applies.
    shell = BIN / 'project-shell'
    shell.write_text('#!/bin/sh\nprintf used >> ' + shlex.quote(str(ROOT / 'shell-used')) + '\nexec /bin/sh "$@"\n')
    shell.chmod(0o755)
    infra('set-option', '-t', session('api')['id'], 'default-shell', shell)
    parent = spawn('project-lead', 'api')
    assert parent['session'] == 'api' and parent['parent'] is None
    assert (ROOT / 'shell-used').exists()
    child = spawn('project-helper', 'web')
    # Native pane metadata, as assigned by a reliable caller; fixture-only setup.
    infra('set-option', '-p', '-t', child['pane'], '@agent-plane-parent', parent['pane'])
    (ROOT / (child['pane'] + '.command')).write_text('idle 3 0')
    eventually('unseen completions and parent', lambda: agent('project-helper')['state'] == 'done'
               and agent('project-helper')['parent'] == parent['pane'])
    child = move('project-helper', session('api')['id'])
    child = move('project-helper', "quoted '$project")
    plane('rename', 'project-helper', "quoted'helper")
    eventually('literal quoted agent name', lambda: agent("quoted'helper"))
    move("quoted'helper", 'api')
    named = spawn('fresh-lead', 'new-project', 'session')
    assert named['session'] == 'new-project' and session('new-project')['agents'] == ['fresh-lead']
    reject('spawn', 'pi', '--in', 'session', '--session', 'new-project', '--name', 'duplicate-lead', contains='duplicate session')
    proof('explicit spawn destination, session-local shell and named new project; moves retain PID, identity, parent and Done state')

    # Mixed-window fixture: unrelated shell and second agent stay in their source window.
    mixed = spawn('mixed-agent', 'web')
    other = spawn('other-agent', 'web')
    infra('join-pane', '-d', '-s', other['pane'], '-t', mixed['pane'])
    eventually('mixed fixture arrangement', lambda: agent('other-agent')['window'] == mixed['window'])
    shell_pane = infra('split-window', '-d', '-P', '-F', '#{pane_id}', '-t', mixed['pane'], '/bin/sh')
    other_before = agent('other-agent')
    move('mixed-agent', 'api')
    assert agent('other-agent')['window'] == other_before['window']
    assert infra('display-message', '-p', '-t', shell_pane, '#{window_id}') == mixed['window']
    proof('mixed source window keeps its other agent and unrelated shell')

    # An attached destination keeps the client's window, active pane and session unchanged.
    attach('archive')
    client_before = infra('list-clients', '-F', '#{client_name} #{session_id} #{window_id} #{pane_id}')
    move('mixed-agent', 'archive')
    assert infra('list-clients', '-F', '#{client_name} #{session_id} #{window_id} #{pane_id}') == client_before
    proof('detached move into an attached project leaves client selection unchanged')

    # Linked source and grouped destination are explicit refusals. Inventory exposes both links.
    linked = spawn('linked-fixture', 'web')
    infra('link-window', '-s', linked['window'], '-t', session('archive')['id'] + ':')
    assert 'linked-fixture' in session('web')['agents'] and 'linked-fixture' in session('archive')['agents']
    eventually('linked canonical row', lambda: agent('linked-fixture')['session'] == 'archive')
    reject('move', 'linked-fixture', '--session', 'api', contains='linked')
    infra('unlink-window', '-t', session('archive')['id'] + ':' + linked['window'])
    infra('new-session', '-d', '-s', 'grouped-copy', '-t', session('web')['id'])
    reject('move', 'mixed-agent', '--session', 'grouped-copy', contains='grouped')
    reject('spawn', 'pi', '--in', 'window', '--session', 'grouped-copy', '--name', 'grouped-spawn', contains='grouped')
    infra('kill-session', '-t', 'grouped-copy')  # only unlinks synthetic fixtures; source processes remain
    proof('linked-window inventory and linked/grouped refusals preserve every agent')

    # Inject a native topology race after the CLI's planning snapshot, before its native guard.
    racing = spawn('race-fixture', 'api')
    (ROOT / 'race.json').write_text(json.dumps(['link-window', '-s', racing['window'], '-t', session('archive')['id'] + ':']))
    reject('move', 'race-fixture', '--session', 'archive', contains='topology changed')
    infra('unlink-window', '-t', session('archive')['id'] + ':' + racing['window'])
    proof('native command-queue guard refuses a newly linked source after planning')

    # Run the real sidebar hook, including final-window/session cleanup.
    plane('toggle-sidebar', parent['window'])
    final = spawn('final-fixture', 'final-project', 'session')
    plane('follow-sidebar', final['window'])
    eventually('final sidebar', lambda: infra('list-panes', '-t', final['window'], '-F', '#{@agent-plane-sidebar}').splitlines().count('1') == 1)
    move('final-fixture', 'api')
    eventually('detached source session removed', lambda: session('final-project') is None)
    assert agent('final-fixture')

    attached = spawn('attached-final', 'attached-project', 'session')
    plane('follow-sidebar', attached['window'])
    client = attach('attached-project')
    # Attaching itself reports native focus to the active fixture, asynchronously.
    # Settle that infrastructure event before measuring the refused move's input.
    eventually('native attachment focus', lambda: (ROOT / (attached['pane'] + '.input')).exists())
    reject('move', 'attached-final', '--session', 'api', contains='attached session')
    assert client.poll() is None
    proof('sidebar cleanup removes detached final source; final attached project is protected')

    assert inputs() == {attached['pane'] + '.input': b'\x1b[I'}, inputs()
    assert not mutation_calls(), mutation_calls()
    (ROOT / 'results.json').write_text(json.dumps(dict(passed=True, cases=RESULTS,
        move_injected_terminal_input_bytes=0, input_or_respawn_calls=0,
        native_client_setup_focus_bytes=3), indent=2))
    proof('moves/refusals inject zero terminal bytes; no input API calls or agent restarts')
finally:
    # The CLI owns every fixture shutdown; the server is then infrastructure-only.
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
