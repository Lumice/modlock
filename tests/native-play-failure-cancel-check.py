#!/usr/bin/env python3
"""A genuine route failure must survive simultaneous Cancel and helper restart."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid

from nativeintegrationcheck import BEFORE, vpk

helper = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='native-play-stale-') as temp:
    root = Path(temp)
    game, data, source = root / 'game', root / 'native', root / 'source/state.json'
    addons = game / 'game/citadel/addons'
    source.parent.mkdir(parents=True)
    addons.mkdir(parents=True)
    (game / 'game/bin/win64').mkdir(parents=True)
    (game / 'game/citadel/pak01_dir.vpk').write_bytes(vpk())
    (addons / 'pak01_dir.vpk').write_bytes(vpk())
    (addons / '.dmm.json').write_text(json.dumps({'version': 3, 'mods': {'remote-a': {'enabled': True, 'order': 0, 'shard': 1, 'currentVpks': ['pak01_dir.vpk'], 'disabledVpks': [], 'originalVpkNames': ['a_dir.vpk']}}}))
    mod = {'id': 'local-a', 'remoteId': 'remote-a', 'name': 'Tiny fixture', 'status': 'installed', 'installOrder': 0}
    source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': [mod], 'profiles': {'default': {'name': 'Default', 'mods': [mod]}}}}}))
    (game / 'game/citadel/gameinfo.gi').write_bytes(BEFORE)
    env = {k: v for k, v in os.environ.items() if not k.startswith('XDG_')}
    for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR'):
        directory = root / key
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    fake = root / 'bin'
    fake.mkdir()
    steam = fake / 'steam'
    steam.write_text('#!/bin/sh\nprintf called > "$STEAM_LOG"\n')
    steam.chmod(0o700)
    steam.unlink()
    (fake / 'sha256sum').symlink_to('/usr/bin/sha256sum')
    flatpak = fake / 'flatpak'
    flatpak.write_text("#!/usr/bin/python3\nimport os,time\nfrom pathlib import Path\np=Path(os.environ['ROUTE_STARTED'])\np.touch()\ndeadline=time.monotonic()+1.5\nwhile not Path(os.environ['ROUTE_RELEASE']).exists() and time.monotonic()<deadline: time.sleep(.005)\nraise SystemExit(1)\n")
    flatpak.chmod(0o700)
    env['PATH'] = str(fake)
    env['ROUTE_STARTED'] = str(root / 'route-started')
    env['ROUTE_RELEASE'] = str(root / 'route-release')
    env['STEAM_LOG'] = str(root / 'steam.log')
    process = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    number = 0
    def rpc(method, **params):
        global number
        number += 1
        process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': number, 'method': method, 'params': {'dataDir': str(data), **params}}) + '\n')
        process.stdin.flush()
        response = json.loads(process.stdout.readline())
        assert 'result' in response, response
        return response['result']
    def command(method, **params):
        control = uuid.uuid4().hex
        rpc(method, controlId=control, **params)
        for _ in range(3000):
            result = rpc('native.launch.control', controlId=control)
            if result['state'] == 'succeeded':
                return result['value']
            assert result['state'] == 'pending', result
            time.sleep(.01)
        raise AssertionError('launch command timed out')
    try:
        review = rpc('native.importPreview', statePath=str(source), gamePath=str(game))
        rpc('native.importCommit', token=review['token'], operationId='import', expectedRevision='0')
        review = rpc('native.applyPreview', profileId='default', gamePath=str(game), deploymentLayout='shards-v1')
        rpc('native.apply', token=review['token'], operationId='initial', expectedRevision='1', deploymentLayout='shards-v1')
        epoch = command('native.launch.open', openId=uuid.uuid4().hex)['epoch']
        attempt = uuid.uuid4().hex
        command('native.launch.begin', epoch=epoch, attemptId=attempt, profileId='default', mode='modded', expectedRevision='2', settingsRevision='0')
        for _ in range(3000):
            status = rpc('native.launch.status', epoch=epoch, attemptId=attempt)
            if status['phase'] == 'ready':
                break
            assert status['phase'] != 'failed', status
            time.sleep(.01)
        else:
            raise AssertionError('review did not finish')
        ready = status['attempt']['ready']
        assert not ready['changed'], ready
        control = uuid.uuid4().hex
        rpc('native.launch.commit', controlId=control, epoch=epoch, attemptId=attempt, reviewToken=ready['reviewToken'], preparationOperationId=ready['preparationOperationId'], preparationInput=ready['preparationInput'], settingsRevision='0')
        deadline = time.monotonic() + 10
        while not (root / 'route-started').exists():
            assert time.monotonic() < deadline, 'route discovery did not start'
            time.sleep(.005)
        rpc('native.launch.cancel', epoch=epoch, attemptId=attempt)
        (root / 'route-release').touch()
        deadline = time.monotonic() + 10
        while rpc('native.launch.owner')['busy']:
            assert time.monotonic() < deadline, 'worker did not finish'
            time.sleep(.01)
        status = rpc('native.launch.status', epoch=epoch, attemptId=attempt)
        assert status['error']['kind'] == 'steamRouteUnavailable', status
        expected = {'status': 'failed', 'reason': 'preparationFailed'}
        assert status['attempt']['outcome'] == expected, status
        assert status['phase'] == 'failed', status
        process.stdin.close()
        assert process.wait(timeout=20) == 0
        process = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        saved = command('native.launch.query', epoch=epoch, attemptId=attempt)
        assert saved['outcome'] == expected, saved
        assert not (root / 'steam.log').exists()
        print('PASS: genuine failure with Cancel remains failed after restart')
    finally:
        process.stdin.close()
        process.wait(timeout=20)
