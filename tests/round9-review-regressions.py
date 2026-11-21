#!/usr/bin/env python3
"""Replay historical partial folder formats without requiring historical binaries."""
import argparse,sys,os,tempfile,importlib.util,json,sqlite3
from pathlib import Path
from types import SimpleNamespace
sys.dont_write_bytecode=True
isolation=tempfile.TemporaryDirectory(prefix='round9-environment-')
for key in ['HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS']:
 p=Path(isolation.name)/key;p.mkdir(mode=0o700);os.environ[key]=str(p)
repo=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(repo/'tests'))
from nativeintegrationcheck import FamilyUi,vpk
spec=importlib.util.spec_from_file_location('compat',repo/'tests/game-format-cross-version-check.py')
c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('helper', type=Path)
parser.add_argument('--old-helper', type=Path)
args = parser.parse_args()
if args.old_helper is not None and not args.old_helper.is_file():
    parser.error(f'Historical helper is missing: {args.old_helper}')
new = args.helper.resolve()
writer = args.old_helper.resolve() if args.old_helper else new
variants = ['flat-addon'] if args.old_helper else ['sharded-addon', 'sharded-repair']
a = SimpleNamespace(helper=new, binary=None, capture_dir=None)
failures=[]
with tempfile.TemporaryDirectory(prefix='round9-') as td:
 for variant,mode in [(v,m) for v in variants for m in ['partial-mount','unmarked-tree','per-file']]:
  f=FamilyUi(a,Path(td)/(variant+'-'+mode),variant)
  folder=f.addons
  if variant.endswith('repair'):
   folder=f.info.parent/'dmm_localization_default';folder.mkdir();(folder/'old.vpk').write_bytes(vpk())
   f.info.write_bytes(f.info.read_bytes().replace(b'   Game citadel/addons // old\r\n',b'   Game citadel/dmm_localization_default\r\n'))
  loose=folder/'materials/original.vmat_c';loose.parent.mkdir();loose.write_bytes(b'ORIGINAL MATERIAL')
  h=c.Helper(writer,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
  layout={'deploymentLayout':'shards-v1'} if variant.startswith('sharded') else {}
  p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),**layout);h.ok('native.apply',token=p['token'],operationId='apply',expectedRevision='1',**layout);h.close()
  d=c.doc(f.data)
  # Intermediate writer format: mount row, but only lowercase VPK children.
  rows=d['previous']['upstreamPayloads'] if variant.endswith('repair') else d['previous']['installedGameOrigins']
  for row in rows:
   if row.get('proofKind')=='mount' or row.get('mount'):
    row.pop('treeComplete',None);row.pop('treeVersion',None)
    if mode=='partial-mount':row['files']=[r for r in row['files'] if r['sourcePath'].endswith('.vpk')]
  if mode=='per-file':
   d['previous'].pop('installedGameOrigins',None)
   d['previous']['upstreamPayloads']=[r for r in d['previous']['upstreamPayloads'] if r.get('proofKind')!='mount']
  import shutil
  if mode=='partial-mount':
   shutil.rmtree(loose.parent)
   if folder==f.addons:
    (folder/'.dmm.json').unlink();(folder/'unknown.txt').unlink()
  with sqlite3.connect(f.data/'library.sqlite3') as db:db.execute('UPDATE library SET body=?',(json.dumps(d),))
  before=f.info.read_bytes();h=c.Helper(new,f.data);snap=h.ok('native.snapshot');r=h.call('native.restore',operationId='restore',expectedRevision=d['revision']);h.close()
  ok=snap['canRestore'] is False and r.get('error',{}).get('data',{}).get('kind')=='unverifiedRestore' and f.info.read_bytes()==before
  print('PASS' if ok else 'FAIL',variant,mode, 'canRestore=',snap['canRestore'],'result=',r.get('error',{'ok':True}),flush=True)
  if not ok:failures.append((variant,mode))
sys.exit(bool(failures))
