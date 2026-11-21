#!/usr/bin/env python3
"""Fixed historical workspace/state and explicit helper discovery contracts."""
import argparse,hashlib,importlib.util,json,os,subprocess,tempfile
from pathlib import Path
from types import SimpleNamespace
from nativeintegrationcheck import FamilyUi
spec=importlib.util.spec_from_file_location('compat',Path(__file__).with_name('game-format-cross-version-check.py'))
compat=importlib.util.module_from_spec(spec);spec.loader.exec_module(compat)
cases = ['fresh-workspace', 'never-applied-old-workspace', 'custom-pending-state', 'selected-helper']
p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--helper', type=Path, required=True)
p.add_argument('--old-helper', type=Path)
p.add_argument('--binary', type=Path, required=True)
p.add_argument('--case', choices=cases)
args = p.parse_args()
if args.old_helper is not None and not args.old_helper.is_file():
 p.error(f'Expected pre-rename helper is missing: {args.old_helper}')
if args.case == 'never-applied-old-workspace' and args.old_helper is None:
 p.error('--case never-applied-old-workspace requires --old-helper')
failed=[]
with tempfile.TemporaryDirectory(prefix='rename-policy-') as temp:
 for case in cases:
  if args.case and args.case != case: continue
  if case == 'never-applied-old-workspace' and args.old_helper is None: continue
  try:
   f=FamilyUi(SimpleNamespace(helper=args.helper,binary=None,capture_dir=None),Path(temp)/case,case)
   seed=compat.Helper(args.old_helper if case=='never-applied-old-workspace' else args.helper,f.data)
   preview=seed.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game))
   seed.ok('native.importCommit',token=preview['token'],operationId='import',expectedRevision='0');seed.close()
   h=compat.Helper(args.helper,f.data)
   if case.endswith('workspace'):
    preview=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1')
    h.ok('native.apply',token=preview['token'],operationId='apply',expectedRevision='1',deploymentLayout='shards-v1')
    d=compat.doc(f.data);generation=f.game/'game/citadel/deadlock_native'/d['applied']['generation']
    assert generation.is_dir(),f'wrong game workspace: {d["applied"]["manifest"]}'
    assert 'workspace' not in d['applied']['manifest'] and 'gameWorkspace' not in d
    assert not (f.game/'game/citadel/modlock').exists()
   elif case=='custom-pending-state':
    h.ok('native.profile',action='create',name='Recovered',operationId='custom-pending',expectedRevision='1')
    state=Path(f.env['XDG_CONFIG_HOME'])/'dl-mod-manager/state';pending=state/'pending';pending.mkdir(parents=True,mode=0o700)
    marker=pending/(hashlib.sha256(str(f.data).encode()).hexdigest()+'.json');marker.write_text(json.dumps({'operationId':'custom-pending','method':4}));marker.chmod(0o600)
    (Path(f.env['XDG_CONFIG_HOME'])/'modlock/state/pending').mkdir(parents=True,mode=0o700)
    result=subprocess.run([str(args.binary),'--helper',str(args.helper),'--data-dir',str(f.data),'--check-ui','legacy-state-reconcile'],env=f.env|{'EXPECT_STATE_ROOT':str(state),'EXPECT_PENDING_PATH':str(marker)},capture_output=True,text=True,timeout=30)
    assert result.returncode==0,result.stdout+result.stderr
    assert json.loads(marker.read_text())=={}
   else:
    wrapper=f.root/'chosen-helper';log=f.root/'discovery.json'
    wrapper.write_text('#!/usr/bin/python3\nimport json,os,sys\nfrom pathlib import Path\nif sys.argv[1:]==["--data-root"]:\n Path('+repr(str(log))+').write_text("queried")\n print(json.dumps({"root":'+repr(str(f.data))+'}))\nelse:os.execv('+repr(str(args.helper))+',['+repr(str(args.helper))+']+sys.argv[1:])\n');wrapper.chmod(0o700)
    result=subprocess.run([str(args.binary),'--helper',str(wrapper),'--check-ui','runtime-paths'],env=f.env|{'EXPECT_STATE_ROOT':str(Path(f.env['XDG_CONFIG_HOME'])/'dl-mod-manager/state'),'EXPECT_LIBRARY_ROOT':str(f.data)},capture_output=True,text=True,timeout=25)
    assert result.returncode==0 and log.exists(),result.stdout+result.stderr+' discovery log='+str(log.exists())
   h.close();print('PASS',case,flush=True)
  except Exception as e:
   if 'h' in locals() and h.p.poll() is None:h.close()
   failed.append(case);print('FAIL',case,str(e),flush=True)
assert not failed,failed
