#!/usr/bin/env python3
import argparse,os,sys,tempfile,subprocess
from pathlib import Path
repo=Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('helper', type=Path)
parser.add_argument('--old-helper', type=Path)
parser.add_argument('--v1-helper', type=Path)
args = parser.parse_args()
for helper in (args.old_helper, args.v1_helper):
 if helper is not None and not helper.is_file():
  parser.error(f'Historical helper is missing: {helper}')
new = args.helper.resolve()
old = args.old_helper.resolve() if args.old_helper else None
v1 = args.v1_helper.resolve() if args.v1_helper else None
isolation=tempfile.TemporaryDirectory(prefix="dlmm-round8-env-")
for key,leaf in [("HOME","home"),("XDG_DATA_HOME","data"),("XDG_CONFIG_HOME","config"),("XDG_CACHE_HOME","cache"),("XDG_STATE_HOME","state"),("XDG_RUNTIME_DIR","runtime"),("XDG_CONFIG_DIRS","config-dirs"),("XDG_DATA_DIRS","data-dirs")]:
 p=Path(isolation.name)/leaf;p.mkdir(mode=0o700);os.environ[key]=str(p)

preload=Path(isolation.name)/'fail-unlink.so'
if old:
 subprocess.run(['cc','-shared','-fPIC','-o',str(preload),str(repo/'tests/fixtures/maintenance-fail-unlink.c'),'-ldl'],check=True)

