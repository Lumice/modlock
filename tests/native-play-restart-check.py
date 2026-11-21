#!/usr/bin/env python3
"""Play after the app or the helper dies: the next state comes from the helper.

Each case uses a disposable real-helper library, a fake Steam and a helper
wrapper that injects one fault. No case may start Steam twice, reopen Play or
editing while a Steam handoff is unsettled, or leave an app launch file behind.
"""
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

from nativeintegrationcheck import BEFORE, vpk

binary, helper = map(lambda p: str(Path(p).resolve()), sys.argv[1:3])
only = sys.argv[3:]

WRAPPER = r'''#!/usr/bin/python3
"""Forward JSON lines to the real helper and inject PLAY_FAULT once."""
import json,os,signal,subprocess,sys,threading
fault=os.environ.get('PLAY_FAULT','')
used=os.environ['PLAY_FAULT_USED']
if os.path.exists(used): fault=''
child=subprocess.Popen([os.environ['REAL_HELPER']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
def trip():
    open(used,'w').close()
def die():
    trip();child.kill();child.wait();os._exit(1)
def output():
    for line in child.stdout:
        result=json.loads(line).get('result')
        if fault.endswith('-prepared') and isinstance(result,dict) and result.get('handoffState')=='dispatched':
            # The helper has asked Steam; the app never learns the result.
            if fault=='die-prepared': die()
            trip();return
        sys.stdout.write(line);sys.stdout.flush()
reader=threading.Thread(target=output,daemon=True);reader.start()
for line in sys.stdin:
    if fault.endswith('-commit') and json.loads(line).get('method')=='native.launch.commit':
        # The helper never receives the commit.
        if fault=='die-commit': die()
        trip();continue
    child.stdin.write(line);child.stdin.flush()
child.stdin.close();child.wait();reader.join(timeout=5)
'''

# The number older builds stored in their marker for BackendClient::Call::LaunchCommit.
OLD_LAUNCH_COMMIT = 34

