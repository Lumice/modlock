"""First Home Play on a fresh library reviews the detected disposable game."""
import json, os, pathlib, subprocess, sys, tempfile
from nativeintegrationcheck import BEFORE, vpk
with tempfile.TemporaryDirectory(prefix='first-play-') as temp:
    root = pathlib.Path(temp)
    env = {k:v for k,v in os.environ.items() if not k.startswith('XDG_')}
    for key in ['HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_DATA_DIRS','XDG_CONFIG_DIRS']:
        p=root/key; p.mkdir(mode=0o700); env[key]=str(p)
    game=root/'HOME/.local/share/Steam/steamapps/common/Deadlock'
    (game/'game/bin/win64').mkdir(parents=True)
    (game/'game/citadel').mkdir()
    (game/'game/citadel/pak01_dir.vpk').write_bytes(vpk())
    before=BEFORE.replace(b'citadel/addons',b'citadel')
    (game/'game/citadel/gameinfo.gi').write_bytes(before)
    fake=root/'bin'; fake.mkdir()
    steam=fake/'steam'; steam.write_text('#!/bin/sh\nexit 77\n'); steam.chmod(0o700)
    env['PATH']=str(fake)+os.pathsep+env['PATH']
    data=root/'data'
    request={'jsonrpc':'2.0','id':1,'method':'native.library.initialize','params':{'version':1,'dataDir':str(data),'expectedRevision':'0','operationId':'a'*32}}
    initialized=subprocess.run([sys.argv[2]],input=json.dumps(request)+'\n',text=True,capture_output=True,env=env,timeout=15)
    assert 'error' not in json.loads(initialized.stdout), initialized.stdout
    env.update(QT_QPA_PLATFORM='offscreen',QT_QUICK_BACKEND='software')
    run=subprocess.run([sys.argv[1],'--helper',sys.argv[2],'--data-dir',str(data),'--check-ui','native-first-play'],env=env,text=True,capture_output=True,timeout=30)
    assert run.returncode == 0 and 'PASS UI' in run.stdout, run.stdout+run.stderr
    assert (game/'game/citadel/gameinfo.gi').read_bytes() == before
    print(run.stdout.strip())
