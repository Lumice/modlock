#!/usr/bin/env python3
"""Opt-in real-helper qualification. Run with python3 -I; no third-party modules."""
import argparse
import binascii
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import sqlite3
import struct
import subprocess
import tempfile
import threading
import time
import traceback
import uuid
import zipfile

BEFORE = b'// exact CRLF fixture\r\n"GameInfo"\r\n{\r\n FileSystem\r\n {\r\n  SearchPaths\r\n  {\r\n   Game citadel/addons\r\n   Game_LowViolence citadel_lv\r\n   Game citadel\r\n   Mod citadel\r\n   Game core\r\n  }\r\n }\r\n}\r\n'
def uid():
    return uuid.uuid4().hex

def vpk(name, padding=0):
    folder, base = name.rsplit('/', 1)
    stem, ext = base.rsplit('.', 1)
    tree = b'\0'.join(x.encode() for x in (ext, folder, stem)) + b'\0'
    tree += struct.pack('<IHHIIH', 7, 0, 0x7fff, 0, 0, 0xffff) + b'\0\0\0'
    return struct.pack('<III', 0x55aa1234, 1, len(tree)) + tree + bytes(padding)

def fingerprint(root):
    return {str(p.relative_to(root)): ('dir' if p.is_dir() else hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mode & 0o777) for p in sorted(root.rglob('*'))}

