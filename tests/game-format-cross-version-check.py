#!/usr/bin/env python3
"""Actual pre-rename writer fixtures and faithful replay of their game journals."""
import argparse, hashlib, json, os, sqlite3, subprocess, sys, tempfile, time
from pathlib import Path
from nativeintegrationcheck import FamilyUi, vpk, fingerprints, logical_document

def digest(b): return hashlib.sha256(b).hexdigest()
def canonical(v): return json.dumps(v,sort_keys=True,separators=(',',':')).encode()
def identity(p):
    s=p.stat(); return dict(device=s.st_dev,inode=s.st_ino,size=s.st_size,modifiedSeconds=s.st_mtime_ns//10**9,modifiedNanos=s.st_mtime_ns%10**9)
class Helper:
    def __init__(self,binary,data):
        self.data=data; self.p=subprocess.Popen([str(binary)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL,text=True)
    def call(self,m,**p):
        self.p.stdin.write(json.dumps(dict(jsonrpc='2.0',id=1,method=m,params=dict(dataDir=str(self.data),**p)))+'\n');self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())
    def ok(self,m,**p):
        r=self.call(m,**p);assert 'result' in r,(m,r);return r['result']
    def close(self): self.p.stdin.close();self.p.wait(timeout=30)
def doc(data):
    with sqlite3.connect(data/'library.sqlite3') as db:return logical_document(json.loads(db.execute('SELECT body FROM library').fetchone()[0]))
def installed_origin_matrix(a,td):
    for case in ('all-overlap-remove-before-migration','all-overlap-remove-after-migration','all-overlap-intact','all-overlap-addons-added','intact','addons-added','addons-removed','addons-changed','repair-added','repair-removed','repair-changed'):
        all_overlap=case.startswith('all-overlap-');mutation=case.removeprefix('all-overlap-')
        f=FamilyUi(a,Path(td)/('installed-'+case),'installed-'+case);c=f.info.parent
        # Standalone VPK shares a manifest row; another is solely installed-origin evidence.
        shared=c/'addons/shared_dir.vpk';shared.write_bytes(vpk())
        extra=c/'addons/extra_dir.vpk';extra.write_bytes(vpk())
        body=json.loads((c/'addons/.dmm.json').read_text());body['mods']['shared']=dict(enabled=True,order=2,shard=1,currentVpks=[shared.name],disabledVpks=[],originalVpkNames=[shared.name]);(c/'addons/.dmm.json').write_text(json.dumps(body))
        if all_overlap:
            body['mods']['extra']=dict(enabled=True,order=3,shard=1,currentVpks=[extra.name],disabledVpks=[],originalVpkNames=[extra.name]);(c/'addons/.dmm.json').write_text(json.dumps(body))
        repair=c/'dmm_localization_default';repair.mkdir();rp=repair/'old.vpk';rp.write_bytes(b'old repair')
        f.info.write_bytes(f.info.read_bytes().replace(b'   Game citadel\r\n',b'   Game citadel/dmm_localization_default\r\n   Game citadel\r\n'))
        f.paths += [shared,extra,rp];f.originals=fingerprints(f.paths)
        old=Helper(a.old_helper.resolve(),f.data)
        preview=old.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));old.ok('native.importCommit',token=preview['token'],operationId='import',expectedRevision='0')
        rev=doc(f.data)['revision'];job='a'*32
        old.ok('native.local.prepare',jobId=job,profileId='default',expectedRevision=rev,roots=[str(shared)])
        deadline=time.monotonic()+20
        while True:
            status=old.ok('native.local.status',jobId=job)
            if status['phase']=='ready':break
            assert status['phase'] not in ('failed','interrupted'),status
            assert time.monotonic()<deadline,status
            time.sleep(.01)
        status=old.ok('native.local.status',jobId=job,includeCandidates=True)['candidateSummary']
        candidates=[v['id'] for v in status['candidates'] if v.get('available',True) and not v.get('unavailableReason') and v.get('kind')!='companion']
        review=old.ok('native.local.review',jobId=job,candidateIds=candidates,metadata=status['defaultMetadata'])
        old.ok('native.local.commit',jobId=job,token=review['token'],profileId='default',expectedRevision=rev,operationId='installed')
        d=doc(f.data);mod=review['modId'];assert any(m['source'].get('installedGameOrigins') for m in d['profiles']['default']['mods']),d
        old.ok('native.setEnabled',profileId='default',modId=mod,enabled=True,expectedRevision=d['revision'],operationId='enable')
        original_gameinfo=f.info.read_bytes()
        d=doc(f.data);preview=old.ok('native.applyPreview',profileId='default',gamePath=str(f.game));old.ok('native.apply',token=preview['token'],operationId='apply',expectedRevision=d['revision']);old.close()
        has_baseline=bool(doc(f.data)['previous'].get('installedGameOrigins'))
        if 'remove-' in case:
            d=doc(f.data);held=[]
            for profile in d['profiles'].values():
                for m in profile['mods']:
                    origins=m['source'].pop('installedGameOrigins',None)
                    if origins:
                        copy=json.loads(json.dumps(m));copy['source']['installedGameOrigins']=origins;held.append(copy)
            d['profiles']['evidence']=dict(id='evidence',name='Evidence',protected=False,mods=held)
            with sqlite3.connect(f.data/'library.sqlite3') as db:db.execute('UPDATE library SET body=?',(json.dumps(d),))
            h=Helper((a.old_helper if 'before-migration' in case else a.helper).resolve(),f.data)
            h.ok('native.profile',action='remove',profileId='evidence',expectedRevision=d['revision'],operationId='remove-evidence');h.close()
            assert bool(doc(f.data)['previous'].get('installedGameOrigins'))==has_baseline
            (c/'addons/added.vpk').write_bytes(vpk());before=f.info.read_bytes()
            h=Helper(a.helper.resolve(),f.data)
            result=h.call('native.restore',operationId='restore',expectedRevision=doc(f.data)['revision'])
            assert 'error' in result,result
            assert result['error'].get('data',{}).get('kind') in ('unverifiedRestore','externalChange'),result
            assert f.info.read_bytes()==before;h.close()
            print('PASS actual 2778c01 checkpoint profile removal',case,flush=True);continue
        old=Helper(a.old_helper.resolve(),f.data);old.ok('native.snapshot');old.close()
        before=f.info.read_bytes();new=Helper(a.helper.resolve(),f.data);new.ok('native.snapshot');new.close()
        old=Helper(a.old_helper.resolve(),f.data);old.ok('native.snapshot');old.close() # read-only new open leaves old compatibility
        if mutation=='addons-added':(c/'addons/late_dir.vpk').write_bytes(vpk())
        elif mutation=='addons-removed':shared.unlink()
        elif mutation=='addons-changed':shared.write_bytes(b'changed addon')
        elif mutation=='repair-added':(repair/'late.vpk').write_bytes(b'added repair')
        elif mutation=='repair-removed':rp.unlink()
        elif mutation=='repair-changed':rp.write_bytes(b'changed repair')
        new=Helper(a.helper.resolve(),f.data)
        snapshot=new.ok('native.snapshot')
        result=new.call('native.restore',operationId='restore',expectedRevision=doc(f.data)['revision'])
        if has_baseline and mutation=='intact':
            assert snapshot['canRestore'] is True and 'result' in result,result
            assert f.info.read_bytes()==original_gameinfo
            print('PASS pre-rename verified checkpoint exact Restore',case,flush=True)
        else:
            allowed=('unverifiedRestore','externalChange','externalGameinfoChange','nativeIo') if mutation.endswith('-removed') else ('unverifiedRestore','externalChange','externalGameinfoChange')
            assert result.get('error',{}).get('data',{}).get('kind') in allowed,(case,result)
            assert f.info.read_bytes()==before
            print('PASS pre-rename missing or changed baseline refuses Restore',case,flush=True)
        new.close()

