#!/usr/bin/env python3
"""Single removal/deletion, cancellation and stale confirmation on copied fixtures."""
import json
import os
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile

from nativeintegrationcheck import BEFORE, vpk

for scenario in ('native-mod-remove', 'native-mod-delete', 'native-mod-delete-cancel', 'native-mod-delete-stale', 'native-mod-delete-auto', 'native-mod-delete-bulk', 'native-mod-delete-running', 'native-mod-remove-inactive', 'native-mod-delete-inactive'):
    if len(sys.argv) > 3 and scenario not in sys.argv[3:]:
        continue
    binary, helper = map(lambda p: str(Path(p).resolve()), sys.argv[1:3])
    with tempfile.TemporaryDirectory(prefix='native-play-poll-') as temp:
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
        source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': [mod], 'profiles': {'default': {'name': 'Default', 'mods': [mod]}, 'other': {'name': 'Other', 'mods': [dict(mod, enabled=False)] if scenario.endswith('-inactive') else [mod]}}}}}))
        (game / 'game/citadel/gameinfo.gi').write_bytes(BEFORE)
        env = os.environ.copy()
        for key in list(env):
            if key.startswith('XDG_'): del env[key]
        for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'):
            directory = root / key
            directory.mkdir(mode=0o700)
            env[key] = str(directory)
        process = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        def rpc(method, **params):
            process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {'dataDir': str(data), **params}}) + '\n')
            process.stdin.flush()
            result = json.loads(process.stdout.readline())
            assert 'result' in result, result
            return result['result']
        review = rpc('native.importPreview', statePath=str(source), gamePath=str(game))
        rpc('native.importCommit', token=review['token'], operationId='import', expectedRevision='0')
        review = rpc('native.applyPreview', profileId='default', gamePath=str(game), deploymentLayout='shards-v1')
        rpc('native.apply', token=review['token'], operationId='initial', expectedRevision='1', deploymentLayout='shards-v1')
        import sqlite3
        with sqlite3.connect(data / 'library.sqlite3') as db:
            before_doc = json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
        before_gameinfo = (game / 'game/citadel/gameinfo.gi').read_bytes()
        process.stdin.close()
        assert process.wait(timeout=20) == 0
        if scenario == 'native-mod-delete-bulk':
            import sqlite3
            with sqlite3.connect(data / 'library.sqlite3') as db:
                doc = json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
                second = json.loads(json.dumps(doc['profiles']['default']['mods'][0]))
                second['source']['id'] = 'local-b'
                doc['profiles']['default']['mods'].append(second)
                db.execute('UPDATE library SET body=? WHERE id=1', (json.dumps(doc, separators=(',', ':')),))
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', UI_NATIVE_GAME=str(game), UI_NATIVE_SOURCE=str(source))
        captures = os.environ.get('UI_REMOVAL_CAPTURES')
        if captures:
            folder = Path(captures) / scenario
            folder.mkdir(parents=True, exist_ok=True)
            env['UI_NATIVE_CAPTURES'] = str(folder)
        fake_game = None
        if scenario == 'native-mod-delete-running':
            executable = root / 'deadlock.exe'
            shutil.copyfile('/bin/sleep', executable); executable.chmod(0o700)
            fake_game = subprocess.Popen([str(executable), '60'], env=env)
        try:
            result = subprocess.run([binary, '--helper', helper, '--data-dir', str(data), '--check-ui', scenario], env=env, capture_output=True, text=True, timeout=45)
            assert result.returncode == 0 and 'PASS UI' in result.stdout, result.stdout + result.stderr
            if fake_game:
                request = {'jsonrpc':'2.0','id':1,'method':'native.deleteMods','params':{'dataDir':str(data),'profileId':'default','modIds':['local-a'],'expectedRevision':'2','operationId':'running-delete'}}
                response = subprocess.run([helper], input=json.dumps(request)+'\n', env=env, capture_output=True, text=True, timeout=15)
                assert json.loads(response.stdout)['error']['data']['kind'] == 'gameRunning', response.stdout
        finally:
            if fake_game:
                fake_game.terminate(); fake_game.wait(timeout=5)
        import sqlite3
        with sqlite3.connect(data / 'library.sqlite3') as db:
            doc = json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
        changed = scenario in ('native-mod-remove', 'native-mod-delete', 'native-mod-delete-auto', 'native-mod-delete-bulk', 'native-mod-remove-inactive', 'native-mod-delete-inactive')
        if scenario.endswith('-inactive'):
            assert doc['revision'] == '3' and doc['activeProfileId'] == 'default'
            assert doc['applied'] == before_doc['applied'] and doc['previous'] == before_doc['previous']
            assert (game / 'game/citadel/gameinfo.gi').read_bytes() == before_gameinfo
            assert len(doc['profiles']['default']['mods']) == (1 if scenario.startswith('native-mod-remove') else 0)
            assert len(doc['profiles']['other']['mods']) == 0
        else:
            assert doc['revision'] == ('4' if scenario == 'native-mod-delete-auto' else '3' if changed else '2'), doc['revision']
            assert len(doc['profiles']['default']['mods']) == (0 if changed else 1)
            assert len(doc['profiles']['other']['mods']) == (0 if changed and scenario != 'native-mod-remove' else 1)
            assert doc['applied'] is not None, 'removal discarded the game checkpoint'
            assert list((data / 'payloads').glob('*.vpk')), 'restore-point payload was deleted'
        print(scenario + ': ' + result.stdout.strip(), flush=True)
