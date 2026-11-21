#!/usr/bin/env python3
"""Bound launch polling on a disposable real-helper fixture and fake Steam."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from nativeintegrationcheck import BEFORE, vpk

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
    source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': [mod], 'profiles': {'default': {'name': 'Default', 'mods': [mod]}}}}}))
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
    process.stdin.close()
    assert process.wait(timeout=20) == 0
    fake = root / 'bin'; fake.mkdir()
    steam = fake / 'steam'
    steam.write_text('#!/usr/bin/python3\nimport json,os,sys,time\nwith open(os.environ["STEAM_LOG"],"a") as f: f.write(json.dumps({"t":time.monotonic(),"args":sys.argv[1:]})+"\\n")\n')
    steam.chmod(0o700)
    wrapper = root / 'delayed-helper'
    wrapper.write_text('''#!/usr/bin/python3
import json,os,subprocess,sys,time,threading
p=subprocess.Popen([os.environ['REAL_HELPER']],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
def output():
    for line in p.stdout:
        try:
            result=json.loads(line).get('result',{})
            if result.get('phase') == 'ready': time.sleep(.5)
        except (ValueError,AttributeError): pass
        sys.stdout.write(line);sys.stdout.flush()
t=threading.Thread(target=output,daemon=True);t.start()
for line in sys.stdin:
    p.stdin.write(line);p.stdin.flush()
p.stdin.close();p.wait();t.join(timeout=2)
''')
    wrapper.chmod(0o700)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', UI_NATIVE_GAME=str(game), UI_NATIVE_SOURCE=str(source),
               PATH=str(fake)+os.pathsep+env['PATH'], STEAM_LOG=str(root/'steam.log'), REAL_HELPER=helper)
    for scenario in ('native-play-cancel', 'native-play-cancel-unsent'):
        result=subprocess.run([binary,'--helper',str(wrapper),'--data-dir',str(data),'--check-ui',scenario],env=env,capture_output=True,text=True,timeout=45)
        assert result.returncode == 0 and 'PASS UI' in result.stdout, result.stdout+result.stderr
        assert not (root/'steam.log').exists(), 'Steam was requested after cancellation'
        print(result.stdout.strip())