class Case:
    def __init__(self, temp, changed):
        self.root = root = Path(temp)
        self.game, self.data, source = root / 'game', root / 'native', root / 'source/state.json'
        addons = self.game / 'game/citadel/addons'
        source.parent.mkdir(parents=True)
        addons.mkdir(parents=True)
        (self.game / 'game/bin/win64').mkdir(parents=True)
        (self.game / 'game/citadel/pak01_dir.vpk').write_bytes(vpk())
        (addons / 'pak01_dir.vpk').write_bytes(vpk())
        (addons / '.dmm.json').write_text(json.dumps({'version': 3, 'mods': {'remote-a': {'enabled': True, 'order': 0, 'shard': 1, 'currentVpks': ['pak01_dir.vpk'], 'disabledVpks': [], 'originalVpkNames': ['a_dir.vpk']}}}))
        mod = {'id': 'local-a', 'remoteId': 'remote-a', 'name': 'Tiny fixture', 'status': 'installed', 'installOrder': 0}
        source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': [mod], 'profiles': {'default': {'name': 'Default', 'mods': [mod]}}}}}))
        (self.game / 'game/citadel/gameinfo.gi').write_bytes(BEFORE)
        self.env = env = os.environ.copy()
        for key in list(env):
            if key.startswith('XDG_') or key in ('DISPLAY', 'WAYLAND_DISPLAY'): del env[key]
        for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR'):
            directory = root / key
            directory.mkdir(mode=0o700)
            env[key] = str(directory)
        process = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        def rpc(method, **params):
            process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {'dataDir': str(self.data), **params}}) + '\n')
            process.stdin.flush()
            result = json.loads(process.stdout.readline())
            assert 'result' in result, result
            return result['result']
        review = rpc('native.importPreview', statePath=str(source), gamePath=str(self.game))
        rpc('native.importCommit', token=review['token'], operationId='import', expectedRevision='0')
        review = rpc('native.applyPreview', profileId='default', gamePath=str(self.game), deploymentLayout='shards-v1')
        snapshot = rpc('native.apply', token=review['token'], operationId='initial', expectedRevision='1', deploymentLayout='shards-v1')
        self.revision = int(snapshot['revision'])
        if changed:
            # A saved toggle makes Play run a real Apply, which advances the revision.
            mod_id = snapshot['mods'][0]['id']
            self.revision = int(rpc('native.setEnabled', profileId='default', modId=mod_id, enabled=False, operationId='toggle', expectedRevision=str(self.revision))['revision'])
        process.stdin.close()
        assert process.wait(timeout=20) == 0
        self.changed = changed
        fake = root / 'bin'; fake.mkdir()
        steam = fake / 'steam'
        steam.write_text('#!/usr/bin/python3\nimport os,sys\nwith open(os.environ["STEAM_LOG"],"a") as f: f.write(" ".join(sys.argv[1:])+"\\n")\n')
        steam.chmod(0o700)
        self.wrapper = root / 'fault-helper'
        self.wrapper.write_text(WRAPPER)
        self.wrapper.chmod(0o700)
        self.used = root / 'fault-used'
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', UI_NATIVE_GAME=str(self.game), UI_NATIVE_SOURCE=str(source),
                   UI_NATIVE_CONFIRM_REVIEW='1', PATH=str(fake) + os.pathsep + env['PATH'], STEAM_LOG=str(root / 'steam.log'),
                   REAL_HELPER=helper, PLAY_FAULT_USED=str(self.used))
        self.pending = root / 'XDG_CONFIG_HOME/dl-mod-manager/state/pending'
        self.key = hashlib.sha256(str(self.data).encode()).hexdigest()

    def command(self, scenario):
        return [binary, '--helper', str(self.wrapper), '--data-dir', str(self.data), '--check-ui', scenario]

    def ui(self, scenario, timeout=45, **extra):
        result = subprocess.run(self.command(scenario), env={**self.env, **extra}, capture_output=True, text=True, timeout=timeout)
        assert result.returncode == 0 and 'PASS UI' in result.stdout, f'{scenario}: {result.stdout}\n{result.stderr}'
        return result.stdout

    def rpc(self, method, **params):
        request = {'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {'dataDir': str(self.data), **params}}
        result = subprocess.run([helper], input=json.dumps(request) + '\n', capture_output=True, text=True, env=self.env, timeout=20)
        response = json.loads(result.stdout)
        assert result.returncode == 0 and 'result' in response, response
        return response['result']

    def kill_app_at(self, fault):
        """Start Play and kill the app, without any shutdown, once the fault has tripped."""
        app = subprocess.Popen(self.command('native-launch-seed'), env={**self.env, 'PLAY_FAULT': fault}, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 30
            while not self.used.exists() and app.poll() is None and time.monotonic() < deadline:
                time.sleep(.02)
            assert self.used.exists() and app.poll() is None, 'the fault point was not reached\n' + (''.join(app.communicate()) if app.poll() is not None else '')
            app.send_signal(signal.SIGKILL)
        finally:
            if app.poll() is None: app.kill()
            app.communicate()
        self.wait_for_root()

    def wait_for_root(self):
        """The orphaned helper retires on end of input; wait until it has released the library."""
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            probe = subprocess.run([helper], input=json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': 'native.snapshot', 'params': {'dataDir': str(self.data)}}) + '\n', capture_output=True, text=True, env=self.env, timeout=20)
            if '"result"' in probe.stdout: return
            time.sleep(.1)
        raise AssertionError('the library stayed busy after the app was killed')

    def steam_starts(self):
        log = self.root / 'steam.log'
        return len(log.read_text().splitlines()) if log.exists() else 0

    def app_launch_files(self):
        """Saved launch context of the app itself. A cleared marker is an empty object; the settings files are not launch context."""
        return sorted(p.name for p in self.pending.glob('*') if p.name in (self.key + '.json', self.key + '.launch.json') and p.read_text().strip() not in ('', '{}'))

    def journals(self):
        with sqlite3.connect(self.data / 'library.sqlite3') as db:
            return [json.loads(row[0]) for row in db.execute('SELECT body FROM journals')]

    def library_revision(self):
        with sqlite3.connect(self.data / 'library.sqlite3') as db:
            return int(json.loads(db.execute('SELECT body FROM library').fetchone()[0])['revision'])

    def last_attempt(self):
        with sqlite3.connect(self.data / 'library.sqlite3') as db:
            return json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])

def killed_before_commit(case):
    # The app dies with its commit unsent. Nothing ran, so the next start is ready to play.
    case.kill_app_at('hold-commit')
    assert case.steam_starts() == 0
    assert case.app_launch_files() == [], case.app_launch_files()
    case.ui('native-launch-clean')
    assert case.library_revision() == case.revision and not case.journals()
    assert case.steam_starts() == 0 and case.app_launch_files() == []

def killed_after_handoff(case):
    # The app dies after the helper asked Steam but before the app saw the result.
    case.kill_app_at('hold-prepared')
    assert case.steam_starts() == 1
    assert [j['kind'] for j in case.journals()] == ['launchHandoff']
    assert case.library_revision() == case.revision + case.changed
    assert case.app_launch_files() == [], case.app_launch_files()
    # The next start remembers the handoff: Play and editing stay closed until it is cleared.
    case.ui('native-launch-held')
    assert [j['kind'] for j in case.journals()] == ['launchHandoff']
    case.ui('native-launch-held', UI_HELD_CLEAR='1')
    assert not case.journals() and case.last_attempt()['outcome']['reason'] == 'steamIntentCancelled'
    case.ui('native-launch-clean')
    assert case.steam_starts() == 1 and case.app_launch_files() == []

def lost_after_handoff(case):
    # The helper dies after asking Steam, while the launch owns the connection.
    out = case.ui('native-play-loss', PLAY_FAULT='die-prepared', UI_LOSS_EXPECT='handoff')
    assert f'revision after loss: {case.revision + case.changed}\n' in out, out
    assert case.steam_starts() == 1 and not case.journals() and case.app_launch_files() == []
    assert case.last_attempt()['outcome']['reason'] == 'steamIntentCancelled'

def lost_before_commit(case):
    # The helper dies before it receives the commit. Nothing ran.
    out = case.ui('native-play-loss', PLAY_FAULT='die-commit', UI_LOSS_EXPECT='none')
    assert f'revision after loss: {case.revision}\n' in out, out
    assert case.steam_starts() == 0 and not case.journals() and case.app_launch_files() == []

def old_files(case, unsent=False, recovery_id=False):
    """Leave an older build's context, without upgrading it into proof."""
    if recovery_id:
        case.rpc('native.recover', operationId='older-recovery', expectedRevision=str(case.library_revision()))
        assert case.rpc('native.operation', operationId='older-recovery')['status'] == 'succeeded'
    case.kill_app_at('hold-commit' if unsent else 'hold-prepared')
    attempt = case.last_attempt()
    begin = attempt['beginInput']['params']
    reference = {'version': 1, 'root': str(case.data), **{key: begin[key] for key in ('epoch', 'attemptId', 'profileId', 'mode', 'expectedRevision', 'settingsRevision')}}
    ready = attempt['ready']
    marker = {key: value for key, value in reference.items() if key != 'version'}
    marker.update(launchContextVersion=1, method=OLD_LAUNCH_COMMIT, operationId=ready['preparationOperationId'], reviewToken=ready['reviewToken'], preparationInput=ready['preparationInput'])
    if recovery_id:
        marker['recoveryId'] = 'older-recovery'
    text = json.dumps(marker, separators=(',', ':'))
    case.pending.mkdir(parents=True, exist_ok=True)
    (case.pending / (case.key + '.launch.json')).write_text(json.dumps(reference, separators=(',', ':')))
    (case.pending / (case.key + '.json')).write_text(text)
    return text

def old_build_files(case):
    # Even the old launch's own handoff is no longer special settlement evidence.
    text = old_files(case)
    for _ in range(2):
        case.ui('native-launch-unproven')
        assert (case.pending / (case.key + '.json')).read_text() == text, 'an unproven Play marker was cleared'
        assert [j['kind'] for j in case.journals()] == ['launchHandoff']
    assert case.steam_starts() == 1

def old_unsent_recovery_id(case):
    # A historical successful Recover cannot prove an unsent preparation succeeded.
    text = old_files(case, unsent=True, recovery_id=True)
    for _ in range(2):
        case.ui('native-launch-unproven', UI_UNPROVEN_NO_HANDOFF='1', UI_UNPROVEN_RECOVER='1')
        assert (case.pending / (case.key + '.json')).read_text() == text, 'Recover discarded an unknown original'
        assert not case.journals() and case.steam_starts() == 0

def old_handoff_recovery_id(case):
    # The old recovery receipt does not prove that today's handoff has settled either.
    text = old_files(case, recovery_id=True)
    case.ui('native-launch-unproven')
    assert (case.pending / (case.key + '.json')).read_text() == text, 'an old recovery receipt cleared the marker'
    assert [j['kind'] for j in case.journals()] == ['launchHandoff']
    assert case.steam_starts() == 1

CASES = [(killed_before_commit, True), (killed_after_handoff, True), (killed_after_handoff, False),
         (lost_after_handoff, True), (lost_after_handoff, False), (lost_before_commit, True), (old_build_files, True),
         (old_build_files, False), (old_unsent_recovery_id, True), (old_handoff_recovery_id, True)]
for function, changed in CASES:
    name = f"{function.__name__}:{'changed' if changed else 'unchanged'}"
    if only and name not in only and function.__name__ not in only: continue
    with tempfile.TemporaryDirectory(prefix='play-restart-') as temp:
        function(Case(temp, changed))
    print('PASS ' + name, flush=True)