def legacy_maintenance_real_probe():
 import sys,tempfile,importlib.util,json,sqlite3,os
 from pathlib import Path
 from types import SimpleNamespace
 repo=Path(__file__).resolve().parents[1];sys.path.insert(0,str(repo/'tests'))
 from nativeintegrationcheck import FamilyUi
 spec=importlib.util.spec_from_file_location('compat',repo/'tests/game-format-cross-version-check.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 a=SimpleNamespace(helper=new,binary=None,capture_dir=None)
 with tempfile.TemporaryDirectory(prefix='legacy-gc-real-') as td:
  f=FamilyUi(a,Path(td)/'fixture','legacy-gc');h=c.Helper(old,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0');h.close()
  foreign=f.data/'payloads'/'abcdefabcdefabcdefabcdefabcdefab.vpk';foreign.write_bytes(b'A USER FILE NOT CREATED BY THE APP')
  h=c.Helper(new,f.data);newscan=h.ok('native.maintenance.scan');h.close();assert any(r.get('path')==str(foreign) and r.get('kind')=='unproven' for r in newscan['rows'])
  os.environ['LD_PRELOAD']=str(preload);h=c.Helper(old,f.data);del os.environ['LD_PRELOAD']
  scan=h.ok('native.maintenance.scan');rows=[r for r in scan['rows'] if r.get('path')==str(foreign)];assert len(rows)==1 and rows[0]['kind']=='payload' and rows[0]['removable']
  interrupted=h.call('native.maintenance.clean',operationId='old-clean',expectedRevision=scan['revision'],token=scan['token']);h.close();assert 'error' in interrupted and foreign.exists(),interrupted
  with sqlite3.connect(f.data/'library.sqlite3') as db:
   journal=json.loads(db.execute("SELECT body FROM journals WHERE id='old-clean'").fetchone()[0]);receipt=db.execute("SELECT name FROM sqlite_master WHERE name='payload_inode_receipts'").fetchone()
  assert journal['payloads'][0]['path']==str(foreign) and receipt is None
  h=c.Helper(new,f.data);pre=h.ok('native.maintenance.scan');result=h.call('native.maintenance.clean',operationId='resume',expectedRevision=scan['revision'],token='unused');h.close()
  print('LEGACY_MAINTENANCE_ACTUAL_OLD_WRITER',json.dumps({'oldCleanError':interrupted['error']['data']['kind'],'journalVersion':journal['version'],'hadCreatorReceipt':bool(receipt),'freshNewScanUnproven':True,'pendingNewScanRemovable':any(r.get('path')==str(foreign) and r.get('removable') for r in pre['rows']),'resumeResult':result.get('error',{'ok':True}),'foreignFileExistsAfter':foreign.exists()}),flush=True)
  assert foreign.exists();assert any(r.get('path')==str(foreign) and r['kind']=='unproven' and not r['removable'] for r in pre['rows']);assert 'result' in result
  with sqlite3.connect(f.data/'library.sqlite3') as db: assert db.execute('SELECT count(*) FROM journals').fetchone()[0]==0

def legacy_repair_probe():
 import sys, tempfile, importlib.util, json
 from pathlib import Path
 from types import SimpleNamespace
 repo=Path(__file__).resolve().parents[1]
 sys.path.insert(0,str(repo/'tests'))
 from nativeintegrationcheck import FamilyUi, vpk
 spec=importlib.util.spec_from_file_location('compat',repo/'tests/game-format-cross-version-check.py'); c=importlib.util.module_from_spec(spec); spec.loader.exec_module(c)
 
 a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None)
 with tempfile.TemporaryDirectory(prefix='legacy-repair-') as td:
  for shard in (False,True):
   f=FamilyUi(a,Path(td)/str(shard),'legacy-repair'); repair=f.info.parent/'dmm_localization_default';repair.mkdir();rf=repair/'old.vpk';rf.write_bytes(vpk())
   original=f.info.read_bytes().replace(b'   Game citadel/addons // old\r\n',b'   Game citadel/dmm_localization_default\r\n');f.info.write_bytes(original)
   h=c.Helper(old,f.data); p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
   layout={'deploymentLayout':'shards-v1'} if shard else {};p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),**layout);h.ok('native.apply',token=p['token'],operationId='old-apply',expectedRevision='1',**layout);h.close()
   d=c.doc(f.data); print('AFTER_OLD_APPLY_REPAIR_MOUNT',shard,b'Game citadel/dmm_localization_default' in f.info.read_bytes(),flush=True);print('CHECKPOINT',shard,'has_origins',bool(d['previous'].get('installedGameOrigins')),'repair_proof',d['previous']['upstreamPayloads'],flush=True)
   # An intervening edit occurs after old Apply and before new Restore begins.
   rf.write_bytes(vpk()+b'CHANGED_AFTER_CHECKPOINT'); added=repair/'new.vpk';added.write_bytes(vpk())
   h=c.Helper(new,f.data);snap=h.ok('native.snapshot');review=h.ok('native.applyPreview',restoreReview=True);res=h.call('native.restore',operationId='restore',expectedRevision=d['revision']);h.close()
   doc=c.doc(f.data)
   print('LEGACY_REPAIR',json.dumps({'sharded':shard,'canRestore':snap['canRestore'],'result':res.get('error',{'ok':True}),'oldGameinfoRestored':f.info.read_bytes()==original,'changedRepairPresent':rf.read_bytes().endswith(b'CHANGED_AFTER_CHECKPOINT'),'addedRepairPresent':added.exists(),'durableRestoredProofFiles':len((doc.get('restoredSetup') or {}).get('upstreamPayloads',[]))}),flush=True)
   assert snap['canRestore'] is False;assert res.get('error',{}).get('data',{}).get('kind')=='unverifiedRestore';assert f.info.read_bytes()!=original