class RPC:
    def __init__(self, helper, root):
        self.log = (root / 'rpc.jsonl').open('a')
        env = dict(os.environ)
        for k in ('HOME', 'XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'TMPDIR'):
            env[k] = str(root / k.lower())
            Path(env[k]).mkdir(exist_ok=True)
        env['PATH'] = str(root / 'bin') + ':' + env.get('PATH', '')
        self.err = (root / 'helper.stderr').open('a')
        self.p = subprocess.Popen([str(helper)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err, env=env, text=True, cwd=root, start_new_session=True)
        self.q = queue.Queue()
        def reader():
            for line in self.p.stdout:
                self.q.put(line)
            self.q.put(None)
        threading.Thread(target=reader, daemon=True).start()
        self.n = 0
        self.timeout = 180
    def send(self, method, **params):
        self.n += 1
        req = dict(jsonrpc='2.0', id=self.n, method=method, params=params)
        self.log.write(json.dumps({'request': req})+'\n'); self.log.flush()
        self.p.stdin.write(json.dumps(req)+'\n'); self.p.stdin.flush()
    def receive(self):
        line = self.q.get(timeout=self.timeout)
        assert line is not None, 'helper exited'
        response = json.loads(line)
        self.log.write(json.dumps({'response': response})+'\n'); self.log.flush()
        assert response.get('id') == self.n, response
        assert 'error' not in response, response
        return response['result']
    def call(self, method, **params):
        self.send(method, **params)
        return self.receive()
    def close(self, kill=False):
        if self.p.poll() is None:
            if kill:
                os.killpg(self.p.pid, signal.SIGKILL)
            else:
                self.p.stdin.close()
            try: self.p.wait(timeout=15)
            except subprocess.TimeoutExpired:
                os.killpg(self.p.pid, signal.SIGKILL); self.p.wait()
        self.log.close(); self.err.close()

class Qualification:
    def __init__(self, helper, root, submission):
        self.helper, self.root, self.submission = helper, root, submission
        self.game = root / 'steam/steamapps/common/Deadlock'
        self.citadel = self.game / 'game/citadel'
        self.addons = self.citadel / 'addons'
        self.addons.mkdir(parents=True)
        self.data = root / 'data'
        self.source = root / 'upstream/state.json'
        self.source.parent.mkdir()
        self.rpc = RPC(helper, root)
        self.rev = '0'
        self.results = []
    def call(self, method, **p):
        return self.rpc.call('native.'+method, dataDir=str(self.data), **p)
    def snap(self):
        s = self.call('snapshot', profileId='default'); self.rev = s['revision']; return s
    def mutate(self, method, **p):
        r = self.call(method, expectedRevision=self.rev, operationId=uid(), **p)
        self.snap()
        return r
    def poll(self, method, done, **p):
        end = time.monotonic()+150
        while time.monotonic() < end:
            r = self.call(method, **p)
            assert r.get('phase') not in ('failed','cancelled','interrupted') and r.get('state') not in ('failed','cancelled','abandoned'), r
            if done(r): return r
            time.sleep(.25)
        raise AssertionError(('timeout', method, r))
    def step(self, name, fn):
        try:
            detail = fn() or 'verified'
            self.results.append((name, 'PASS', detail))
        except Exception as e:
            self.results.append((name, 'FAIL', str(e)))
            traceback.print_exc()
        print(self.results[-1], flush=True)
    def fixture(self):
        self.payload = vpk('models/qualification.vmdl_c')
        (self.addons/'pak01_dir.vpk').write_bytes(self.payload)
        (self.addons/'unmanaged.txt').write_bytes(b'preserve unknown bytes\x00\xff')
        (self.citadel/'gameinfo.gi').write_bytes(BEFORE)
        (self.addons/'.dmm.json').write_text(json.dumps(dict(version=3, mods={'fixture-remote': dict(enabled=True, order=0, shard=1, currentVpks=['pak01_dir.vpk'], disabledVpks=[], originalVpkNames=['fixture_dir.vpk'])})))
        mods = [dict(id='fixture-a', remoteId='fixture-remote', name='Synthetic Ivy — Täht', status='installed', installOrder=0)]
        state = {'local-config': dict(version=33, state=dict(activeProfileId='default', localMods=mods, profiles={'default':dict(id='default', name='Default',mods=mods, enabledMods={'fixture-remote':dict(enabled=True)})}))}
        self.source.write_text(json.dumps(state))
        self.baseline = fingerprint(self.game)
        return 'schema-33 state, manifest v3, valid VPK and CRLF SearchPaths; isolated HOME/XDG/TMPDIR'
    def upstream(self):
        p = self.call('importPreview', statePath=str(self.source), gamePath=str(self.game))
        self.mutate('importCommit', token=p['token'])
        assert next(m for m in self.snap()['mods'] if m['id']=='fixture-a')['available']
        self.mutate('setEnabled', profileId='default', modId='fixture-a', enabled=False)
        assert not next(m for m in self.snap()['mods'] if m['id']=='fixture-a')['enabled']
        self.mutate('setEnabled', profileId='default', modId='fixture-a', enabled=True)
        for action in ('create','copy'):
            r = self.mutate('profile', action=action, profileId='default', name='Qualification '+action)
            pid = r['selectedProfileId']
            assert pid != 'default', r
            self.mutate('profile', action='rename', profileId=pid, name='Renamed')
            self.mutate('profile', action='remove', profileId=pid)
        return 'import preview/commit, toggle, create/copy/rename/remove'
    def edit(self, change):
        r = self.call('profileEdit.preview', profileId='default', expectedRevision=self.rev, change=change)
        self.mutate('profileEdit.commit', profileId='default', token=r['token'])
    def apply(self):
        p = self.call('applyPreview', profileId='default', gamePath=str(self.game))
        self.mutate('apply', token=p['token'])
        return p
    def restore(self):
        self.mutate('restore')
    def disk(self):
        p = self.apply()
        dirs = list((self.citadel/'modlock').glob('*/pak01_dir.vpk'))
        assert len(dirs)==1 and dirs[0].read_bytes()==self.payload, dirs
        mount = 'citadel/deadlock_native/'+dirs[0].parent.name
        expected = BEFORE.replace(b'   Game citadel/addons\r\n', b'').replace(b'  {\r\n', b'  {\r\n\t\t\tGame\t"'+mount.encode()+b'"\r\n')
        assert (self.citadel/'gameinfo.gi').read_bytes()==expected
        self.restore()
        after = fingerprint(self.game)
        assert after==self.baseline, {'added': sorted(after.keys()-self.baseline.keys()), 'removed': sorted(self.baseline.keys()-after.keys()), 'changed':[k for k in after.keys() & self.baseline.keys() if after[k]!=self.baseline[k]]}
        return 'deployed VPK and entire gameinfo bytes exact; full tree restored'
    def local(self, kind):
        folder = self.root/('input-'+kind); folder.mkdir()
        src = folder/'local_dir.vpk'; content = vpk('models/'+kind+'.vmdl_c'); src.write_bytes(content)
        if kind=='folder':
            (folder/'second_dir.vpk').write_bytes(vpk('models/folder-second.vmdl_c'))
            src=folder
        elif kind=='zip':
            src=folder/'local.zip'
            with zipfile.ZipFile(src,'w') as z: z.writestr('nested/local_dir.vpk',content)
        elif kind=='rar':
            def header(kind,flags,body):
                h=struct.pack('<BHH',kind,flags,7+len(body))+body
                return struct.pack('<H',binascii.crc32(h)&0xffff)+h
            name=b'local_dir.vpk'
            main=header(0x73,0,bytes(6))
            file=header(0x74,0x8000,struct.pack('<IIBIIBBHI',len(content),len(content),3,binascii.crc32(content),0,20,0x30,len(name),0o100644)+name)
            src=folder/'local.rar'
            src.write_bytes(b'Rar!\x1a\x07\x00'+main+file+content+header(0x7b,0,b''))
        elif kind=='7z':
            src=folder/'local.7z'
            subprocess.run(['7z','a',str(src),str(folder/'local_dir.vpk')],stdout=subprocess.DEVNULL,check=True)
        job=uid()
        self.call('local.prepare',jobId=job,profileId='default',expectedRevision=self.rev,roots=[str(src)])
        self.poll('local.status',lambda x:x['phase']=='ready',jobId=job)
        s=self.call('local.status',jobId=job,includeCandidates=True)['candidateSummary']
        review=self.call('local.review',jobId=job,candidateIds=sorted(s['defaultSelectedCandidateIds']),metadata=s['defaultMetadata'])
        before={m['id'] for m in self.snap()['mods']}
        self.mutate('local.commit',jobId=job,token=review['token'],profileId='default')
        mod=next(m for m in self.snap()['mods'] if m['id'] not in before)
        self.mutate('setEnabled',profileId='default',modId=mod['id'],enabled=True)
        detail=self.call('options.get',profileId='default',modId=mod['id'],expectedRevision=self.rev)
        (self.root/('options-'+kind+'.json')).write_text(json.dumps(detail,indent=2))
        self.apply()
        assert any(p.read_bytes()==content for p in (self.citadel/'modlock').rglob('*.vpk'))
        self.restore()
        if kind=='folder':self.folder_mod=mod['id']
        return mod['id']
    def selection_order(self):
        mods=self.snap()['mods']; mod=next(m for m in mods if m['id']==self.folder_mod)
        d=self.call('options.get',profileId='default',modId=mod['id'],expectedRevision=self.rev)
        self.edit(dict(kind='selection',modId=mod['id'],optionIds=[sorted(d['detail']['catalog']['selectedIds'])[0]]))
        ids=[m['id'] for m in mods][::-1]
        self.edit(dict(kind='order',modIds=ids))
        assert [m['id'] for m in self.snap()['mods']]==ids
        return 'saved options selection review/commit and reversed precedence'
    def launch_control(self,method,**p):
        control=uid(); self.call('launch.'+method,controlId=control,**p)
        r=self.poll('launch.control',lambda x:x['state']!='pending',controlId=control)
        assert r['state']=='succeeded',r
        return r['value']
    def launch(self):
        before=fingerprint(self.game)
        opened=self.launch_control('open',openId=uid())
        assert opened['admitted']=='0',opened
        settings=self.launch_control('settings.get')
        (self.root/'launch-open.json').write_text(json.dumps([opened,settings],indent=2))
        game=self.call('game.status')
        assert game['state']=='idle',game
        attempt=uid()
        self.launch_control('begin',epoch=opened['epoch'],attemptId=attempt,profileId='default',mode='modded',expectedRevision=self.rev,settingsRevision=settings['revision'])
        r=self.poll('launch.status',lambda x:x['phase'] in ('reviewReady','failed','ready'),epoch=opened['epoch'],attemptId=attempt)
        assert r['phase'] in ('ready','reviewReady'),r
        assert r['handoffState']=='notDispatched' and r['observationState']=='notStarted',r
        assert not self.call('launch.owner')['childRetained']
        assert fingerprint(self.game)==before
        (self.root/'launch-ready.json').write_text(json.dumps(r,indent=2))
        self.call('launch.cancel',epoch=opened['epoch'],attemptId=attempt)
        self.rpc.close(); self.rpc=RPC(self.helper,self.root); self.snap()
        return 'game idle; preparation reached review, no launch.commit or Steam/game spawn'
    def remote_setup(self):
        self.session=uid()
        def remote(method,**p):
            end=time.monotonic()+30
            while True:
                try:return self.call('remote.'+method,version=1,sessionId=self.session,**p)
                except AssertionError as e:
                    pending = 'remoteBusy' in str(e) or (method in ('commit','replaceCommit') and 'outcomeUnknown' in str(e))
                    if not pending or time.monotonic()>end:raise
                    time.sleep(.5)
        self.remote_call=remote
        end=time.monotonic()+30
        while True:
            r=remote('open')
            if r['phase']=='ready':break
            assert r['phase']!='failed' and time.monotonic()<end,r
            time.sleep(.25)
        key=uid(); remote('resolve',requestKey=key,submission=dict(provider='gamebanana',submissionType='mod',submissionId=self.submission))
        def wait(method,done,**p):
            end=time.monotonic()+150
            while time.monotonic()<end:
                r=remote(method,**p)
                assert r.get('phase')!='failed' and r.get('state')!='failed',r
                if done(r):return r
                time.sleep(1)
            raise AssertionError(r)
        self.remote_wait=wait
        resolved=wait('resolveStatus',lambda r:r['phase']=='ready',requestKey=key,page=0)
        candidates=[c for c in resolved['candidates'] if c['declaredSize'] and 0<int(c['declaredSize'])<=2*1024*1024 and c['list']=='current']
        assert candidates, ('no small current file',resolved)
        c=min(candidates,key=lambda c:int(c['declaredSize']))
        assert c['fileKey']['fileId']=='1809951', 'qualification file changed'
        return remote, wait, resolved, c
    def remote(self):
        remote,wait,resolved,c=self.remote_setup()
        job=uid(); self.remote_job=job
        remote('start',jobId=job,selectionToken=resolved['selectionToken'],candidateFingerprint=c['candidateFingerprint'],fileId=c['fileKey']['fileId'],profileId='default',expectedRevision=self.rev,acceptArchived=False)
        wait('status',lambda r:r['state']=='downloaded',jobId=job)
        prepare=uid(); remote('prepare',jobId=job,profileId='default',expectedRevision=self.rev,acknowledgeRebase=False,prepareKey=prepare)
        wait('status',lambda r:r['state']=='ready',jobId=job)
        cs=remote('candidates',jobId=job,prepareKey=prepare,page=0)['candidates']
        remote('review',jobId=job,profileId='default',expectedRevision=self.rev,requestKey=uid(),candidateIds=sorted(c['candidateId'] for c in cs if c['available']),metadata=dict(name='Qualification remote',author='',category='',description=''))
        status=wait('status',lambda r:(r.get('review') or {}).get('phase')=='ready',jobId=job)
        review=status['review']
        remote('commit',jobId=job,profileId='default',expectedRevision=self.rev,reviewToken=review['reviewToken'],operationId=uid(),consentFingerprint=c['candidateFingerprint'],cohortFingerprint=status['cohort']['fingerprint'],selectionDigest=review['selectionDigest'])
        wait('status',lambda r:r['state']=='installed',jobId=job)
        self.snap(); self.remote_mod=review['modId']
        self.remote_consent_fingerprint=c['candidateFingerprint']
        self.mutate('setEnabled',profileId='default',modId=self.remote_mod,enabled=True)
        self.apply()
        detail=self.call('options.get',profileId='default',modId=self.remote_mod,expectedRevision=self.rev)['detail']['catalog']
        hashes={hashlib.sha256(p.read_bytes()).hexdigest() for p in (self.citadel/'modlock').rglob('*.vpk')}
        selected=[e for e in detail['entries'] if e['id'] in detail['selectedIds']]
        assert selected
        assert all(e['package']['directory']['sha256'] in hashes for e in selected)
        self.restore()
        return f'GameBanana mod {self.submission}, file {c["fileKey"]["fileId"]}, {c["declaredSize"]} bytes'
    def reinstall(self, purpose='reinstall', resolution=None, file=None, target=None):
        remote,wait=self.remote_call,self.remote_wait
        if target is None:
            target=wait('replaceTarget',lambda r:r['phase']=='ready',requestKey=uid(),profileId='default',modId=self.remote_mod,expectedRevision=self.rev)['target']
        if resolution is None:
            key=uid()
            remote('resolve',requestKey=key,submission=target['receipt']['submission'])
            resolution=wait('resolveStatus',lambda r:r['phase']=='ready',requestKey=key,page=0)
            file=next(c for c in resolution['candidates'] if c['fileKey']['fileId']==target['receipt']['rootFileId'])
            assert file['candidateFingerprint']==self.remote_consent_fingerprint, 'provider changed; inspect update before reinstall'
        job=uid(); initial=self.rev
        remote('replaceStart',jobId=job,selectionToken=resolution['selectionToken'],candidateFingerprint=file['candidateFingerprint'],fileId=file['fileKey']['fileId'],profileId='default',modId=self.remote_mod,expectedRevision=self.rev,targetDigest=target['recordDigest'],purpose=purpose,acceptArchived=False)
        wait('replaceStatus',lambda r:r['status']['state']=='downloaded',jobId=job)
        prepare=uid()
        remote('prepare',jobId=job,profileId='default',expectedRevision=self.rev,acknowledgeRebase=False,prepareKey=prepare)
        wait('replaceStatus',lambda r:r['status']['state']=='ready',jobId=job)
        candidates=remote('candidates',jobId=job,prepareKey=prepare,page=0)['candidates']
        remote('replaceReview',jobId=job,profileId='default',modId=self.remote_mod,expectedRevision=self.rev,targetDigest=target['recordDigest'],requestKey=uid(),candidateIds=sorted(c['candidateId'] for c in candidates if c['available']))
        status=wait('replaceStatus',lambda r:(r.get('review') or {}).get('phase')=='ready',jobId=job)
        review=status['review']
        remote('replaceCommit',jobId=job,profileId='default',modId=self.remote_mod,purpose=purpose,initialRevision=initial,expectedRevision=self.rev,targetDigest=target['recordDigest'],afterRecordDigest=review['afterRecordDigest'],reviewToken=review['reviewToken'],operationId=uid(),consentFingerprint=file['candidateFingerprint'],cohortFingerprint=status['status']['cohort']['fingerprint'],selectionDigest=review['selectionDigest'])
        wait('replaceStatus',lambda r:r['status']['state']=='installed',jobId=job)
        self.snap()
        if purpose=='link':return
        self.verify_remote_bytes();self.restore()
        return 'fresh resolve matches installed receipt; Reinstall review/commit and Apply'
    def document(self):
        with sqlite3.connect('file:'+str(self.data/'library.sqlite3')+'?mode=ro',uri=True) as db:
            return json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
    def verify_remote_bytes(self):
        self.apply()
        cat=self.call('options.get',profileId='default',modId=self.remote_mod,expectedRevision=self.rev)['detail']['catalog']
        files=[]
        for entry in cat['entries']:
            if entry['id'] in cat['selectedIds']:
                package=entry['package']
                files.append(package['directory'])
                files.extend(c['file'] for c in package['companions'])
        deployed=[p.read_bytes() for p in (self.citadel/'modlock').rglob('*.vpk')]
        assert files
        for f in files:
            blob=(self.data/'payloads'/(f['id']+'.vpk')).read_bytes()
            assert hashlib.sha256(blob).hexdigest()==f['sha256']
            assert blob in deployed, 'deployed bytes differ from immutable payload'
    def link(self):
        self.fixture()
        state=json.loads(self.source.read_text())
        raw=state['local-config']['state']
        mod=raw['localMods'][0]
        mod.update(author='Qualification author', category='Crosshair', description='Preserved notes', installOrder=7)
        # A second unavailable row makes position and unrelated-row preservation observable.
        raw['localMods'].insert(0,dict(id='untouched',remoteId='absent',name='Untouched',status='downloaded',installOrder=0))
        self.source.write_text(json.dumps(state))
        self.upstream()
        self.mutate('setEnabled',profileId='default',modId='fixture-a',enabled=False)
        before=self.document()
        assert before['storeVersion']==2
        old=next(m for m in before['profiles']['default']['mods'] if m['source']['id']=='fixture-a')
        blobs=fingerprint(self.data/'payloads')
        remote,wait,resolved,file=self.remote_setup()
        self.remote_mod='fixture-a';self.remote_consent_fingerprint=file['candidateFingerprint']
        target=wait('replaceTarget',lambda r:r['phase']=='ready',requestKey=uid(),profileId='default',modId=self.remote_mod,expectedRevision=self.rev)['target']
        assert target['receipt'] is None
        self.reinstall(purpose='link',resolution=resolved,file=file,target=target)
        after=self.document()
        assert after['storeVersion']==4 and after['packageVersion']==2
        assert after['fonts']==dict(version=1,installed=None,previous=None)
        new=next(m for m in after['profiles']['default']['mods'] if m['source']['id']=='fixture-a')
        historical=dict(new['source']);overlay=historical.pop('nativeRemoteLink')
        assert historical==old['source']
        assert overlay==dict(version=1,association='userSelected',submission=overlay['receipt']['submission'],receipt=overlay['receipt'])
        assert overlay['receipt']['rootFileId']=='1809951'
        for k in ('enabled','previewHidden','available','unavailableReason'):assert new[k]==old[k],k
        assert [m['source']['id'] for m in after['profiles']['default']['mods']]==[m['source']['id'] for m in before['profiles']['default']['mods']]
        assert after['profiles']['default']['mods'][0]==before['profiles']['default']['mods'][0]
        for k in ('importArchive','importLedger','applied','previous'):
            assert after.get(k)==before.get(k),k
        assert all(fingerprint(self.data/'payloads').get(k)==v for k,v in blobs.items())
        self.mutate('setEnabled',profileId='default',modId=self.remote_mod,enabled=True)
        self.verify_remote_bytes();self.restore()
        self.reinstall()
        final=next(m for m in self.document()['profiles']['default']['mods'] if m['source']['id']==self.remote_mod)
        history=dict(final['source']);history.pop('nativeRemoteLink');assert history==old['source']
        assert all(fingerprint(self.data/'payloads').get(k)==v for k,v in blobs.items())
        assert fingerprint(self.game)==self.baseline
        return 'Store2 → Store4; null-receipt eligibility; history/order/flags/metadata and old blobs preserved; link review/commit; exact Apply; fresh Check/Reinstall'

    def interruption(self):
        # A PATH-local hash wrapper pauses only on a deployed generation VPK.
        # The helper has persisted generationIdentity and stagedFiles before hashing it.
        bindir=self.root/'bin'; bindir.mkdir(exist_ok=True)
        wrapper=bindir/'sha256sum'
        marker=self.root/'apply-hash-barrier'
        wrapper.write_text("#!/usr/bin/python3\nimport os,time\nfrom pathlib import Path\np=Path(os.readlink('/proc/self/fd/0'))\nif 'modlock' in p.parts and p.suffix=='.vpk':\n Path("+repr(str(marker))+ ").write_text(str(p))\n while True:time.sleep(.01)\nos.execv('/usr/bin/sha256sum',['sha256sum'])\n")
        wrapper.chmod(0o700)
        before=fingerprint(self.game)
        p=self.call('applyPreview',profileId='default',gamePath=str(self.game))
        self.rpc.send('native.apply',dataDir=str(self.data),expectedRevision=self.rev,operationId=uid(),token=p['token'])
        deadline=time.monotonic()+15
        while not marker.exists() and time.monotonic()<deadline:time.sleep(.01)
        try:
            assert marker.exists(),'Apply did not reach staged-payload hash barrier'
            with sqlite3.connect('file:'+str(self.data/'library.sqlite3')+'?mode=ro',uri=True) as db:
                journals=[json.loads(row[0]) for row in db.execute('SELECT body FROM journals')]
            assert any(j.get('kind')=='game' and j.get('generationIdentity') and j.get('stagedFiles') for j in journals),journals
            (self.root/'interrupted-journals.json').write_text(json.dumps(journals,indent=2))
            self.rpc.close(kill=True)
        finally:wrapper.unlink()
        self.rpc=RPC(self.helper,self.root)
        s=self.snap(); assert s['recoveryRequired'],s
        self.mutate('recover')
        assert not self.snap()['recoveryRequired']
        assert fingerprint(self.game)==before
        return 'SIGKILL during journaled Apply; restart recoveryRequired; Recover restores full tree'


def prior_bytes_case(helper, root, submission):
    """Mixed importability must preserve upstream bytes through Apply/Restore."""
    childroot = root/'prior-bytes'; childroot.mkdir()
    child = Qualification(helper, childroot, submission)
    try:
        child.fixture()
        bad = child.addons/'broken_dir.vpk'
        bad.write_bytes(b'not a VPK directory')
        manifest = json.loads((child.addons/'.dmm.json').read_bytes())
        manifest['mods']['broken'] = dict(enabled=True, shard=1, currentVpks=['broken_dir.vpk'], disabledVpks=[], originalVpkNames=['broken_dir.vpk'])
        (child.addons/'.dmm.json').write_text(json.dumps(manifest))
        state = json.loads(child.source.read_bytes())
        state['local-config']['state']['localMods'].append(dict(id='broken',remoteId='broken',name='Synthetic broken',status='installed',installOrder=1))
        state['local-config']['state']['profiles']['default']['enabledMods']['broken'] = dict(enabled=False)
        child.source.write_text(json.dumps(state))
        baseline = fingerprint(child.game)
        preview = child.call('importPreview', statePath=str(child.source), gamePath=str(child.game))
        summary = preview.get('summary',preview)
        assert (summary['availableModCount'],summary['unavailableModCount']) == (1,1)
        child.mutate('importCommit',token=preview['token'])
        child.apply(); child.restore()
        assert fingerprint(child.game) == baseline
        # Preserve the import baseline while checking initial Apply rejection.
        bad.write_bytes(b'changed opaque bytes')
        try:
            child.apply()
        except AssertionError as e:
            assert e.args[0]['error']['data']['kind'] == 'externalChange'
        else: raise AssertionError('changed prior bytes accepted by Apply')
        return 'available=1 unavailable=1 exactRestore; Apply tamper=externalChange (Restore tamper covered in Rust)'
    finally:
        child.rpc.close()


def real_library(helper, submission):
    """Read only upstream sources; every RPC and all writes target a deleted copy."""
    home=Path.home()
    data=Path(os.environ.get('XDG_DATA_HOME',''))
    if not data.is_absolute():data=home/'.local/share'
    candidates=[home/'.var/app/dev.stormix.deadlock-mod-manager/data/dev.stormix.deadlock-mod-manager/state.json',data/'dev.stormix.deadlock-mod-manager/state.json']
    source=next((p for p in candidates if p.exists()),None)
    if source is None:return dict(result='SKIP',reason='upstreamNotFound')
    report=dict(result='FAIL',reason='copyIncomplete')
    # /tmp is often a small RAM-backed tmpfs; real libraries need disk space (twice their size).
    with tempfile.TemporaryDirectory(prefix='deadlock-real-copy-',dir=os.environ.get('QUALIFICATION_REAL_TMP','/tmp')) as tmp:
        root=Path(tmp);child=None
        try:
            child=Qualification(helper,root,submission)
            child.rpc.timeout=600  # real libraries may contain multi-gigabyte VPK sets
            # Copy bytes only. Never preserve hard links to, or write through, sources.
            copied={}
            def copy(src,dst):
                if not src.is_file():return False
                with src.open('rb') as inp:
                    before=os.fstat(inp.fileno())
                    dst.parent.mkdir(parents=True,exist_ok=True)
                    digest=hashlib.sha256()
                    with dst.open('wb') as out:
                        while block:=inp.read(1024*1024):digest.update(block);out.write(block)
                    after=os.fstat(inp.fileno())
                assert (before.st_size,before.st_mtime_ns,before.st_ino)==(after.st_size,after.st_mtime_ns,after.st_ino),'sourceChanged'
                copied[src]=(before.st_size,before.st_mtime_ns,digest.hexdigest())
                return True
            copy(source,child.source)
            raw=json.loads(child.source.read_bytes())['local-config']
            config=json.loads(raw) if isinstance(raw,str) else raw
            state=config['state'];game=Path(state['gamePath'])
            assert game.is_absolute(),'gamePathUnavailable'
            assert copy(game/'game/citadel/gameinfo.gi',child.citadel/'gameinfo.gi'),'gameinfoUnavailable'
            safe=lambda n:isinstance(n,str) and n not in ('.','..') and '/' not in n and '\\' not in n and '\0' not in n
            folders={p.get('folderName') or '' for p in state['profiles'].values()}
            for folder in folders:
                if folder and not safe(folder):continue
                manifest=game/'game/citadel/addons'/folder/'.dmm.json'
                dest=child.addons/folder/'.dmm.json'
                if not copy(manifest,dest):continue
                m=json.loads(dest.read_bytes())
                for entry in m.get('mods',{}).values():
                    for key in ('currentVpks','disabledVpks'):
                        shard=entry.get('shard',1)
                        if not isinstance(shard,int) or not 1<=shard<=10:continue
                        base='addons'+(str(shard) if key=='currentVpks' and shard>1 else '')
                        for name in entry.get(key,[]):
                            if not safe(name) or not name.lower().endswith('.vpk'):continue
                            rel=Path('game/citadel')/base/folder/name
                            copy(game/rel,child.game/rel)
            # Sanitize only the copied config's game locator so no later code can
            # follow the real install path. Preserve all library records verbatim.
            state['gamePath']=str(child.game)
            if isinstance(raw,str):config=json.dumps(config)
            document=json.loads(child.source.read_bytes());document['local-config']=config
            child.source.write_text(json.dumps(document))
            baseline=fingerprint(child.game)
            preview=child.call('importPreview',statePath=str(child.source),gamePath=str(child.game))
            summary=preview.get('summary',preview)
            report.update(profiles=summary['profileCount'],mods=summary['modCount'],available=summary['availableModCount'],unavailable=summary['unavailableModCount'],warnings=[w['code'] for w in summary.get('warnings',[])])
            reasons={}
            reason_prefixes={
                'Unrecognized mod metadata':'unrecognizedMetadata',
                'Profile folderName':'unsafeProfileFolder',
                'Choose the game':'gamePathUnavailable',
                'No .dmm.json':'manifestMissing',
                'No exact remote-ID':'ownershipMissing',
                'Manifest filename':'unsafeAssociations',
                'VPK family':'invalidVpkFamily',
                'Multiple manifest entries':'ambiguousOwnership',
                'A recognized':'payloadUnavailable',
            }
            for item in summary.get('unavailableMods',[]):
                reason=item.get('reason','')
                code=next((v for k,v in reason_prefixes.items() if reason.startswith(k)),'unavailablePayload')
                reasons[code]=reasons.get(code,0)+1
            report['reasons']=reasons
            child.mutate('importCommit',token=preview['token'])
            doc=child.document()
            profiles=doc['profiles'];mods=[m for p in profiles.values() for m in p['mods']]
            warnings=preview.get('summary',preview).get('warnings',[])
            report['warnings']=[w['code'] for w in warnings]
            pid,mod=next((pid,m) for pid,p in profiles.items() for m in p['mods'] if m['available'])
            mid=mod['source']['id']
            child.mutate('setEnabled',profileId=pid,modId=mid,enabled=not mod['enabled'])
            snap=child.call('snapshot',profileId=pid)
            assert next(m for m in snap['mods'] if m['id']==mid)['enabled']==(not mod['enabled'])
            # Exercise the selected payload even when the original row was enabled.
            child.mutate('setEnabled',profileId=pid,modId=mid,enabled=True)
            plan=child.call('applyPreview',profileId=pid,gamePath=str(child.game))
            child.mutate('apply',token=plan['token']);child.restore()
            assert fingerprint(child.game)==baseline,'restoreMismatch'
            for src,(size,mtime,digest) in copied.items():
                with src.open('rb') as inp:
                    st=os.fstat(inp.fileno());actual=hashlib.file_digest(inp,'sha256').hexdigest()
                assert (st.st_size,st.st_mtime_ns,actual)==(size,mtime,digest),'sourceChanged'
            report.update(result='PASS',reason='verifiedCopyAndExactRestore')
        except Exception as e:
            # Real titles, paths, state and RPC errors must never escape this root.
            code=str(e) if str(e) in ('restoreMismatch','sourceChanged','gamePathUnavailable','gameinfoUnavailable') else type(e).__name__
            if isinstance(e,AssertionError) and e.args and isinstance(e.args[0],dict):
                code=e.args[0].get('error',{}).get('data',{}).get('kind','AssertionError')
            report.update(result='FAIL',reason=code)
        finally:
            if child:child.rpc.close()
    return report


def freeze_binaries(helper, root):
    binaries=root/'executables';binaries.mkdir()
    for name in ('modlock-helper','modlock-network'):
        shutil.copy2(helper.resolve().parent/name,binaries/name)
    return binaries/'modlock-helper'

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--helper',type=Path,required=True)
    p.add_argument('--network',action='store_true',help='enable ONE real GameBanana submission')
    p.add_argument('--real-library',action='store_true',help='read upstream discovery path and qualify a minimal temporary copy; delete it afterward')
    p.add_argument('--submission',default='714763')
    a=p.parse_args()
    assert a.helper.is_file()
    root=Path(tempfile.mkdtemp(prefix='deadlock-qualification-',dir=os.environ.get('TMPDIR')))
    print('Evidence root:',root,flush=True)
    # Freeze sibling executables so rebuilds in a shared checkout cannot change
    # the protocol implementation between helper restarts within one run.
    helper=freeze_binaries(a.helper,root)
    identity=dict(source=str(a.helper.resolve()),sha256=hashlib.sha256(helper.read_bytes()).hexdigest())
    (root/'binary.json').write_text(json.dumps(identity,indent=2))
    q=Qualification(helper,root,a.submission)
    try:
        q.step('1 disposable fixture',q.fixture)
        q.step('2 upstream import and profiles',q.upstream)
        q.step('3 Apply / exact Restore',q.disk)
        q.step('3 opaque prior bytes',lambda:prior_bytes_case(helper,root,a.submission))
        for kind in ('vpk','folder','zip','7z','rar'):
            if (kind=='7z' and not shutil.which('7z')) or (kind in ('7z','rar') and not all(shutil.which(x) for x in ('lsar','unar'))):
                q.results.append(('4 local '+kind,'SKIP','archive tools unavailable')); continue
            if kind=='rar':
                def isolated_rar():
                    childroot=root/'rar-scenario';childroot.mkdir()
                    child=Qualification(helper,childroot,a.submission)
                    try:
                        child.fixture();child.upstream()
                        return child.local('rar')
                    finally:child.rpc.close()
                q.step('4 local rar',isolated_rar)
            else:q.step('4 local '+kind,lambda k=kind:q.local(k))
        q.step('2 options and reorder',q.selection_order)
        if a.network:
            q.step('5 remote install',q.remote)
            if q.results[-1][1]=='PASS':q.step('5 updates / Reinstall',q.reinstall)
            else:q.results.append(('5 updates / Reinstall','BLOCKED','remote install did not complete'))
        else:q.results.append(('5 remote install','SKIP','pass --network'))
        if a.network:
            def live_link():
                childroot=root/'link-scenario';childroot.mkdir()
                child=Qualification(helper,childroot,a.submission)
                try:return child.link()
                finally:child.rpc.close()
            q.step('A live link-and-replace (Store2)',live_link)
        else:q.results.append(('A live link-and-replace','SKIP','pass --network'))
        if a.real_library:
            detail=real_library(helper,a.submission)
            q.results.append(('B real-library copy',detail.pop('result'),json.dumps(detail,sort_keys=True)))
        else:q.results.append(('B real-library copy','SKIP','pass --real-library'))
        q.step('6 launch preparation only',q.launch)
        q.rpc.close(); q.rpc=RPC(q.helper,q.root); q.snap()
        q.step('7 interrupted Apply recovery',q.interruption)
    finally:q.rpc.close()
    report=dict(helper=identity,root=str(root),results=q.results)
    (root/'results.json').write_text(json.dumps(report,indent=2))
    print('| Step | Result | Evidence |\n|---|---|---|')
    for row in q.results:print('| '+' | '.join(str(x).replace('|','/').replace('\n',' ') for x in row)+' |')
    return int(any(r[1]=='FAIL' for r in q.results))
if __name__=='__main__':raise SystemExit(main())
