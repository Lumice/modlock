#!/usr/bin/python3
"""Model-only local-import peer with polled history and bounded transport faults."""
import json
import os
import sys
import threading
import time
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup, apply_review

# These protocol doubles support the same explicit discovery entry point.
if sys.argv[1:]==['--data-root']:
    import os
    print(json.dumps({'root':str(Path(os.environ.get('XDG_DATA_HOME') or str(Path.home()/'.local/share'))/'modlock')}))
    sys.exit(0)

# Model-only protocol double: independent of the concurrently maintained UI peer.
mode = os.path.basename(sys.argv[0])
lock = threading.Lock()
local_job = None
local_polls = 0
trace_root = None

def local_candidates():
    return {'sources': [{'rootOrdinal': 0, 'path': '/synthetic/a space.vpk', 'kind': 'file'}],
      'candidates': [{'id': 'a'*64, 'rootOrdinal': 0, 'relativePath': 'complete_dir.vpk', 'optionName': 'Complete family', 'available': True, 'unavailableReason': None,
        'members': [{'role': 'directory', 'archiveIndex': None, 'sourcePath': '/synthetic/complete_dir.vpk', 'sourceName': 'complete_dir.vpk', 'sha256': 'b'*64, 'size': 64},
                    {'role': 'companion', 'archiveIndex': 0, 'sourcePath': '/synthetic/complete_000.vpk', 'sourceName': 'complete_000.vpk', 'sha256': 'c'*64, 'size': 12}]},
        {'id':'d'*64, 'rootOrdinal':0, 'relativePath':'missing_dir.vpk', 'optionName':'Missing sibling', 'available':False, 'unavailableReason':'Required companion missing_000.vpk is absent', 'members':[]}],
      'exclusions': [{'rootOrdinal':0,'relativePath':'archive.zip','reason':'Archive import is pending'}, {'rootOrdinal':0,'relativePath':'font.ttf','reason':'Font import is pending'}],
      'warnings': [], 'sourceArtwork': json.loads((trace_root/'artwork-fixture.json').read_text()) if trace_root is not None and (trace_root/'artwork-fixture.json').exists() else None,
      'defaultMetadata': {'name':'Local family','author':'Unknown','category':'Uncategorized','link':'','description':''}, 'defaultSelectedCandidateIds':['a'*64]}


def send(request_id, result):
    observe(result)
    line = json.dumps({'jsonrpc': '2.0', 'id': request_id, 'result': result}) + '\n'
    if 'boundary' in mode and isinstance(result,dict) and 'candidateSummary' in result: line=line[:-1]+' '*(32*1024*1024-len(line))+'\n'
    if 'local-oversized' in mode and isinstance(result,dict) and result.get('kind')=='localImport': line=line[:-1]+' '*49152+'\n'
    if trace_root is not None:
        with (trace_root/'trace.jsonl').open('a') as trace: trace.write(json.dumps({'event':'response','pid':os.getpid(),'id':request_id})+'\n')
    with lock:
        if mode == 'partial':
            sys.stdout.write(line[:12]); sys.stdout.flush(); time.sleep(.03)
            sys.stdout.write(line[12:]); sys.stdout.flush()
        else:
            sys.stdout.write(line); sys.stdout.flush()

def snapshot(profile):
    return {'protocolVersion': 1, 'sourcePath': '/synthetic/state.json', 'storeVersion': 33,
            'activeProfileId': 'default', 'selectedProfileId': profile,
            'profiles': [{'id': p, 'name': p, 'isActive': p == 'default', 'modCount': 0}
                         for p in ['default', 'slow', 'fast']], 'mods': [], 'warnings': []}

