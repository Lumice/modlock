#!/usr/bin/env python3
"""App and helper share library-file selection, including legacy bulk storage."""
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

helper=Path(sys.argv[1]).resolve()
app=Path(sys.argv[2]).resolve() if len(sys.argv)>2 else None
with tempfile.TemporaryDirectory(prefix='modlock-paths-') as tmp:
    root=Path(tmp);env=os.environ.copy()
    for key in set(k for k in env if k.startswith('XDG_')) | {'HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','XDG_STATE_HOME','XDG_DATA_DIRS','XDG_CONFIG_DIRS'}:
        folder=root/key;folder.mkdir(mode=0o700);env[key]=str(folder)
    env.update(QT_QPA_PLATFORM='offscreen',QT_QUICK_BACKEND='software')
    base=Path(env['XDG_DATA_HOME']);modern=base/'modlock';legacy=base/'deadlock-native'
    def rpc(method,**params):
        p=subprocess.run([str(helper)],input=json.dumps(dict(jsonrpc='2.0',id=1,method=method,params=params))+'\n',env=env,text=True,capture_output=True,timeout=30,check=True)
        return json.loads(p.stdout)
    def call(method,**params):
        v=rpc(method,**params);assert 'error' not in v,v;return v['result']
    def selected(expected):
        p=subprocess.run([str(helper),'--data-root'],env=env,capture_output=True,text=True,check=True)
        assert json.loads(p.stdout)['root']==str(expected)
        if app:
            state='dl-mod-manager'
            run=subprocess.run([str(app),'--demo','--check-ui','runtime-paths'],env=env|{'EXPECT_STATE_ROOT':str(Path(env['XDG_CONFIG_HOME'])/state/'state'),'EXPECT_LIBRARY_ROOT':str(expected)},capture_output=True,text=True,timeout=25)
            assert run.returncode==0,run.stdout+run.stderr
    def initialize(path,title):
        call('native.library.initialize',version=1,dataDir=str(path),operationId='a'*32,expectedRevision='0')
        with sqlite3.connect(path/'library.sqlite3') as db:
            doc=json.loads(db.execute('select body from library').fetchone()[0]);doc['profiles']['default']['name']=title
            db.execute('update library set body=?',(json.dumps(doc),))
    selected(modern)  # Neither exists; no directory or database is created by selection.
    assert not modern.exists() and not legacy.exists()
    initialize(modern,'New');selected(modern)
    assert call('native.snapshot')['profiles'][0]['name']=='New'
    modern.rename(legacy);selected(legacy)
    modern.mkdir()  # Empty new root must never hide an existing legacy library.
    selected(legacy)
    assert call('native.snapshot')['profiles'][0]['name']=='New'
    bulk=root/'bulk storage';bulk.mkdir()
    review=call('native.storage.review',destination=str(bulk))
    call('native.storage.move',destination=str(bulk),token=review['token'],operationId='move-root-test')
    selected(legacy)
    status=call('native.storage.status')
    storage=Path(status['current']['path'])
    assert storage.is_relative_to(bulk) and Path(status['database']['path']) == legacy,status
    def check_storage_display(state):
        if app:
            run=subprocess.run([str(app),'--helper',str(helper),'--data-dir',str(legacy),
                                '--check-ui','settings-storage-'+state],env=env,capture_output=True,text=True,timeout=20)
            assert run.returncode==0 and 'PASS UI storage status' in run.stdout,run.stdout+run.stderr
    check_storage_display('available')
    detached=root/'disconnected'
    bulk.rename(detached)
    offline=call('native.storage.status')
    assert offline['current'] == {'path':str(storage),'unavailable':True},offline
    assert offline['database']['path'] == str(legacy),offline
    check_storage_display('offline')
    detached.rename(bulk)
    before=(legacy/'library.sqlite3').read_bytes()
    initialize(modern,'Second')
    v=rpc('native.snapshot');assert v['error']['data']['kind']=='ambiguousLibrary',v
    for folder in (modern,legacy):assert str(folder) in v['error']['message']
    if app:
        p=subprocess.run([str(app),'--demo','--check-ui','runtime-paths'],env=env,text=True,capture_output=True,timeout=25)
        assert p.returncode==1 and all(str(folder) in p.stderr for folder in (modern,legacy)),p.stdout+p.stderr
        assert 'Pass --data-dir <folder> to choose the library explicitly.' in p.stderr,p.stderr
    assert (legacy/'library.sqlite3').read_bytes()==before
    assert call('native.snapshot',dataDir=str(legacy))['profiles'][0]['name']=='New'
    print('PASS neither, new only, legacy with absent/empty new root, legacy bulk storage, both refuse')
    # Leave only legacy and seed a completed reference in its original state dir.
    (modern/'library.sqlite3').unlink()
    s=call('native.snapshot');call('native.profile',action='create',name='Recovered',operationId='pending-legacy',expectedRevision=s['revision'])
    config=Path(env['XDG_CONFIG_HOME']);old_state=config/'dl-mod-manager/state/pending';old_state.mkdir(parents=True,mode=0o700)
    (config/'modlock/state/pending').mkdir(parents=True,exist_ok=True,mode=0o700)
    marker=old_state/(hashlib.sha256(str(legacy).encode()).hexdigest()+'.json');marker.write_text(json.dumps({'operationId':'pending-legacy','method':4}))
    marker.chmod(0o600)
    if app:
        p=subprocess.run([str(app),'--check-ui','legacy-state-reconcile'],env=env|{'EXPECT_LIBRARY_ROOT':str(legacy),'EXPECT_STATE_ROOT':str(old_state.parent),'EXPECT_PENDING_PATH':str(marker)},capture_output=True,text=True,timeout=30)
        assert p.returncode==0,p.stdout+p.stderr
        assert json.loads(marker.read_text())=={}
        print('PASS empty modern state does not hide pending legacy reference; startup reconciles it')
