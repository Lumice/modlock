#!/usr/bin/env python3
"""Round 7 actual-old-writer regressions from the independent review."""
import argparse,sys,tempfile,shutil
from pathlib import Path

def legacy_marker_membership(new, old):
    import sys,json,time,tempfile
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi,vpk
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')));c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
    a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None)
    with tempfile.TemporaryDirectory(prefix='dlmm-r7-member-') as td:
     f=FamilyUi(a,Path(td)/'legacy','legacy');extra=f.addons/'external_dir.vpk';extra.write_bytes(vpk());original=f.info.read_bytes()
     h=c.Helper(old,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
     job='c'*32;h.ok('native.local.prepare',jobId=job,profileId='default',expectedRevision='1',roots=[str(extra)])
     for _ in range(2000):
      status=h.ok('native.local.status',jobId=job)
      if status['phase']=='ready':break
      assert status['phase'] not in ('failed','interrupted'),status;time.sleep(.01)
     else:raise RuntimeError('timeout')
     s=h.ok('native.local.status',jobId=job,includeCandidates=True)['candidateSummary'];ids=[v['id'] for v in s['candidates'] if v.get('available',True) and not v.get('unavailableReason') and v.get('kind')!='companion'];review=h.ok('native.local.review',jobId=job,candidateIds=ids,metadata=s['defaultMetadata']);h.ok('native.local.commit',jobId=job,token=review['token'],profileId='default',expectedRevision='1',operationId='local')
     d=c.doc(f.data);p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1');h.ok('native.apply',token=p['token'],operationId='first',expectedRevision=d['revision'],deploymentLayout='shards-v1')
     f.info.write_bytes(original);d=c.doc(f.data);p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1')
     late=f.addons/'late_dir.vpk';late.write_bytes(vpk());res=h.call('native.apply',token=p['token'],operationId='second',expectedRevision=d['revision'],deploymentLayout='shards-v1');print('OLD_REAPPLY_WITH_LATE_ADDITION',json.dumps(res.get('error',{'ok':True})),flush=True)
     if 'error' in res:
      # Recent pre-rename helpers already reject this race. Verify the new
      # reader preserves that refusal and can apply a fresh reviewed setup.
      assert res['error']['data']['kind']=='stalePreview',res
      assert f.info.read_bytes()==original
      late.unlink();h.close();h=c.Helper(new,f.data)
      d=c.doc(f.data);p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1')
      h.ok('native.apply',token=p['token'],operationId='fresh-after-refusal',expectedRevision=d['revision'],deploymentLayout='shards-v1')
      assert h.ok('native.snapshot')['recoveryRequired'] is False
      assert not (f.info.parent/'modlock').exists()
      h.close();print('PASS pre-rename stale review refusal and fresh legacy Apply',flush=True);return
     assert 'result' in res,res
     d=c.doc(f.data);rows=d['previous']['upstreamPayloads']
     origins=d['previous'].get('installedGameOrigins',[])
     complete=any(o.get('treeComplete') and o.get('treeVersion')==1 and any(p.get('sourcePath')==str(late) for p in o['files']) for o in origins)
     assert not any(x.get('sourcePath')==str(late) for x in rows)
     assert complete or any(x.get('installedOrigin') for x in rows),d['previous']
     h.close();h=c.Helper(new,f.data)
     snap=h.ok('native.snapshot')
     if complete:
      # 78c6aef retains full tree membership separately from deduplicated
      # upstreamPayloads. Leaf markers do not describe this checkpoint.
      assert snap['canRestore'] is True,snap
      late.unlink()
      # Once a recorded file is gone, Restore must not be offered.
      after=h.ok('native.snapshot')
      assert after['canRestore'] is False,after
      expected=('nativeIo','externalChange')
     else:
      late.unlink()
      assert snap['canRestore'] is False,snap
      assert any("Apply your current setup" in str(w) for w in snap['warnings'])
      expected=('unverifiedRestore',)
     before=f.info.read_bytes();files={str(p):p.read_bytes() for p in f.addons.rglob('*') if p.is_file()}
     res=h.call('native.restore',operationId='restore',expectedRevision=d['revision'])
     assert res.get('error',{}).get('data',{}).get('kind') in expected,res
     assert f.info.read_bytes()==before
     assert {str(p):p.read_bytes() for p in f.addons.rglob('*') if p.is_file()}==files
     h.close();print('PASS historical checkpoint membership refusal',flush=True)

def conflicting_apply(new, old):
    import sys,json,sqlite3,time,os
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi,vpk
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')));c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
    a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None);root=Path(tempfile.mkdtemp(prefix='dlmm-r7-conflict-'))
    def imported(label):
     f=FamilyUi(a,root/label,label);h=c.Helper(old,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0');return f,h
    def local(h,f,source,job):
     d=c.doc(f.data);rev=d['revision'];h.ok('native.local.prepare',jobId=job,profileId='default',expectedRevision=rev,roots=[str(source)])
     for _ in range(2000):
      status=h.ok('native.local.status',jobId=job)
      if status['phase']=='ready':break
      assert status['phase'] not in ('failed','interrupted'),status;time.sleep(.01)
     else:raise Exception('timeout')
     s=h.ok('native.local.status',jobId=job,includeCandidates=True)['candidateSummary'];ids=[v['id'] for v in s['candidates'] if v.get('available',True) and not v.get('unavailableReason') and v.get('kind')!='companion'];review=h.ok('native.local.review',jobId=job,candidateIds=ids,metadata=s['defaultMetadata']);h.ok('native.local.commit',jobId=job,token=review['token'],profileId='default',expectedRevision=rev,operationId='local-'+job[:4])
    # Old apply, then two independent installed imports capture different mount histories.
    f,h=imported('migration-conflict');preview=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game));h.ok('native.apply',token=preview['token'],operationId='apply',expectedRevision='1')
    p=f.addons/'first_dir.vpk';p.write_bytes(vpk());local(h,f,p,'c'*32)
    p=f.addons/'second_dir.vpk';p.write_bytes(vpk());local(h,f,p,'d'*32)
    d=c.doc(f.data);rows=[m['source']['installedGameOrigins'] for m in d['profiles']['default']['mods'] if m['source'].get('installedGameOrigins')];assert len(rows)==2 and rows[0]!=rows[1]
    old_snapshot=h.call('native.snapshot');assert 'result' in old_snapshot,old_snapshot;h.close()
    h=c.Helper(new,f.data);snap=h.call('native.snapshot');edit=h.call('native.profile',action='copy',name='Rescue',profileId='default',expectedRevision=d['revision'],operationId='copy');print('CONFLICT_SNAPSHOT_AND_EDIT', 'result' in snap, 'result' in edit);assert 'result' in snap and 'result' in edit;result=h.call('native.applyPreview',profileId='default',gamePath=str(f.game),deploymentLayout='shards-v1');print('CONFLICT_APPLY_PREVIEW',json.dumps(result.get('error',{'ok':True})));assert 'result' in result,result;revision=c.doc(f.data)['revision'];h.ok('native.apply',token=result['result']['token'],operationId='rescue-apply',expectedRevision=revision,deploymentLayout='shards-v1');assert h.ok('native.snapshot')['canRestore'] is True;h.close();shutil.rmtree(root)