for line in sys.stdin:
    request = json.loads(line)
    request_id = request['id']
    if mode == 'timeout':
        continue
    if mode == 'exit':
        sys.exit(0)
    if mode.startswith('native-'):
        params = request.get('params', {})
        method = request['method']
        if mode.startswith('native-local'):
            if method != 'hello':
                root = Path(params['dataDir']); root.mkdir(parents=True, exist_ok=True); trace_root = root
                with (root / 'trace.jsonl').open('a') as trace: trace.write(json.dumps({'event':'request','id':request_id,'pid':os.getpid(),'method':method,'params':params})+'\n')
            if method.startswith('native.local.'):
                if method == 'native.local.prepare':
                    local_job = {'jobId':params['jobId'],'kind':'localImport','jobVersion':1,'profileId':params['profileId'], 'profileNameAtStart':params['profileId'],
                      'expectedRevision':params['expectedRevision'],'observedRevision':params['expectedRevision'],'sequence':'1','phase':'enumerating',
                      'filesDone':0,'filesTotal':None,'bytesDone':0,'bytesTotal':None,'canCancel':True,'operationId':None,'error':None}
                    local_polls=0
                    send(request_id, local_job); continue
                if method == 'native.local.status':
                    if not params.get('jobId'):
                        saved=root/'local-restarted.json'
                        if local_job is None and saved.exists(): local_job=json.loads(saved.read_text())
                        if local_job and local_job['phase']=='cancelRequested': local_job['phase']='cancelled'
                        if local_job and local_job['phase'] not in ('cancelled','succeeded','interrupted'):
                            local_polls += 1
                            local_job['phase'] = 'ready' if local_polls >= (12 if 'slow' in mode or 'held' in mode else 1) else 'hashing'
                            local_job['sequence'] = str(local_polls+1)
                            if 'status-error' in mode and local_polls == 1:
                                print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':-32020,'message':'local storage unavailable','data':{'kind':'localPollFailure'}}}),flush=True)
                                continue
                        send(request_id, [local_job] if local_job else []); continue
                    local_polls += 1
                    if 'status-error' in mode and local_polls == 1:
                        print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':-32020,'message':'local storage unavailable','data':{'kind':'localPollFailure'}}}),flush=True)
                        continue
                    if local_job['phase'] == 'cancelRequested': local_job['phase']='cancelled'
                    if local_job['phase'] not in ('cancelled','succeeded'):
                        local_job['phase'] = 'ready' if local_polls >= (12 if 'slow' in mode or 'held' in mode else 1) else 'hashing'
                    local_job['sequence']=str(local_polls+1)
                    response = dict(local_job)
                    if "old-job" in mode: response["jobId"]="0"*32
                    if params.get('includeCandidates'):
                        response['candidateSummary']=local_candidates()
                        if 'crossing' in mode: time.sleep(.35)
                    send(request_id,response); continue
                if method == 'native.local.review':
                    response=local_candidates()
                    response.pop('defaultMetadata'); response.pop('defaultSelectedCandidateIds')
                    response |= {'jobId':local_job['jobId'],'token':'review-token','modId':'local-'+'1'*32,'profileId':local_job['profileId'],
                      'profileNameAtStart':local_job['profileNameAtStart'],'expectedRevision':local_job['expectedRevision'], 'metadata':params['metadata'],
                      'selectedCandidateIds':params['candidateIds'],'selectedPackageCount':len(params['candidateIds']),'storedPackageCount':1,'uniqueCopyBytes':76,'logicalPackageBytes':76, 'inactiveOptionCount':0, 'retainedPackageCount':1, 'retainedLogicalPackageBytes':76, 'selectedLogicalPackageBytes':76, 'uniquePayloadBytes':76, 'retainedArchiveBytes':0, 'archives':None}
                    if response['sourceArtwork']:
                        response['uniqueCopyBytes'] += response['sourceArtwork']['size']
                    if 'crossing' in mode: time.sleep(.35)
                    send(request_id,response); continue
                if method == 'native.local.cancel':
                    local_job['phase']='cancelRequested' if 'slow' in mode else 'cancelled'; local_job['canCancel']=False; local_polls+=1; local_job['sequence']=str(local_polls+2)
                    send(request_id, local_job); continue
                if method == 'native.local.dismiss':
                    send(request_id, {'jobId':params['jobId'],'dismissed':True}); local_job=None; continue
        if method == 'hello':
            send(request_id, paired_hello())
            continue
        if mode == 'native-family-legacy-import' and method == 'library.snapshot':
            send(request_id, snapshot('default')); continue
        handled, value = paired_startup(method, params)
        if handled:
            send(request_id, value); continue
        assert params['dataDir'].endswith('/native') or (mode.startswith('native-local') and params['dataDir'].endswith('/modlock')), 'native routing lost isolation'
        result = snapshot('default') | {'sourceKind': 'native', 'storeVersion': 1, 'revision': '1',
                'pendingChanges': True, 'gamePath': '/synthetic/game', 'appliedProfileId': None,
                'canRestore': False, 'recoveryRequired': False,
                'mods': [{'id': 'image-mod', 'remoteId': '123', 'title': 'Artwork', 'author': 'Synthetic',
                          'hero': 'Ivy', 'category': 'Skins', 'status': 'installed', 'enabled': True,
                          'appliedEnabled': False, 'available': True, 'unavailableReason': None,
                          'installOrder': None, 'installedVpks': [], 'missingVpks': []}]}
        if mode.startswith('native-local-home-'):
            case = mode.removeprefix('native-local-home-')
            result['activeProfileId'] = 'default'
            result['selectedProfileId'] = params.get('profileId', 'default')
            result['appliedProfileId'] = None if case.endswith('unapplied') else 'default'
            result['pendingChanges'] = case.endswith('changed')
            result['pendingStatus'] = 'pending' if result['pendingChanges'] else 'current'
            result['pendingReason'] = None
            if case.endswith('choices'):
                result['pendingStatus'] = 'selectionRequired'
            result['mods'] = []
        if mode == 'native-local-hero':
            result['mods'][0]['hero'] = 'General'
            result['mods'][0]['heroDetectionEligible'] = True
            if method == 'native.hero.detect':
                assert params['profileId'] == 'default' and params['expectedRevision'] == '1'
                send(request_id, {'modId':params['modId'], 'revision':'1', 'fingerprint':'payload-sha256', 'heroDisplay':'Grey Talon'})
                continue
        if mode == 'native-local-external-gameinfo':
            result['pendingStatus'] = 'unknown'
            result['pendingReason'] = 'externalGameinfoChange'
        if mode.startswith('native-local'):
            result['selectedProfileId'] = params.get('profileId','default')
            finished = Path(params['dataDir']) / 'local-finished.json'
            if finished.exists(): result['revision']='2'
            if 'batch-unknown' in mode and method == 'native.snapshot' and finished.exists(): result=json.loads(finished.read_text())['result']
            if method == 'native.snapshot' and finished.exists() and 'fresh' in mode: time.sleep(.5)
            if method == 'native.snapshot' and params.get('profileId') == 'slow' and 'held' in mode: time.sleep(.8)
            if method == 'native.local.commit':
                assert params['profileId']==local_job['profileId'] and params['expectedRevision']==local_job['expectedRevision']
                time.sleep(.25)
                finished.write_text(json.dumps({'operationId':params['operationId']}))
                local_job['phase']='succeeded'; local_job['canCancel']=False; result['revision']='2'
                if 'unknown' in mode:
                    print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':-32603,'message':'postcommit job storage failed','data':{'kind':'localJobOutcomeUnknown'}}}), flush=True)
                else: send(request_id,result)
                continue
            if method == 'native.operation':
                assert params['operationId']==json.loads(finished.read_text())['operationId']
                send(request_id,{'operationId':params['operationId'],'status':'succeeded','result':json.loads(finished.read_text()).get('result',result)}); continue
            if method == 'native.applyPreview':
                time.sleep(5.4)
                if mode.startswith('native-local-home-'):
                    send(request_id, apply_review(params['profileId']))
                    continue
        if method == 'native.deleteMods':
            assert params['profileId']=='default' and params['modIds']==['image-mod']
            assert params['expectedRevision']=='1' and params['operationId']
            result['revision']='2';result['mods']=[];result['bulkResult']={'changed':1,'skipped':[]}
            send(request_id,result);continue
        if method in ('native.setModsEnabled', 'native.removeModsFromProfile'):
            assert params['profileId'] == 'default' and params['modIds'] == ['image-mod','missing']
            assert params['expectedRevision'] == '1' and params['operationId']
            time.sleep(.15)
            result['revision']='2'
            result['bulkResult']={'changed':1,'skipped':[{'id':'missing','reason':'unknownMod'}]}
            if method == 'native.setModsEnabled': result['mods'][0]['enabled']=params['enabled']
            else: result['mods']=[]
            if 'batch-unknown' in mode:
                finished.write_text(json.dumps({'operationId':params['operationId'],'result':result}))
                print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':-32603,'message':'Reply lost','data':{'kind':'outcomeUnknown'}}}),flush=True)
            else: send(request_id,result)
            continue
        if method == 'native.setEnabled':
            assert params['profileId'] == 'default' and params['modId'] == 'image-mod'
            assert params['expectedRevision'] == '1' and not params['enabled']
            if mode == 'native-reject':
                print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32020, 'message': 'Changed elsewhere', 'data': {'kind': 'staleRevision'}}}), flush=True)
                continue
            time.sleep(5.4)
            result['revision'] = '2'; result['mods'][0]['enabled'] = False
        send(request_id, result)
        continue
if mode.startswith('native-local') and local_job:
    local_job['sequence']=str(int(local_job['sequence'])+1)
    if local_job['phase'] in ('enumerating','inspecting','hashing','ready','cancelRequested'):
        local_job['phase']='interrupted'; local_job['canCancel']=False
    (root/'local-restarted.json').write_text(json.dumps(local_job))
    with (root/'trace.jsonl').open('a') as trace: trace.write(json.dumps({'pid':os.getpid(),'method':'EOF'})+'\n')
