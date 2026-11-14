#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Project leadership and both UIs on a private server with native Pi publishers.

cargo build && uv run tests/orchestrators_live.py target/debug/tmux-agent-plane
No model calls, installed binaries, real agent panes or user settings are changed.
"""
import fcntl
import hashlib
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

import pyte

SOURCE = Path(__file__).resolve().parents[1]
CLI = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else SOURCE / 'target/debug/tmux-agent-plane'
ROOT = Path(tempfile.mkdtemp(prefix='tap-orch-', dir='/tmp'))
print('Evidence:', ROOT, flush=True)
BIN = ROOT / 'bin'
STATE = ROOT / 's/tmux-agent-plane'
for folder in (BIN, STATE / 'pi', ROOT / 'home', ROOT / 'scratch'):
    folder.mkdir(parents=True, exist_ok=True)
TMUX = shutil.which('tmux')
ENV = dict(os.environ, HOME=str(ROOT / 'home'), XDG_STATE_HOME=str(ROOT / 's'),
           XDG_CONFIG_HOME=str(ROOT / 'config'), CLAUDE_CONFIG_DIR=str(ROOT / 'claude'),
           GROK_HOME=str(ROOT / 'grok'), GROK_LEADER_SOCKET=str(ROOT / 'no-grok.sock'),
           PLANE_NAV_ROOT=str(ROOT), PATH=str(BIN) + os.pathsep + os.environ['PATH'],
           TERM='xterm-256color', ENV='/dev/null')
for key in ('TMUX', 'TMUX_PANE', 'CODEX_THREAD_ID', 'CODEX_SESSION_ID', 'NO_COLOR'):
    ENV.pop(key, None)
WRAPPER = BIN / 'tmux'
WRAPPER.write_text(f'#!/bin/sh\ncase "$*" in *" notice"*) exit 0;; esac\nexec {shlex.quote(TMUX)} -S {shlex.quote(str(ROOT / "t"))} "$@"\n')
WRAPPER.chmod(0o755)
(BIN / 'tmux-agent-plane').symlink_to(CLI)
for tool in ('osascript', 'notify-send'):
    p = BIN / tool
    p.write_text('#!/bin/sh\nexit 0\n')
    p.chmod(0o755)
subprocess.run(['cc', '-Wall', '-Wextra', '-Werror', str(SOURCE / 'tests/fixtures/sidebar/agent.c'),
                '-o', str(BIN / 'pi')], check=True)
NAMER = BIN / 'namer'
NAMER.write_text(f'''#!{sys.executable}
import json, pathlib, sys, time
root = pathlib.Path({str(ROOT)!r})
data = json.loads(sys.argv[-1].split('Task data: ', 1)[1])
with (root / 'calls').open('a') as log: log.write(json.dumps(data) + '\\n')
if (root / 'retry').exists() and 'rejected_answer' not in data:
    print(json.dumps({{'name': 'this-agent-name-is-too-long', 'orchestrator': True}})); sys.exit(0)
while (root / 'hold').exists(): time.sleep(.03)
print(json.dumps({{'name': 'auto-p' + data['pane'][1:], 'orchestrator': True}}))
''')
NAMER.chmod(0o755)
CONFIG = ROOT / 'config/tmux-agent-plane/config.json'
SETTINGS = Path.home() / '.gemini/antigravity-cli/settings.json'
def settings_hash():
    return hashlib.sha256(SETTINGS.read_bytes()).hexdigest() if SETTINGS.exists() else None
BASELINE = settings_hash()
RESULTS, SPAWNED, CLIENTS = [], [], []


def plane(*args, check=True):
    result = subprocess.run([str(CLI), *map(str, args)], env=ENV, text=True,
                            capture_output=True, timeout=25)
    if check:
        assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result


def infra(*args, check=True):
    result = subprocess.run([str(WRAPPER), *map(str, args)], env=ENV, text=True,
                            capture_output=True, timeout=10)
    if check:
        assert result.returncode == 0, (args, result.stdout, result.stderr)
    return result.stdout.strip()


def agents():
    return json.loads(plane('list', '--json').stdout)


def by_pane(pane):
    return next((a for a in agents() if a['pane'] == pane), None)


def leaders(session):
    return [a['pane'] for a in agents() if a['session'] == session and a['orchestrator']]


def wait(label, predicate, timeout=15):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for client in CLIENTS:
            client.drain()
        try:
            result = predicate()
            if result:
                return result
        except (FileNotFoundError, json.JSONDecodeError):
            pass
        time.sleep(.04)
    for i, client in enumerate(CLIENTS):
        (ROOT / f'failure-{i}.txt').write_text(client.text())
    raise AssertionError(('timeout', label, ROOT))


def proof(label):
    RESULTS.append(label)
    print('PASS:', label, flush=True)


def spawn(name=None, session='harness'):
    args = ['spawn', 'pi', '--in', 'window', '--session', session, '--cwd', ROOT / 'scratch']
    if name:
        args += ['--name', name]
    pane = plane(*args).stdout.split()[1]
    SPAWNED.append(pane)
    return pane


def calls():
    return [json.loads(line) for line in (ROOT / 'calls').read_text().splitlines()] if (ROOT / 'calls').exists() else []


def naming(enabled):
    CONFIG.parent.mkdir(parents=True, exist_ok=True)
    config = {'auto_name': {'enabled': enabled, 'command': [str(NAMER), '{prompt}'], 'output': 'text'},
              'sidebar': {'family': False}}
    CONFIG.write_text(json.dumps(config))


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(120, 38)
        self.stream = pyte.ByteStream(self.screen)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 38, 120, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), 'attach-session', '-t', session], env=ENV,
                                        stdin=slave, stdout=slave, stderr=slave)
        os.close(slave)

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            try:
                self.stream.feed(os.read(self.master, 65536))
            except OSError:
                break

    def text(self):
        self.drain()
        return '\n'.join(self.screen.display)

    def key(self, value):
        os.write(self.master, value)

    def close(self):
        self.process.terminate()
        self.process.wait(timeout=5)
        os.close(self.master)


try:
    infra('-f', '/dev/null', 'new-session', '-d', '-s', 'harness', '-x', '120', '-y', '38', '/bin/sh')
    infra('set-option', '-g', 'default-shell', '/bin/sh')
    infra('set-option', '-g', 'default-command', '/bin/sh')
    infra('set-option', '-g', 'prefix', 'C-a')
    plane('daemon', 'start')
    worker = spawn('worker')
    lead = spawn('lead')
    assert leaders('harness') == []
    plane('orchestrator', 'set', 'lead')
    wait('lead promoted', lambda: leaders('harness') == [lead])
    assert '[orch]' in plane('list').stdout
    plane('orchestrator', 'set', 'worker')
    wait('role transfers', lambda: leaders('harness') == [worker])
    plane('orchestrator', 'clear', 'lead')
    wait('late demotion leaves successor', lambda: leaders('harness') == [worker])
    plane('orchestrator', 'clear', 'worker')
    wait('zero leaders after demotion', lambda: leaders('harness') == [])
    proof('zero or one leader, transfer, targeted demotion and text/JSON visibility')

    plane('orchestrator', 'set', 'lead')
    wait('lead promoted again', lambda: leaders('harness') == [lead])
    plane('rename', 'lead', 'project-lead')
    wait('name changed', lambda: by_pane(lead)['name'] == 'project-lead')
    plane('sessions', 'rename', 'harness', 'project')
    wait('project renamed', lambda: leaders('project') == [lead])
    plane('daemon', 'restart')
    wait('restart preserves role', lambda: leaders('project') == [lead])
    proof('agent and project renaming and daemon restarts preserve leadership')

    plane('sessions', 'create', 'destination', '--cwd', ROOT / 'scratch')
    plane('move', 'project-lead', '--session', 'destination')
    wait('role moves', lambda: leaders('destination') == [lead] and leaders('project') == [])
    rival = spawn('rival', 'project')
    plane('orchestrator', 'set', 'rival')
    wait('rival promoted', lambda: leaders('project') == [rival])
    refused = plane('move', 'project-lead', '--session', 'project', check=False)
    assert refused.returncode and 'already has an orchestrator' in refused.stderr, refused.stderr
    assert by_pane(lead)['session'] == 'destination'
    plane('orchestrator', 'clear', 'rival')
    wait('destination cleared', lambda: leaders('project') == [])
    plane('move', 'project-lead', '--session', 'project')
    wait('role returns', lambda: leaders('project') == [lead])
    proof('moves carry leadership to vacancies and refuse conflicting leaders')

    # A suspended naming result must still name its agent, but cannot take leadership after
    # the current agent explicitly reports being ordinary, or after an intervening handoff.
    naming(True)
    (ROOT / 'hold').touch()
    pending = spawn(session='project')
    wait('first naming call held', lambda: len(calls()) == 1)
    assert any(a['orchestrator'] for a in calls()[0]['agents'])
    plane('orchestrator', 'clear', by_pane(pending)['name'])
    (ROOT / 'hold').unlink()
    wait('named ordinary in led project', lambda: by_pane(pending)['name'].startswith('auto-'))
    assert leaders('project') == [lead]
    plane('orchestrator', 'clear', 'project-lead')
    wait('project vacant', lambda: leaders('project') == [])
    (ROOT / 'hold').touch()
    demoted = spawn(session='project')
    wait('second naming call held', lambda: len(calls()) == 2)
    plane('orchestrator', 'clear', by_pane(demoted)['name'])
    (ROOT / 'hold').unlink()
    wait('late answer still names', lambda: by_pane(demoted)['name'].startswith('auto-'))
    assert leaders('project') == []
    proof('naming uses project context and cannot override explicit demotion or another leader')

    (ROOT / 'hold').touch()
    handoff = spawn(session='project')
    wait('third naming call held', lambda: len(calls()) == 3)
    plane('orchestrator', 'set', 'project-lead')
    wait('lead elected explicitly', lambda: leaders('project') == [lead])
    plane('orchestrator', 'clear', 'project-lead')
    wait('explicit vacancy', lambda: leaders('project') == [])
    (ROOT / 'hold').unlink()
    wait('held answer settles', lambda: by_pane(handoff)['name'].startswith('auto-'))
    assert leaders('project') == []
    # Rejected-output retries must retain the original role snapshot too.
    (ROOT / 'retry').touch()
    (ROOT / 'hold').touch()
    retried = spawn(session='project')
    wait('naming retry held', lambda: len(calls()) == 5)
    assert 'rejected_answer' in calls()[-1]
    plane('orchestrator', 'clear', by_pane(retried)['name'])
    (ROOT / 'hold').unlink()
    wait('retry names without promotion', lambda: by_pane(retried)['name'].startswith('auto-'))
    assert leaders('project') == []
    (ROOT / 'retry').unlink()
    inferred = spawn(session='project')
    wait('background inference fills vacancy', lambda: leaders('project') == [inferred])
    naming(False)
    proof('intervening handoffs invalidate pending inference; fresh naming can fill a vacancy')

    # A process exits by its fixture control file, leaving its normal shell. A replacement
    # publisher in that same immutable pane must start ordinary.
    old_pid = by_pane(inferred)['process_id']
    (ROOT / f'{inferred}.command').write_text('exit 0 0')
    wait('owner exits', lambda: by_pane(inferred) is None)
    wait('vacancy retained', lambda: leaders('project') == [])
    (ROOT / f'{inferred}.command').unlink()
    plane('keys', inferred, 'p', 'i', 'Enter')
    wait('replacement native agent', lambda: by_pane(inferred) and by_pane(inferred)['process_id'] != old_pid)
    assert not by_pane(inferred)['orchestrator']
    proof('exit leaves no successor and replacement processes cannot inherit leadership')

    plane('orchestrator', 'set', 'project-lead')
    wait('lead for UI', lambda: leaders('project') == [lead])
    plane('sessions', 'create', 'elsewhere', '--cwd', ROOT / 'scratch', '--workspace', 'other')
    outsider = spawn('other-lead', 'elsewhere')
    plane('orchestrator', 'set', 'other-lead')
    wait('other workspace leader', lambda: leaders('elsewhere') == [outsider])
    plane('task', 'start', 'role-task', '--cwd', ROOT / 'scratch', '--', 'sleep', '120')
    subprocess.run(['bash', str(SOURCE / 'tmux-agent-plane.tmux')], env=ENV, check=True)
    client = Client('project')
    CLIENTS.append(client)
    wait('client attaches', lambda: infra('list-clients', '-F', '#{client_name}'))
    plane('focus', 'worker')
    plane('toggle-sidebar', worker)
    sidebar = wait('sidebar focused', lambda: next((line.split()[0] for line in
        infra('list-clients', '-F', '#{pane_id} #{@agent-plane-sidebar}').splitlines()
        if line.endswith(' 1')), None))
    def side():
        return plane('read', sidebar).stdout
    wait('role badge rendered', lambda: 'project-lead orch' in side())
    shown = side()
    assert shown.index('project-lead orch') < shown.index('worker'), shown
    assert 'other-lead' not in shown, shown
    wait('task shown normally', lambda: 'role-task' in side())
    plane('keys', sidebar, 'm')
    wait('sidebar filters', lambda: 'project-lead orch' in side() and 'worker' not in side() and 'rival' not in side() and 'role-task' not in side())
    (ROOT / f'{lead}.command').write_text('blocked 0 0')
    wait('blocked leader in sidebar snapshot', lambda: by_pane(lead)['state'] == 'blocked' and 'blocked' in side())
    plane('keys', sidebar, 'd')
    wait('state and role filters compose', lambda: 'd blocked' in side() and 'project-lead orch' in side())
    plane('keys', sidebar, 'C-c')
    wait('ctrl-c clears filters', lambda: 'worker' in side() and 'd all' in side())
    (ROOT / 'sidebar.txt').write_text(side())
    plane('keys', sidebar, 'q')
    wait('sidebar closes', lambda: not any(line.endswith(' 1') for line in
        infra('list-panes', '-a', '-F', '#{pane_id} #{@agent-plane-sidebar}').splitlines()))
    proof('sidebar pins and badges leader; m composes with state and Ctrl+C clears filters')

    # The actual popup receives keys from an attached client, not a separate pseudo-UI.
    client.key(b'\x01a')
    wait('picker renders badge', lambda: 'project-lead orch' in client.text())
    client.key(b'm')
    wait('picker filters', lambda: 'm orchestrators' in client.text())
    popup = client.text()
    # Agent terminal output behind the popup contains no names; these assertions are its rows.
    assert 'worker' not in popup and 'rival' not in popup, popup
    client.key(b'd')
    wait('picker combines filters', lambda: 'd blocked' in client.text() and 'project-lead orch' in client.text())
    (ROOT / 'picker.txt').write_text(client.text())
    client.key(b'\x03')
    wait('picker clears filters before closing', lambda: 'worker' in client.text() and 'm orchestrators' not in client.text())
    client.key(b'q')
    proof('picker shares badge, orchestrator filter, state composition and cancellation')

    # Malformed role data degrades only role reporting, leaving healthy tracking available.
    infra('set-option', '-t', '=project:', '@agent-plane-orchestrator', 'broken')
    wait('role fault visible', lambda: '[orchestrator:' in (STATE / 'daemon.log').read_text())
    assert by_pane(lead) and not by_pane(lead)['orchestrator']
    infra('set-option', '-u', '-t', '=project:', '@agent-plane-orchestrator')
    wait('role fault recovers', lambda: 'recovered' in (STATE / 'daemon.log').read_text())
    proof('malformed role metadata reports diagnostics without losing healthy agents')
    (ROOT / 'results.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
finally:
    (ROOT / 'hold').unlink(missing_ok=True)
    for client in CLIENTS:
        client.close()
    plane('task', 'stop', 'role-task', check=False)
    for pane in SPAWNED:
        plane('kill', pane, check=False)
    plane('daemon', 'stop', check=False)
    infra('kill-server', check=False)
    assert settings_hash() == BASELINE, 'real Antigravity settings changed'
