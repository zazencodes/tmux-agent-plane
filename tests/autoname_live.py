#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# dependencies = ["pyte==0.8.2"]
# ///
"""Private auto-naming proof: native Pi fixtures and a naming fixture; zero model calls.

uv run tests/autoname_live.py target/debug/tmux-agent-plane
PTY decoding asserts UI only. Agent state comes exclusively from native fixtures/index.
Evidence persists in /tmp/tap-auto-*. Agent jobs use the branch CLI.
"""
import fcntl
from collections import Counter
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

ROOT = Path(tempfile.mkdtemp(prefix="tap-auto-", dir="/tmp"))
SOURCE = Path(__file__).resolve().parent
BINARY = Path(sys.argv[1]).resolve()
TMUX = shutil.which("tmux")
assert TMUX
BIN = ROOT / "bin"
BIN.mkdir()
STATE = ROOT / "s" / "tmux-agent-plane"
(STATE / "pi").mkdir(parents=True)
(ROOT / "home").mkdir()
(ROOT / "claude" / "sessions").mkdir(parents=True)
ENV = dict(os.environ, HOME=str(ROOT / "home"), XDG_STATE_HOME=str(ROOT / "s"),
           CLAUDE_CONFIG_DIR=str(ROOT / "claude"), GROK_HOME=str(ROOT / "grok"),
           GROK_LEADER_SOCKET=str(ROOT / "absent.sock"), PLANE_NAV_ROOT=str(ROOT), XDG_CONFIG_HOME=str(ROOT / "config"), TERM="xterm-256color",
           PATH=str(BIN) + os.pathsep + os.environ["PATH"])
ENV.pop("TMUX", None)
ENV.pop("NO_COLOR", None)  # The agent tool environment disables colours; this PTY tests them.
# Keep Codex identity intact, use explicit names/window placements for agent jobs.
WRAPPER = BIN / "tmux"
WRAPPER.write_text(f"#!{sys.executable}\nimport json, os, sys\n"
    f"with open({str(ROOT / 'tmux.calls')!r}, 'a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n"
    f"if 'switch-client' in sys.argv and os.path.exists({str(ROOT / 'switch.fail')!r}):\n"
    "    print('injected switch failure', file=sys.stderr); sys.exit(23)\n"
    f"if 'if-shell' in sys.argv and '@agent-plane-name' in ' '.join(sys.argv) and os.path.exists({str(ROOT / 'race.arm')!r}):\n"
    f"    open({str(ROOT / 'race.ready')!r}, 'w').close()\n"
    f"    import time\n    while not os.path.exists({str(ROOT / 'race.release')!r}): time.sleep(0.01)\n"
    "if 'display-popup' in sys.argv: sys.exit(0)\n"  # Diagnostics are asserted from daemon.log; popups would consume setup test keys.
    f"os.execv({TMUX!r}, [{TMUX!r}, '-S', {str(ROOT / 't.sock')!r}, *sys.argv[1:]])\n")
WRAPPER.chmod(0o755)
(BIN / "tmux-agent-plane").symlink_to(BINARY)
for tool in ("osascript", "notify-send"):
    path = BIN / tool
    path.write_text("#!/bin/sh\necho delivered >> " + shlex.quote(str(ROOT / "desktop.delivered")) + "\n")
    path.chmod(0o755)
fixture = ROOT / "fixture"
subprocess.run(["cc", "-Wall", "-Wextra", "-Werror", str(SOURCE / "fixtures/sidebar/agent.c"), "-o", str(fixture)], check=True)
(BIN / "pi").symlink_to(fixture)


def tmux(*args, check=True):
    return subprocess.run([str(WRAPPER), *args], env=ENV, check=check, text=True,
                          capture_output=True, timeout=8).stdout.strip()


def plane(*args):
    result = subprocess.run([str(BINARY), *args], env=ENV, check=True, text=True,
                            capture_output=True, timeout=20)
    output = result.stdout.strip()
    if args[0] == 'spawn':
        name, pane, _ = output.split()
        FIXTURES[name] = pane
    return output


def index():
    return json.loads((STATE / "agents.json").read_text())


class Screen(pyte.Screen):
    def report_device_status(self, mode, **kwargs):
        pass