def loose_file_probe():
 import sys,tempfile,importlib.util,json
 from pathlib import Path
 from types import SimpleNamespace
 repo=Path(__file__).resolve().parents[1];sys.path.insert(0,str(repo/'tests'))
 from nativeintegrationcheck import FamilyUi
 spec=importlib.util.spec_from_file_location('compat',repo/'tests/game-format-cross-version-check.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 a=SimpleNamespace(helper=new,binary=None,capture_dir=None)
 with tempfile.TemporaryDirectory(prefix='loose-') as td:
  for shard in (True,):
   f=FamilyUi(a,Path(td)/str(shard),'loose');initial=f.addons/'materials/original.vmat_c';initial.parent.mkdir();initial.write_bytes(b'EXISTING LOOSE MATERIAL');upper=f.addons/'UPPER.VPK';upper.write_bytes(b'UPPERCASE');before=f.info.read_bytes();h=c.Helper(new,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0');layout={'deploymentLayout':'shards-v1'} if shard else {};p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),**layout);h.ok('native.apply',token=p['token'],operationId='apply',expectedRevision='1',**layout)
   d=c.doc(f.data);origins=d['previous']['installedGameOrigins'];h.close();assert all(any(y['sourcePath']==str(path) for x in origins for y in x['files']) for path in (initial,initial.parent,upper))
   # Edit the no-longer-mounted directory after Apply and before Restore begins.
   loose=f.addons/'materials/models/later.vmat_c';loose.parent.mkdir(parents=True);loose.write_bytes(b'NEW UNREVIEWED LOOSE MATERIAL')
   h=c.Helper(new,f.data);snap=h.ok('native.snapshot');res=h.call('native.restore',operationId='restore',expectedRevision=d['revision']);h.close()
   print('LOOSE_FILE',json.dumps({'sharded':shard,'origin_records':sum(len(x['files']) for x in origins),'looseWasRecorded':any(any(y['sourcePath']==str(loose) for y in x['files']) for x in origins),'canRestore':snap['canRestore'],'result':res.get('error',{'ok':True}),'oldMountRestored':f.info.read_bytes()==before,'loosePresent':loose.exists()}),flush=True)
   assert snap['canRestore'] is False;assert 'error' in res;assert f.info.read_bytes()!=before;assert loose.exists()

def legacy_flat_retire_probe():
 import sys,tempfile,importlib.util,json,os
 from pathlib import Path
 from types import SimpleNamespace
 repo=Path(__file__).resolve().parents[1];sys.path.insert(0,str(repo/'tests'))
 from nativeintegrationcheck import FamilyUi,vpk
 spec=importlib.util.spec_from_file_location('compat',repo/'tests/game-format-cross-version-check.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
 old=v1;a=SimpleNamespace(helper=new,binary=None,capture_dir=None)
 with tempfile.TemporaryDirectory(prefix='flat-retire-') as td:
  f=FamilyUi(a,Path(td)/'fixture','flat-retire')
  for p in f.addons.glob('*.vpk'):p.unlink()
  (f.addons/'pak01_dir.vpk').write_bytes(vpk())
  (f.addons/'.dmm.json').write_text(json.dumps({'version':3,'mods':{'remote-a':{'enabled':True,'order':0,'shard':1,'currentVpks':['pak01_dir.vpk'],'disabledVpks':[],'originalVpkNames':['a_dir.vpk']},'remote-b':{'enabled':False,'order':1,'shard':1,'currentVpks':[],'disabledVpks':[],'originalVpkNames':[]}}}))
  h=c.Helper(old,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
  for i in range(2):
   d=c.doc(f.data);p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game));res=h.call('native.apply',token=p['token'],operationId=f'old-apply-{i}',expectedRevision=d['revision']);assert 'result' in res,res
  h.close();d=c.doc(f.data);a=d['previous']['applied'];assert a['manifest']['version']==1 and 'directoryIdentity' not in a and 'manifest.json' not in a['identities']
  generation=f.info.parent/'modlock'/a['generation'];manifest=generation/'manifest.json';original=manifest.read_bytes();origino=manifest.stat().st_ino
  replacement=generation/'replacement';replacement.write_bytes(original);os.replace(replacement,manifest);newino=manifest.stat().st_ino;assert newino!=origino
  h=c.Helper(new,f.data);p=h.call('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1');print('NEW_PREVIEW',p.get('error',{'ok':True}),flush=True)
  if 'result' in p:
   res=h.call('native.apply',token=p['result']['token'],operationId='new-apply',expectedRevision=d['revision'],deploymentLayout='shards-v1');print('NEW_APPLY',res.get('error',{'ok':True}),flush=True)
  h.close();print('LEGACY_FLAT_RETIRE',json.dumps({'oldManifestVersion':a['manifest']['version'],'oldProofHasDirectoryIdentity':False,'oldProofHasManifestIdentity':False,'replacedManifestInode':newino,'replacementStillExists':manifest.exists(),'generationStillExists':generation.exists()}),flush=True)
  assert manifest.exists();assert generation.exists();h=c.Helper(new,f.data);scan=h.ok('native.maintenance.scan');assert any(r['kind']=='unproven' and not r['removable'] and r['path']==str(generation) for r in scan['rows']);h.close()

cases = []
if old:
 cases.extend([legacy_maintenance_real_probe, legacy_repair_probe])
if v1:
 cases.append(legacy_flat_retire_probe)
if not cases:
 cases.append(loose_file_probe)

failures = 0
for case in cases:
 try: case();print("PASS",case.__name__,flush=True)
 except Exception as e: failures+=1;print("FAIL",case.__name__,repr(e),flush=True)

sys.exit(bool(failures))
