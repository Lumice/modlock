#!/usr/bin/env python3
"""Round-6 reviewer fixtures: old payload replacement, conflicting and late baselines."""
import sys, tempfile, shutil
from pathlib import Path

def repro():
    import tempfile
    import sys, tempfile, json, hashlib, sqlite3, time, os
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi, vpk
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')))
    compat=importlib.util.module_from_spec(spec);spec.loader.exec_module(compat)
    old=Path(sys.argv[2])
    new=Path(sys.argv[1])
    a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None)
    root=Path(tempfile.mkdtemp(prefix='dlmm-round6-'+'repro'+'-'));root.mkdir(exist_ok=True)
    # A tuple under importLedger is not an ownership receipt.
    f=FamilyUi(a,root/'foreign-tuple','foreign-tuple'); h=compat.Helper(old,f.data)
    p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0');h.close()
    foreign_id='abcdefabcdefabcdefabcdefabcdefab';foreign=f.data/'payloads'/f'{foreign_id}.vpk';foreign.write_bytes(b'foreign proof fixture')
    with sqlite3.connect(f.data/'library.sqlite3') as db:
     d=json.loads(db.execute('SELECT body FROM library').fetchone()[0]);d.setdefault('importLedger',{})['unrelatedManifestProperty']={'id':foreign_id,'sha256':hashlib.sha256(foreign.read_bytes()).hexdigest(),'size':foreign.stat().st_size};db.execute('UPDATE library SET body=?',(json.dumps(d),))
    h=compat.Helper(new,f.data); scan=h.ok('native.maintenance.scan');row=next(r for r in scan['rows'] if r.get('id')==foreign_id);print('FOREIGN_TUPLE',json.dumps(row),flush=True);assert row['removable'] is False and foreign.exists();h.close()
    # A normal old-writer import into a copy profile, then a foreign same-bytes
    # inode replaces its payload before the new writer sees the library.
    f=FamilyUi(a,root/'backfill-replacement','backfill-replacement');h=compat.Helper(old,f.data)
    p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
    r=h.ok('native.profile',action='copy',profileId='default',name='Temporary',operationId='copy',expectedRevision='1');pid=r['selectedProfileId'];rev=r['revision']
    source=f.root/'foreign-input_dir.vpk';source.write_bytes(vpk());job='b'*32
    h.ok('native.local.prepare',jobId=job,profileId=pid,expectedRevision=rev,roots=[str(source)])
    deadline=time.monotonic()+30
    while True:
     status=h.ok('native.local.status',jobId=job)
     if status['phase']=='ready':break
     assert status['phase'] not in ('failed','interrupted') and time.monotonic()<deadline,status
     time.sleep(.02)
    status=h.ok('native.local.status',jobId=job,includeCandidates=True)['candidateSummary']; ids=[v['id'] for v in status['candidates'] if v.get('available',True) and not v.get('unavailableReason') and v.get('kind')!='companion']
    review=h.ok('native.local.review',jobId=job,candidateIds=ids,metadata=status['defaultMetadata']);h.ok('native.local.commit',jobId=job,token=review['token'],profileId=pid,expectedRevision=rev,operationId='local');h.close()
    d=compat.doc(f.data);m=next(m for m in d['profiles'][pid]['mods'] if m['source']['id']==review['modId']);
    sha=hashlib.sha256(source.read_bytes()).hexdigest(); matches=[p for p in (f.data/'payloads').glob('*.vpk') if hashlib.sha256(p.read_bytes()).hexdigest()==sha];assert len(matches)==1,matches
    victim=matches[0];before=victim.stat().st_ino;replacement=victim.with_suffix('.replacement');replacement.write_bytes(victim.read_bytes());os.replace(replacement,victim);after=victim.stat().st_ino;assert before!=after
    h=compat.Helper(new,f.data);scan=h.ok('native.maintenance.scan');row=next(r for r in scan['rows'] if r.get('id')==victim.stem);assert row['kind']=='unproven' and row['bytes']==victim.stat().st_size;h.close()
    h=compat.Helper(new,f.data);r=h.ok('native.profile',action='remove',profileId=pid,operationId='remove',expectedRevision=d['revision']);scan=h.ok('native.maintenance.scan');row=next(r for r in scan['rows'] if r.get('id')==victim.stem);print('REPLACEMENT_SCAN',json.dumps({'old_inode':before,'foreign_inode':after,'row':row,'canClean':scan['canClean']}),flush=True)
    assert row['removable'] is False

    if scan['canClean']: h.ok('native.maintenance.clean',operationId='clean',expectedRevision=scan['revision'],token=scan['token'])
    print('REPLACEMENT_DELETED',not victim.exists(),flush=True);assert victim.exists();h.close()
    shutil.rmtree(root)