def nonmount_token(new, old):
    import sys,tempfile,json
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')));c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
    a=SimpleNamespace(helper=new,binary=None,capture_dir=None)
    with tempfile.TemporaryDirectory(prefix='dlmm-r7-token-') as td:
     for shard in (True,):
      f=FamilyUi(a,Path(td)/str(shard),'token');f.info.write_bytes(f.info.read_bytes().replace(b'"GameInfo"\r\n{',b'"GameInfo"\r\n{\r\n  "SomeMetadata" "citadel/addons/not-a-mount"'))
      h=c.Helper(new,f.data);p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
      layout={'deploymentLayout':'shards-v1'} if shard else {};p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game),**layout);res=h.call('native.apply',token=p['token'],operationId='apply',expectedRevision='1',**layout)
      print('NONMOUNT_TOKEN',shard,json.dumps(res.get('error',{'ok':True})),flush=True);assert 'result' in res,res;h.close()

if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('helper', type=Path)
    parser.add_argument('--old-helper', type=Path)
    args = parser.parse_args()
    if args.old_helper is not None and not args.old_helper.is_file():
        parser.error(f'Historical helper is missing: {args.old_helper}')
    cases = [legacy_marker_membership, conflicting_apply] if args.old_helper else [nonmount_token]
    for case in cases:
        case(args.helper.resolve(), args.old_helper.resolve() if args.old_helper else None)