class Client:
    def __init__(self, session):
        self.screen = Screen(100, 28)
        self.stream = pyte.ByteStream(self.screen)
        self.master, self.slave = pty.openpty()
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", 28, 100, 0, 0))
        self.process = subprocess.Popen([str(WRAPPER), "attach-session", "-t", session], env=ENV,
            stdin=self.slave, stdout=self.slave, stderr=self.slave)
        self.raw = (ROOT / f"{session}.raw").open("wb")

    def drain(self):
        while select.select([self.master], [], [], 0)[0]:
            data = os.read(self.master, 65536)
            self.raw.write(data)
            self.raw.flush()
            self.stream.feed(data)

    def snapshot(self, label):
        self.drain()
        (ROOT / f"{self.process.pid}-{label}.txt").write_text("\n".join(self.screen.display) + "\n")

    def footer(self):
        return self.screen.display[-1][:38]

    def context(self):
        rows = tmux("list-clients", "-F", "#{client_pid} #{client_name} #{pane_id} #{window_id} #{session_name}")
        row = next(line for line in rows.splitlines() if line.split()[0] == str(self.process.pid))
        return row.split()[1:]

    def selected(self):
        self.drain()
        return next((line[1:38].split()[1] for line in self.screen.display if line[1:2] == "▌" and len(line[1:38].split()) > 1), None)

    def text(self):
        self.drain()
        return "\n".join(line[:38] for line in self.screen.display)

    def key(self, value):
        os.write(self.master, value)

    def close(self):
        self.process.terminate()
        try:
            self.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.raw.close()
        os.close(self.master)
        os.close(self.slave)


CLIENTS = []


def wait(label, condition, timeout=15):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        try:
            if condition():
                return
        except (FileNotFoundError, json.JSONDecodeError, StopIteration):
            pass
        time.sleep(0.05)
    for client in CLIENTS:
        client.snapshot("failure")
    raise AssertionError(f"timeout: {label}; evidence {ROOT}")


# No real agent binary is reachable through these fixture names.
NAMER = BIN / 'namer-fixture'
NAMER.write_text(f'''#!{sys.executable}
import json, os, pathlib, sys, time
root = pathlib.Path({str(ROOT)!r})
prompt = sys.argv[-1]
data = json.loads(prompt.split('Task data: ', 1)[1])
record = {{'data': data, 'cwd': os.getcwd(), 'tmux': os.getenv('TMUX'), 'pane': os.getenv('TMUX_PANE'), 'stdin': sys.stdin.read()}}
with (root / 'namer.calls').open('a') as log: log.write(json.dumps(record) + '\\n')
while (root / 'namer.hold').exists(): time.sleep(0.05)
mode = (root / 'namer.mode').read_text() if (root / 'namer.mode').exists() else 'good'
if mode == 'bad': print('`bad-output`')
elif mode == 'retry' and 'rejected_answer' not in data: print('much-too-long-for-an-agent-name')
elif mode == 'fail': print('fixture failed', file=sys.stderr); sys.exit(7)
elif mode == 'duplicate': print('keep-name')
else: print('auto-p' + data['title'].rsplit('%', 1)[-1])
''')
NAMER.chmod(0o755)
(BIN / 'agy').symlink_to(NAMER)
CONFIG = ROOT / 'config/tmux-agent-plane/config.json'
FIXTURES = {}
PANES = []
RESULTS = {}


def by_pane(pane):
    return next(a for a in index()['agents'] if a['pane'] == pane)


def spawn(name=None, placement='window'):
    args = ['spawn', 'pi', '--in', placement, '--cwd', str(ROOT)]
    if name:
        args += ['--name', name]
    result = plane(*args)
    pane = result.split()[1]
    PANES.append(pane)
    return pane


def calls():
    path = ROOT / 'namer.calls'
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def settle(seconds=1.3):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        for client in CLIENTS:
            client.drain()
        time.sleep(0.03)


def save_config(command=None, **changes):
    config = json.loads(CONFIG.read_text())
    if command is not None:
        config['auto_name']['command'] = command
        config['auto_name']['output'] = 'text'
    config['auto_name'].update(changes)
    temp = CONFIG.with_suffix('.tmp')
    temp.write_text(json.dumps(config))
    temp.replace(CONFIG)


def automatic_commands():
    return [json.loads(line) for line in (ROOT / 'tmux.calls').read_text().splitlines()
            if '@agent-plane-name' in line and 'if-shell' in line]