def main():
    ap=argparse.ArgumentParser();ap.add_argument('--helper',type=Path,required=True);ap.add_argument('--old-helper',type=Path,required=True);ap.add_argument("--binary",type=Path);ap.add_argument("--capture-dir",type=Path);a=ap.parse_args()
    assert a.old_helper.is_file(), f'Expected pre-rename helper is missing: {a.old_helper}'
    subprocess.run([sys.executable,str(Path(__file__).with_name("round9-review-regressions.py")),str(a.helper.resolve()),"--old-helper",str(a.old_helper.resolve())],check=True)
    with tempfile.TemporaryDirectory(prefix='dlmm-game-compat-') as td:
      subprocess.run([sys.executable,str(Path(__file__).with_name('round6-review-regressions.py')),str(a.helper.resolve()),str(a.old_helper.resolve())],check=True)
      subprocess.run([sys.executable,str(Path(__file__).with_name('round7-review-regressions.py')),str(a.helper.resolve()),'--old-helper',str(a.old_helper.resolve())],check=True)
      installed_origin_matrix(a,td)
      for sharded in (False,True):
       for recovery in (None,'rollback','forward'):
        root=Path(td)/f'{sharded}-{recovery}';f=FamilyUi(a,root,'old-writer');c=f.info.parent
        repair=c/'dmm_localization_default';repair.mkdir();(repair/'old.vpk').write_bytes(b'old repair')
        f.info.write_bytes(f.info.read_bytes().replace(b'   Game citadel\r\n',b'   Game citadel/dmm_localization_default\r\n   Game citadel\r\n'))
        old=Helper(a.old_helper.resolve(),f.data)
        preview=old.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game))
        old.ok('native.importCommit',token=preview['token'],operationId='import',expectedRevision='0')
        initial=doc(f.data);before=f.info.read_bytes();before_id=identity(f.info);os.link(f.info,root/'before-held')
        layout={'deploymentLayout':'shards-v1'} if sharded else {}
        preview=old.ok('native.applyPreview',profileId='default',gamePath=str(f.game),**layout)
        params=dict(token=preview['token'],operationId='apply',expectedRevision=initial['revision'],**layout)
        old.ok('native.apply',**params);old.close();after_doc=doc(f.data);after=f.info.read_bytes();applied=after_doc['applied']
        with sqlite3.connect(f.data/'library.sqlite3') as db:operation_input=json.loads(db.execute("SELECT input FROM operations WHERE id='apply'").fetchone()[0])
        print('PASS actual pre-rename checkpoint writer',sharded,recovery,flush=True)
        if recovery:
            # Replay prepare/prepare_transition from 2778c01, using actual writer
            # output for every checkpoint, manifest, content hash and inode.
            projected=json.loads(json.dumps(after_doc));projected['revision']=initial['revision']
            j=dict(kind='game',version=5 if sharded else int(after_doc['storeVersion']),expectedRevision=initial['revision'],input=operation_input,citadel=str(c),before=before.decode(),after=after.decode(),beforeSha256=digest(before),afterSha256=digest(after),document=projected,workspaceExisted=False,temp='.native-gameinfo-'+'a'*32,generation=applied['generation'],fontStateDigest=digest(canonical(dict(version=1,installed=None,previous=None))))
            if sharded:
                generation=c/'deadlock_native'/applied['generation'];names=['']+[f'shard{i+1:02}' for i in range(len(applied['manifest']['shards']))];files=[]
                for i,shard in enumerate(applied['manifest']['shards']):
                    for slot,pkg in enumerate(shard['packages'],1):
                        files.append(f'shard{i+1:02}/pak{slot:02}_dir.vpk')
                        files.extend(f'shard{i+1:02}/pak{slot:02}_{v["archiveIndex"]:03}.vpk' for v in pkg['companions'])
                nodes=[]
                for name in names+files+['manifest.json']:
                    ident=identity(generation/name);row=dict(kind='directory' if name in names else 'file',name=name,device=ident['device'],inode=ident['inode'])
                    if name not in names:row['complete']=True
                    nodes.append(row)
                temp_identity=identity(f.info);temp_identity['size']=0
                j.update(beforeIdentity=before_id,libraryVersionBefore=initial['storeVersion'],libraryVersionAfter=4,packageVersion=initial.get('packageVersion',initial['storeVersion']),deploymentIntent=applied.get('deploymentIntent'),desiredSignature=applied.get('desiredSignature'),beforeDocument=initial,stagedNodes=nodes,tempIdentity=temp_identity if recovery=='forward' else None)
            else:
                j.update(generationIdentity=applied['directoryIdentity'],stagedFiles=[dict(name=n,device=v['device'],inode=v['inode'],complete=True) for n,v in applied['identities'].items()])
            if recovery=='rollback':f.info.unlink();os.link(root/'before-held',f.info)
            with sqlite3.connect(f.data/'library.sqlite3') as db:
                db.execute('UPDATE library SET body=?',(json.dumps(initial),));db.execute("DELETE FROM operations WHERE id='apply'");db.execute('INSERT INTO journals VALUES (?,?)',('apply',json.dumps(j)))
            new=Helper(a.helper.resolve(),f.data);new.ok('native.recover',operationId='recover',expectedRevision=initial['revision']);new.close()
            assert f.info.read_bytes()==(before if recovery=='rollback' else after)
            print('PASS legacy journal',sharded,recovery,flush=True);continue
        new=Helper(a.helper.resolve(),f.data)
        review=new.ok('native.applyPreview',restoreReview=True);assert review=={'restoreReview':True},review
        has_baseline=bool(after_doc['previous'].get('installedGameOrigins'))
        result=new.call('native.restore',operationId='restore',expectedRevision=after_doc['revision'])
        if has_baseline:
            assert 'result' in result,result
            assert f.info.read_bytes()==before
            assert 'gameWorkspace' not in doc(f.data)
            print('PASS verified pre-rename exact Restore without workspace fields',sharded,flush=True)
        else:
            assert result.get('error',{}).get('data',{}).get('kind')=='unverifiedRestore',result
            assert f.info.read_bytes()==after
            print('PASS historical checkpoint without retained baseline refuses Restore',sharded,flush=True)
        new.close()
        old=Helper(a.old_helper.resolve(),f.data)
        result=old.call('native.snapshot')
        assert 'result' in result or result.get('error',{}).get('data',{}).get('kind')=='unsupportedVersion',result
        old.close()
      # Replay the old storage writer using its actual review rows. Its file
      # enumeration omits skip history; the new reader must carry it separately.
      f=FamilyUi(a,Path(td)/'old-storage','old-storage');old=Helper(a.old_helper.resolve(),f.data)
      preview=old.ok('native.importPreview',statePath=str(f.source),gamePath=str(f.game));old.ok('native.importCommit',token=preview['token'],operationId='import',expectedRevision='0')
      (f.data/'remote-skips.v1.json').write_bytes(b'legacy skip history');target=f.root/'destination';target.mkdir()
      review=old.ok('native.storage.review',destination=str(target));initial=doc(f.data);old.close()
      # Use the actual old writer's rows: some releases already include skip history.
      j=dict(kind='storageMove',version=1,before=initial,source=dict(path=review['source']['path'],identity=review['sourceIdentity']),destination=dict(path=review['destination']['path'],identity=review['destinationIdentity']),files=review['files'],input=dict(method='native.storage.move',params=dict(operationId='move')))
      with sqlite3.connect(f.data/'library.sqlite3') as db:
          db.execute('INSERT INTO journals VALUES (?,?)',('move',json.dumps(j)));db.execute('UPDATE library SET body=?',(json.dumps(initial|dict(storeVersion=5,storageDocumentVersion=initial['storeVersion'])),))
      new=Helper(a.helper.resolve(),f.data);new.ok('native.storage.resume');new.close()
      assert list(target.rglob('remote-skips.v1.json'))[0].read_bytes()==b'legacy skip history'
      print('PASS faithful pre-rename storage journal with actual writer rows and skip history',flush=True)
if __name__=='__main__':main()