def migration():
    import tempfile
    import sys,json,sqlite3,time,os
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi,vpk
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')));c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
    old=Path(sys.argv[2]);new=Path(sys.argv[1]);a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None);root=Path(tempfile.mkdtemp(prefix='dlmm-round6-'+'migration'+'-'))
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
    h=c.Helper(new,f.data);snap=h.call('native.snapshot');edit=h.call('native.profile',action='copy',name='Rescue',profileId='default',expectedRevision=d['revision'],operationId='copy');print('CONFLICT_SNAPSHOT',json.dumps(snap));print('CONFLICT_EDIT',json.dumps(edit));assert 'result' in snap and 'result' in edit; assert snap['result']['canRestore']==old_snapshot['result']['canRestore'];h.close()
    shutil.rmtree(root)

def late_origin():
    import tempfile
    import sys, time, json
    from pathlib import Path
    from types import SimpleNamespace
    sys.path.insert(0,str(Path(__file__).resolve().parent))
    from nativeintegrationcheck import FamilyUi,vpk
    import importlib.util
    spec=importlib.util.spec_from_file_location('compat',str(Path(__file__).with_name('game-format-cross-version-check.py')));c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
    old=Path(sys.argv[2]);new=Path(sys.argv[1]);a=SimpleNamespace(old_helper=old,helper=new,binary=None,capture_dir=None)
    f=FamilyUi(a,Path(tempfile.mkdtemp(prefix='dlmm-round6-late-')),'late-origin');h=c.Helper(old,f.data)
    p=h.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));h.ok('native.importCommit',token=p['token'],operationId='import',expectedRevision='0')
    p=h.ok('native.applyPreview',profileId='default',gamePath=str(f.game));h.ok('native.apply',token=p['token'],operationId='apply',expectedRevision='1')
    before=f.info.read_bytes();d=c.doc(f.data);up=d['previous']['upstreamPayloads'];print('BEFORE_LOCAL_BASELINES',bool(d['previous'].get('installedGameOrigins')), 'MARKED_ORIGINS',sum(1 for p in up if p.get('installedOrigin')),flush=True)
    added=f.addons/'foreign-after-apply_dir.vpk';added.write_bytes(vpk());job='e'*32;rev=d['revision']
    h.ok('native.local.prepare',jobId=job,profileId='default',expectedRevision=rev,roots=[str(added)])
    for _ in range(2000):
     status=h.ok('native.local.status',jobId=job)
     if status['phase']=='ready':break
     assert status['phase'] not in ('failed','interrupted'),status;time.sleep(.01)
    else:raise Exception('timeout')
    s=h.ok('native.local.status',jobId=job,includeCandidates=True)['candidateSummary'];ids=[v['id'] for v in s['candidates'] if v.get('available',True) and not v.get('unavailableReason') and v.get('kind')!='companion'];review=h.ok('native.local.review',jobId=job,candidateIds=ids,metadata=s['defaultMetadata']);h.ok('native.local.commit',jobId=job,token=review['token'],profileId='default',expectedRevision=rev,operationId='local');h.close()
    d=c.doc(f.data);origins=[o for m in d['profiles']['default']['mods'] for o in m['source'].get('installedGameOrigins',[])];assert len(origins)==1 and any(p['sourcePath']==str(added) for p in origins[0]['files'])
    h=c.Helper(new,f.data);result=h.call('native.restore',operationId='restore',expectedRevision=d['revision']);print('LATE_ORIGIN_RESTORE',json.dumps(result),flush=True);print('LIVE_GAMEINFO_CHANGED',f.info.read_bytes()!=before,'MOUNTS_ADDED_VPK',b'Game citadel/addons' in f.info.read_bytes() and added.exists(),flush=True);h.close()

    # Later pre-rename helpers record full origins and report externalChange;
    # earlier helpers report missing proof. Both must preserve every byte.
    assert result.get('error',{}).get('data',{}).get('kind') in ('unverifiedRestore','externalChange'),result
    assert f.info.read_bytes()==before

    shutil.rmtree(f.root)

if __name__=='__main__':
    for case in (repro,migration,late_origin):
        case()
        print('PASS round6',case.__name__,flush=True)