try:
    tmux('-f', '/dev/null', 'new-session', '-d', '-s', 'scratch', '-x', '100', '-y', '28', '/bin/sh')
    tmux('set-option', '-g', 'default-shell', '/bin/sh')
    tmux('set-option', '-g', 'default-command', '/bin/sh')
    tmux('set-option', '-g', 'status-position', 'top')
    plane('daemon', 'start')
    wait('index', lambda: (STATE / 'agents.json').exists())
    default = spawn()
    preserved = spawn('keep-name', placement='session')
    # User-supplied names that look like a default are explicit, too.
    explicit_default = spawn('pi-999')
    CLIENTS.append(Client('scratch'))
    client = CLIENTS[0]
    wait('client attached', lambda: tmux('list-clients', '-F', '#{client_name}'))
    plane('focus', by_pane(default)['name'])
    plane('toggle-sidebar', by_pane(default)['window'])
    wait('hint at width 38', lambda: 'c configure' in client.text())
    settle(2)
    assert not calls() and not automatic_commands()
    RESULTS['off_no_model_or_extra_naming_tmux_commands'] = True
    # Help is a local modal: closing q/x/Enter never reaches list actions.
    wait('compact help hint', lambda: '? keys' in client.footer())
    selected_before = client.selected()
    for closing in [b'q', b'x', b'\r', b'j']:
        client.key(b'?')
        wait('help opens', lambda: 'Keyboard shortcuts' in client.text())
        assert 'Configure' in client.text()
        assert 'c configure' not in client.text()
        client.snapshot('shortcuts-full-' + closing.hex())
        client.key(closing)
        wait('help closes safely', lambda: 'v off' in client.text() and 'c configure' in client.text())
        assert client.selected() == selected_before
        assert '? [Y/n]' not in client.text()
        assert len(index()['agents']) == 3
    # Search and rename retain ? as ordinary text.
    client.key(b'/?')
    wait('literal question in search', lambda: '/ ?▏' in client.text())
    assert 'Keyboard shortcuts' not in client.text()
    client.key(b'\x1b')
    wait('search cancelled before rename', lambda: '/ search' in client.text())
    client.key(b'r?')
    wait('literal question in rename', lambda: '?▏' in client.text())
    assert 'Keyboard shortcuts' not in client.text()
    client.key(b'\x1b')
    wait('rename cancelled', lambda: 'save' not in client.footer())
    # Help temporarily covers a kill confirmation and restores it without replay.
    client.key(b'x')
    wait('kill confirmation before help', lambda: '? [Y/n]' in client.text())
    client.key(b'?')
    wait('help covers confirmation', lambda: 'Keyboard shortcuts' in client.text())
    assert '? [Y/n]' not in client.text()
    client.key(b'x')
    wait('closing x restores confirmation', lambda: '? [Y/n]' in client.text())
    client.key(b'n')
    wait('confirmation cancelled', lambda: '? [Y/n]' not in client.text())
    # A short window pages the same list; every closing key is still consumed.
    window = by_pane(default)['window']
    original_height = tmux('display-message', '-p', '-t', window, '#{window_height}')
    tmux('resize-window', '-t', window, '-y', '10')
    seen_help = ''
    for page in range(4):
        client.key(b'?')
        wait('short help page', lambda: 'Keyboard shortcuts' in client.text())
        seen_help += client.text()
        client.snapshot('shortcuts-short-' + str(page))
        client.key(b'q')
        wait('short help close', lambda: 'v off' in client.text())
    assert 'Move selection' in seen_help and 'Configure: turn AI names off' in seen_help
    assert 'Configure' in seen_help
    tmux('resize-window', '-t', window, '-y', original_height)
    wait('full height restored', lambda: '? keys' in client.footer())
    RESULTS.update(help_real_keypress=True, help_closing_keys_consumed=True,
                   help_literal_during_edits=True, help_preserves_confirmation=True,
                   help_short_height_all_pages_reachable=True)
    client.key(b'c')
    wait('configure menu', lambda: '5 Antigravity (agy)' in client.text())
    client.key(b'\x1b')
    wait('configure closes', lambda: 'v off' in client.text())
    # The supported agy preset resolves to our private fake binary, never the real CLI.
    client.key(b'c5')
    wait('disclosure before enable', lambda: '⏎ enable' in client.text() and 'What is sent' in client.text() and 'Model: gemini-3.8-flash' in client.text() and 'Thinking: low' in client.text())
    client.snapshot('agy-consent-38-columns')
    assert not calls()
    client.key(b'\r')
    wait('default renamed', lambda: by_pane(default)['name'] == 'auto-p' + default[1:])
    assert by_pane(preserved)['name'] == 'keep-name'
    assert by_pane(explicit_default)['name'] == 'pi-999'
    assert len(calls()) == 1, calls()
    call = calls()[0]
    assert Path(call['cwd']).resolve() == (STATE / 'auto-name-work').resolve() and call['tmux'] is None and call['pane'] is None and call['stdin'] == ''
    assert set(call['data']) == {'title', 'cwd', 'detail'}
    RESULTS.update(setup_by_real_keypress=True, agy_preset_fixture_only=True, consent_before_execution=True,
                   default_renamed=True, explicit_names_preserved=True, detached_no_input=True)
    # The button stays once naming is on: there is always a way back to configuration.
    wait('configure stays after enabling', lambda: 'c configure' in client.text())
    assert not (set(json.loads(CONFIG.read_text())) - {'auto_name'})
    assert 'hide_hint' not in json.loads(CONFIG.read_text())['auto_name']
    RESULTS['configure_always_available'] = True
    # Exercise custom configuration in the actual sidebar as well.
    client.key(b'c7')
    wait('custom editor', lambda: 'Custom command' in client.text())
    command = [str(NAMER), '{prompt}']
    client.key(json.dumps(command).encode() + b'\r')
    wait('custom review', lambda: '⏎ enable' in client.text() and str(NAMER)[:12] in client.text())
    client.key(b'\r')
    wait('custom saved', lambda: json.loads(CONFIG.read_text())['auto_name']['command'] == command)
    RESULTS['custom_setup_by_keypress'] = True
    # Hold the background worker: the daemon must continue updating its index.
    (ROOT / 'namer.hold').touch()
    racing = spawn()
    wait('worker running', lambda: len(calls()) == 2)
    at = index()['updated_at']
    wait('daemon continues during AI', lambda: index()['updated_at'] > at + 1000)
    # Intercept the actual assignment after its detection snapshot, before tmux sees it.
    (ROOT / 'race.arm').touch()
    (ROOT / 'namer.hold').unlink()
    wait('assignment intercepted', lambda: (ROOT / 'race.ready').exists())
    plane('rename', by_pane(racing)['name'], 'human-wins')
    (ROOT / 'race.release').touch()
    wait('manual rename survives atomic guard', lambda: by_pane(racing)['name'] == 'human-wins')
    settle(2)
    assert by_pane(racing)['name'] == 'human-wins'
    (ROOT / 'race.arm').unlink()
    RESULTS.update(daemon_keeps_ticking=True, racing_manual_rename_wins=True)
    # Another session may claim the proposed name after the daemon snapshot.
    (ROOT / 'race.ready').unlink()
    (ROOT / 'race.release').unlink()
    (ROOT / 'namer.hold').touch()
    collision = spawn()
    wait('collision worker running', lambda: len(calls()) == 3)
    queued = spawn()
    settle(2)
    assert len(calls()) == 3, 'more than one naming worker ran at once'
    plane('rename', by_pane(queued)['name'], 'queue-human')
    (ROOT / 'race.arm').touch()
    (ROOT / 'namer.hold').unlink()
    wait('collision assignment intercepted', lambda: (ROOT / 'race.ready').exists())
    plane('rename', by_pane(preserved)['name'], 'auto-p' + collision[1:])
    (ROOT / 'race.release').touch()
    settle(2)
    assert by_pane(collision)['name'] == 'pi-' + collision[1:]
    assert by_pane(queued)['name'] == 'queue-human'
    plane('rename', by_pane(preserved)['name'], 'keep-name')
    (ROOT / 'race.arm').unlink()
    RESULTS.update(racing_cross_session_collision_rejected=True, one_worker_at_a_time=True)
    # Disable/re-enable a default agent after reserving: the ledger survives restart.
    (ROOT / 'namer.mode').write_text('bad')
    bad = spawn()
    wait('bad output retried once', lambda: len(calls()) == 5)
    assert calls()[4]['data']['rejected_answer'] == '`bad-output`\n'
    wait('bad output diagnostic', lambda: '[auto-name:worker] ERROR:' in (STATE / 'daemon.log').read_text())
    assert by_pane(bad)['name'] == 'pi-' + bad[1:]
    assert 'it answered "`bad-output`\\n"' in (STATE / 'daemon.log').read_text()
    settle(3)
    assert len(calls()) == 5
    assert (STATE / 'daemon.log').read_text().count('[auto-name:worker] ERROR:') == 1
    assert 'auto-name:worker' in tmux('show-options', '-gqv', '@agent_plane_status')
    plane('daemon', 'restart')
    settle(3)
    assert len(calls()) == 5, 'failed attempts repeated after restart'
    assert (STATE / 'daemon.log').read_text().count('[auto-name:worker] ERROR:') == 1, 'restart raised a stale fault'
    assert 'auto-name:worker' not in tmux('show-options', '-gqv', '@agent_plane_status')
    RESULTS.update(bad_output_rejected=True, failure_logged_and_deduplicated=True, attempts_survive_restart=True, restart_drops_stale_fault=True)
    # A rejected answer is corrected by its retry, without a fault.
    (ROOT / 'namer.mode').write_text('retry')
    corrected = spawn()
    wait('retry names the agent', lambda: by_pane(corrected)['name'] == 'auto-p' + corrected[1:])
    assert len(calls()) == 7
    assert calls()[6]['data']['rejected_answer'] == 'much-too-long-for-an-agent-name\n'
    assert (STATE / 'daemon.log').read_text().count('[auto-name:worker] ERROR:') == 1
    RESULTS['rejected_answer_corrected_by_retry'] = True
    # Change command explicitly to acknowledge the fault and clear global backoff.
    (ROOT / 'namer.mode').write_text('duplicate')
    save_config([str(NAMER), 'fixture-duplicate', '{prompt}'])
    duplicate = spawn()
    wait('duplicate name rejected', lambda: len(calls()) == 9 and 'already the name' in (STATE / 'daemon.log').read_text())
    assert by_pane(duplicate)['name'] == 'pi-' + duplicate[1:]
    RESULTS['duplicate_output_rejected'] = True
    # Missing executables fail once without affecting live tracking.
    save_config([str(BIN / 'missing-ai'), '{prompt}'])
    missing = spawn()
    wait('missing command diagnostic', lambda: 'not installed or executable' in (STATE / 'daemon.log').read_text())
    assert len(index()['agents']) == len(PANES)
    assert len(calls()) == 9
    # Setup reports missing commands without enabling them.
    client.key(b'c7')
    wait('custom editor again', lambda: 'Custom command' in client.text())
    client.key(json.dumps([str(BIN / 'missing-ai'), '{prompt}']).encode() + b'\r')
    wait('missing command shown in setup', lambda: 'executable' in client.text())
    client.snapshot('missing-command')
    client.key(b'\x1b')
    settle(0.2)
    client.key(b'\x1b')
    RESULTS['missing_command_visible_tracking_healthy'] = True
    # Explicit disabling is also persisted by the sidebar flow.
    client.key(b'cd')
    wait('disabled saved', lambda: not json.loads(CONFIG.read_text())['auto_name']['enabled'])
    wait('configure stays after disabling', lambda: 'c configure' in client.text())
    RESULTS['explicit_disable'] = True
    before = len(calls())
    late = spawn()
    settle(2)
    assert len(calls()) == before and by_pane(late)['name'] == 'pi-' + late[1:]
    RESULTS['disable_persists_no_more_calls'] = True
    # Configuration and naming-state corruption degrade naming, while tracking continues.
    good_config = CONFIG.read_bytes()
    CONFIG.write_text('{broken')
    wait('malformed config fault', lambda: '[auto-name:config] ERROR:' in (STATE / 'daemon.log').read_text())
    at = index()['updated_at']
    wait('tracking survives config fault', lambda: index()['updated_at'] > at)
    CONFIG.write_bytes(good_config)
    wait('config recovery', lambda: '[auto-name:config] recovered' in (STATE / 'daemon.log').read_text())
    plane('daemon', 'stop')
    (STATE / 'auto-name.json').write_text('{broken')
    plane('daemon', 'start')
    wait('state fault remains scoped', lambda: '[auto-name:state] ERROR:' in (STATE / 'daemon.log').read_text())
    at = index()['updated_at']
    wait('tracking survives state fault', lambda: index()['updated_at'] > at)
    assert len(calls()) == before
    RESULTS.update(config_and_ledger_faults_keep_tracking=True)
    assert len(index()['agents']) == len(PANES), 'naming process entered the agent index'
    # The fixture also records terminal focus reports, which are not input to an agent.
    typed = [(ROOT / (pane + '.input')).read_bytes().replace(b'\x1b[I', b'').replace(b'\x1b[O', b'')
             for pane in PANES if (ROOT / (pane + '.input')).exists()]
    assert not any(typed), ('unintended fixture input', typed)
    RESULTS.update(passed=True, models_used=0, real_servers_used=0, unintended_agent_input=0)
finally:
    for path in [ROOT / 'namer.hold', ROOT / 'race.arm']:
        path.unlink(missing_ok=True)
    (ROOT / 'race.release').touch()
    for pane in PANES:
        try:
            plane('kill', by_pane(pane)['name'])
        except subprocess.CalledProcessError:
            pass
    try:
        plane('daemon', 'stop')
    finally:
        for client in CLIENTS:
            client.close()
        tmux('kill-server', check=False)
    (ROOT / 'results.json').write_text(json.dumps(RESULTS, indent=2) + '\n')
    print(json.dumps(RESULTS, indent=2))
    print('Evidence:', ROOT)
