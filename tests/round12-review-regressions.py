#!/usr/bin/env python3
"""Fixture-only repair casing and actual historical-writer Qt loading checks."""
import argparse, importlib.util, os, subprocess, sys, tempfile
from pathlib import Path
from types import SimpleNamespace
repo = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(repo / 'tests'))
from nativeintegrationcheck import FamilyUi
spec = importlib.util.spec_from_file_location('compat', repo / 'tests/game-format-cross-version-check.py')
c = importlib.util.module_from_spec(spec); spec.loader.exec_module(c)
p = argparse.ArgumentParser(); p.add_argument('--helper', type=Path, required=True); p.add_argument('--binary', type=Path, required=True)
p.add_argument('--old-helper', type=Path)
p.add_argument('--case', choices=['repair','banner','warnings','all'], default='all'); a=p.parse_args()
if a.old_helper is not None and not a.old_helper.is_file():
    p.error(f'Historical helper is missing: {a.old_helper}')
if a.case == 'warnings' and a.old_helper is None:
    p.error('--case warnings requires --old-helper')
def ui(f, scenario):
    result=subprocess.run([str(a.binary), '--helper',str(a.helper),'--data-dir',str(f.data),'--check-ui',scenario],env=f.env,capture_output=True,text=True,timeout=45)
    assert result.returncode==0 and 'PASS UI' in result.stdout+result.stderr, result.stdout+result.stderr

def fixture(root): return FamilyUi(SimpleNamespace(helper=a.helper,binary=a.binary,capture_dir=None),root,'round12')
def imported(f, helper):
    h=c.Helper(helper,f.data); p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0');return h
def apply(f,h,shard):
    layout={'deploymentLayout':'shards-v1'} if shard else {}
    reply=h.call('native.applyPreview',profileId='default',gamePath=str(f.game),**layout)
    if 'error' in reply: return reply
    p=reply['result']
    return h.call('native.apply',token=p['token'],operationId='apply',expectedRevision='1',**layout)
def repair(root):
    for shard in (True,):
      for token in (False,True):
       for mode in ('changed','historical','ambiguous'):
        f=fixture(root/f'{shard}-{token}-{mode}'); d=f.info.parent/'DMM_Localization_Default';d.mkdir(); loose=d/'loose.dat';loose.write_bytes(b'ORIGINAL')
        spelling=b'|gameinfo_path|DMM_Localization_Default' if token else b'citadel/DMM_Localization_Default'
        before=f.info.read_bytes().replace(b'   Game citadel\r\n',b'   Game '+spelling+b'\r\n   Game citadel\r\n');f.info.write_bytes(before)
        h=imported(f,a.helper)
        if mode=='ambiguous': (f.info.parent/'dmm_localization_default').mkdir()
        res=apply(f,h,shard)
        if mode=='ambiguous':
            h.close();assert 'error' in res and f.info.read_bytes()==before, res;continue
        assert 'result' in res,(shard,token,mode,res)
        doc=c.doc(f.data)
        proof=next(x for x in doc['previous']['upstreamPayloads'] if x.get('proofKind')=='mount' and 'localization' in x['sourcePath'].lower())
        assert proof['sourcePath']==str(d) and proof['files'], proof
        if mode=='historical':
            import sqlite3,json
            proof['sourcePath']=str(f.info.parent/'dmm_localization_default');proof['files']=None
            with sqlite3.connect(f.data/'library.sqlite3') as db: db.execute('UPDATE library SET body=? WHERE id=1',(json.dumps(doc),))
        else: loose.write_bytes(b'USER CHANGE AFTER APPLY')
        snap=h.ok('native.snapshot');assert snap['canRestore'] is False,snap
        res=h.call('native.restore',operationId='restore',expectedRevision=doc['revision']);h.close();assert 'error' in res,res
        assert f.info.read_bytes()!=before and loose.exists()
    print('PASS repair ordinary/token current shards: literal proof, edits, historical mismatch, ambiguity',flush=True)
def warnings(root):
    f=fixture(root/'old');h=imported(f,a.old_helper);assert 'result' in apply(f,h,False);h.close()
    h=c.Helper(a.helper,f.data);snap=h.ok('native.snapshot');assert snap['canRestore'] is False
    refused=h.call('native.restore',operationId='refused-restore',expectedRevision=snap['revision']);h.close()
    assert refused.get('error',{}).get('data',{}).get('kind')=='unverifiedRestore',refused
    ui(f,'native-refused-restore')
    assert all(isinstance(w,dict) and isinstance(w.get('code'),str) and isinstance(w.get('message'),str) for w in snap['warnings']),snap['warnings']
    print('PASS actual 2778c01 library loads in Qt model with refused Restore',flush=True)
with tempfile.TemporaryDirectory(prefix='dlmm-round12-') as td:
    root=Path(td)
    for key in ['HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS']:
        path=root/key;path.mkdir(mode=0o700);os.environ[key]=str(path)
    if a.case in ('repair','all'): repair(root)
    if a.old_helper and a.case in ('warnings','all'): warnings(root)
    if a.case in ('banner','all'):
        f=fixture(root/'banner');h=imported(f,a.helper);h.close();ui(f,'storage-banner');print('PASS interrupted banner runtime QML',flush=True)
