#!/usr/bin/python3
"""Synthetic JSON-RPC peer for transport failures; never opens a state path."""
import json
import os
import sys
import threading
import time
import tempfile
import subprocess
import hashlib
import struct
import zlib
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup, apply_review, STARTUP_METHODS

# These protocol doubles support the same explicit discovery entry point.
if sys.argv[1:]==['--data-root']:
    import os
    print(json.dumps({'root':str(Path(os.environ.get('XDG_DATA_HOME') or str(Path.home()/'.local/share'))/'modlock')}))
    sys.exit(0)

if len(sys.argv) > 1 and sys.argv[1] == '--run-local-ui':
    with tempfile.TemporaryDirectory(prefix='local-import-ui-') as temporary:
        captures = Path(os.environ.get('LOCAL_UI_CAPTURE_DIR', temporary + '/captures'))
        for scenario, peer_mode in [('local-success','native-local'), ('local-cancel','native-local-slow'), ('local-unknown','native-local-unknown'), ('local-artwork','native-local'), ('local-artwork-broken','native-local')]:
            root = Path(temporary) / scenario; root.mkdir(); (root/'native').mkdir()
            peer = root/peer_mode; peer.symlink_to(Path(__file__).resolve())
            capture = captures/scenario; capture.mkdir(parents=True, exist_ok=True)
            if scenario.startswith('local-artwork'):
                def chunk(kind, data): return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data)&0xffffffff)
                png = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',32,24,8,2,0,0,0))+chunk(b'IDAT',zlib.compress((b'\0'+bytes([210,45,65])*32)*24))+chunk(b'IEND',b'')
                artwork = root/'preview.png'; artwork.write_bytes(png if not scenario.endswith('broken') else b'\x89PNG\r\n\x1a\ninvalid')
                st=artwork.stat(); dto={'path':str(artwork),'sha256':hashlib.sha256(artwork.read_bytes()).hexdigest(),'size':st.st_size,'device':st.st_dev,'inode':st.st_ino,'modifiedSeconds':st.st_mtime_ns//1000000000,'modifiedNanos':st.st_mtime_ns%1000000000,'format':'png'}
                (root/'native/artwork-fixture.json').write_text(json.dumps(dto))
            env=os.environ|{'QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software','QT_FORCE_STDERR_LOGGING':'1','XDG_CONFIG_HOME':str(root/'config'),'UI_NATIVE_CAPTURES':str(capture)}
            result=subprocess.run([sys.argv[2],'--helper',str(peer),'--data-dir',str(root/'native'),'--check-ui',scenario],env=env,capture_output=True,text=True,timeout=20)
            assert result.returncode==0 and 'PASS UI' in result.stdout, (scenario,result.returncode,result.stdout,result.stderr)
            print('PASS',scenario,flush=True)
    sys.exit(0)

mode = os.path.basename(sys.argv[0])
lock = threading.Lock()
local_job = None
local_polls = 0
trace_root = None
remote_job = None
remote_form = None
remote_committed = False
remote_snapshot = None
remote_added = 0
remote_updated_mods = set()
remote_revision = 1
remote_review_failed = False
remote_status_polls = 0
remote_saved_file = '12'
page_linked = os.environ.get('INSTALL_LINK_CASE','').startswith(('page-linked','browse-linked')) or os.environ.get('INSTALL_LINK_CASE') in ('legacy-link','known-receipt','batch-auto')
if page_linked:
    remote_form={'name':'Linked local mod','author':'Kept author','category':'Skins','description':'Kept notes'}
    submission={'provider':'gamebanana','submissionType':'mod','submissionId':'42'}

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
    if os.environ.get('INSTALL_LINK_TRACE'):
        with open(os.environ['INSTALL_LINK_TRACE'],'a') as trace: trace.write(json.dumps({'response':result})+'\n')
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
    if os.environ.get('INSTALL_LINK_TRACE'):
        with open(os.environ['INSTALL_LINK_TRACE'],'a') as trace: trace.write(json.dumps(request)+'\n')
    request_id = request['id']
    if mode == 'timeout':
        continue
    if mode == 'exit':
        sys.exit(0)
    if mode.startswith('native-'):
        params = request.get('params', {})
        method = request['method']
        if mode.startswith('native-storage-') and method.startswith('native.storage.'):
            if method == 'native.storage.status':
                revision = '1'
                if mode == 'native-storage-stall':
                    data = Path(params['dataDir'])
                    assert data.is_absolute() and data.name == 'native' and data.parent == Path(sys.argv[0]).parent
                    revision = '2' if (data / 'revision-advanced').exists() else '1'
                    with (data / 'status-pids').open('a') as trace:
                        trace.write(str(os.getpid()) + '\n')
                    deadline = time.monotonic() + 60
                    while (data / 'hold-status').exists() and time.monotonic() < deadline:
                        time.sleep(.05)
                time.sleep(.05)
                send(request_id, {'revision':revision,'resume':True,'canCancel':mode.endswith('pre'),'current':{'path':'/synthetic/source'}})
            else:
                print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':'storageChanged','message':'Synthetic interrupted move'}}),flush=True)
            continue
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
        if mode == 'native-links':
            assert method in ('hello','native.snapshot','native.operation','native.modUpdates.get') or method.startswith('native.remote.') or method in STARTUP_METHODS or (method == 'native.applyPreview' and remote_committed), 'Install link attempted an unreviewed action: '+method
        if mode == 'native-links' and method == 'native.applyPreview':
            # This UI fixture has no game files. A complete helper can validate
            # the reviewed request and refuse that nonexistent game normally.
            assert remote_committed and params['deploymentLayout']=='shards-v1' and params['profileId']=='default' and params['gamePath']=='/synthetic/game'
            print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':-32020,'message':'No game files exist in this synthetic fixture.','data':{'kind':'invalidGamePath'}}}),flush=True)
            continue
        if mode == 'native-links' and page_linked and method=='native.modUpdates.get':
            if os.environ.get('INSTALL_LINK_CASE')=='batch-auto':
                send(request_id,{'profileId':params['profileId'],'revision':str(remote_revision),'lastUpdateCheck':int(time.time()),'mods':[{'modId':mod,'submission':submission,'status':'available','fileId':'12','createdAt':123,'versionUnknown':False,'recordDigest':('8' if mod in remote_updated_mods else '9')*64} for mod in ['page-linked','unrelated']]})
                continue
            send(request_id,{'profileId':params['profileId'],'revision':str(remote_revision),'lastUpdateCheck':int(time.time()),'mods':[{'modId':'page-linked','submission':submission,'recordDigest':('8' if remote_added else '9')*64,'status':'current' if remote_added else 'unsure','versionUnknown':not bool(remote_added), **({'status':'available','fileId':'999','createdAt':123,'versionUnknown':False} if os.environ.get('INSTALL_LINK_CASE','').startswith('page-linked-bulk') else {})}]})
            continue
        if mode == 'native-links' and method.startswith('native.remote.'):
            base = {'version':1, 'root':params['dataDir'], 'sessionId':params['sessionId']}
            if method == 'native.remote.open': send(request_id, base | {'phase':'ready','error':None})
            elif method == 'native.remote.close': send(request_id, base | {'phase':'closed','error':None})
            elif method == 'native.remote.replaceList': send(request_id,base | {'jobs':[{k:remote_job[k] for k in ('jobId','submission','profileId','state','sequence','cleanupPending','purpose','modId','targetDigest')}] if remote_job and remote_job.get('purpose') else []})
            elif method == 'native.remote.list': send(request_id, base | {'jobs':[{k:remote_job[k] for k in ('jobId','submission','profileId','state','sequence','cleanupPending')}] if remote_job and not remote_job.get('purpose') else []})
            elif method == 'native.remote.resolve':
                assert params['submission'] == {'provider':'gamebanana','submissionType':'mod','submissionId':'42'}
                send(request_id, base | {'requestKey':params['requestKey'],'state':'pending'})
            elif method == 'native.remote.resolveStatus':
                submission = {'provider':'gamebanana','submissionType':'mod','submissionId':'42'}
                consent = {'fileId':'12','originalName':'variant.zip','declaredSize':'1024','createdAt':'1730000000','md5':None,'list':'current','archiveFlag':False}
                fingerprint = hashlib.sha256(json.dumps(['remoteConsent/v1',submission,'Variants',consent],sort_keys=True,separators=(',',':')).encode()).hexdigest()
                row = {k:v for k,v in consent.items() if k != 'fileId'} | {'fileKey':{'submission':submission,'fileId':'12'},'submissionName':'Variants','candidateFingerprint':fingerprint,'description':'reze_human'}
                rows=[row]
                if os.environ.get('INSTALL_LINK_CASE','').startswith('multi') or os.environ.get('INSTALL_LINK_CASE')=='page-linked-multi' or os.environ.get('INSTALL_LINK_AUTO')!='1':
                    consent2=consent | {'fileId':'13','originalName':'second.zip'}
                    if os.environ.get('INSTALL_LINK_CASE')=='multi-changed' and remote_added: consent2['declaredSize']='2048'
                    fingerprint2=hashlib.sha256(json.dumps(['remoteConsent/v1',submission,'Variants',consent2],sort_keys=True,separators=(',',':')).encode()).hexdigest()
                    rows.append({k:v for k,v in consent2.items() if k != 'fileId'} | {'fileKey':{'submission':submission,'fileId':'13'},'submissionName':'Variants','candidateFingerprint':fingerprint2,'description':'A second look' if os.environ.get('INSTALL_LINK_CASE','').startswith('multi') else 'reze_human'})
                send(request_id, base | {'requestKey':params['requestKey'],'phase':'ready','page':0,'hasMore':False,'selectionToken':'a'*32,'candidates':rows,'error':None})
            elif method in ('native.remote.start','native.remote.replaceStart'):
                if os.environ.get('INSTALL_LINK_CASE')=='browse-linked-refused':
                    print(json.dumps({'jsonrpc':'2.0','id':request_id,'error':{'code':'eligibilityUnknown','message':'Restore the missing original file, then try Update again.'}}),flush=True)
                    continue
                if page_linked and not remote_added and not os.environ.get('INSTALL_LINK_CASE','').startswith('browse-linked'):
                    assert params.get('purpose')==os.environ.get('INSTALL_LINK_PURPOSE','update'), 'Unexpected replacement purpose'
                if remote_job is not None and params['jobId'] == remote_job['jobId']:
                    send(request_id,base | {'jobId':params['jobId'],'state':'pending'}); continue
                remote_snapshot=None;remote_status_polls=0
                remote_job = base | {'jobId':params['jobId'],'submission':submission,'profileId':params['profileId'],'profileNameAtStart':'Default','initialRevision':params['expectedRevision'],'reviewRevision':None,'sequence':'1','state':'downloaded','progress':None,'cohort':{'fingerprint':'b'*64,'complete':True,'members':[{'fileId':params['fileId'],'originalName':'variant.zip','state':'sealed','durableOffset':'1024','total':'1024','receipt':None}]},'prepareKey':None,'review':None,'operationId':None,'lastControl':None,'error':None,'cleanupPending':False}
                if method.endswith('replaceStart'): remote_job |= {'purpose':params['purpose'],'modId':params['modId'],'targetDigest':params['targetDigest']}
                if os.environ.get('INSTALL_LINK_CASE')=='bubble' or page_linked: remote_job |= {'state':'transferring','progress':{'epoch':'f'*32,'volatileSequence':'1','bytesWritten':'12900000','durableOffset':'12000000','total':'17100000'}}
                send(request_id, base | {'jobId':params['jobId'],'state':'pending'})
            elif method in ('native.remote.status','native.remote.replaceStatus'):
                remote_status_polls+=1
                if remote_job['state']=='transferring' and remote_status_polls>=3: remote_job |= {'state':'downloaded','sequence':'2','progress':None}
                if method.endswith('replaceStatus'):
                    status={k:v for k,v in remote_job.items() if k not in ['purpose','modId','targetDigest']}
                    review=status.pop('review');status['review']=None
                    send(request_id,base | {'purpose':remote_job['purpose'],'modId':remote_job['modId'],'targetDigest':remote_job['targetDigest'],'status':status,'review':review})
                else: send(request_id,remote_job)
            elif method == 'native.remote.replaceTarget':
                send(request_id,base | {'requestKey':params['requestKey'],'phase':'ready','target':{'profileId':params['profileId'],'modId':params['modId'],'revision':params['expectedRevision'],'recordDigest':('8' if (params['modId'] in remote_updated_mods if os.environ.get('INSTALL_LINK_CASE')=='batch-auto' else remote_added>(0 if page_linked else 1)) else '9')*64,'receipt':None if page_linked and not remote_added and os.environ.get('INSTALL_LINK_CASE') not in ('known-receipt','batch-auto') else {'version':1,'submission':submission,'rootFileId':remote_saved_file,'cohortFingerprint':'b'*64},'enabled':page_linked,'previewHidden':False,'available':True,'installOrder':1,'metadata':remote_form,'optionCount':str(max(1,remote_added) if page_linked else remote_added),'selectedOptionIds':['d'*32] if page_linked and remote_added else ['c'*32]},'skip':{'state':'none','entry':None},'error':None})
            elif method == 'native.remote.prepare':
                assert params['acknowledgeRebase'] is True
                if os.environ.get('INSTALL_LINK_CASE')=='rebase': assert params['expectedRevision']=='2'
                remote_job |= {'prepareKey':params['prepareKey'],'reviewRevision':params['expectedRevision'],'state':'ready','sequence':str(int(remote_job['sequence'])+1)}
                send(request_id,base | {'jobId':params['jobId'],'prepareKey':params['prepareKey'],'state':'pending'})
            elif method == 'native.remote.candidates':
                count=int(os.environ.get('INSTALL_LINK_CANDIDATES','1'))
                candidates=[{'candidateId':c*64,'optionId':c*32,'optionName':'Variant '+str(i+1),'sourceLabel':'variant.zip','available':True,'unavailableReason':None,'memberCount':'1','logicalBytes':'2048'} for i,c in enumerate(('de' if remote_job.get('purpose') else 'cd')[:count])]
                paged=os.environ.get('INSTALL_LINK_CASE')=='paged'
                page=params['page']
                send(request_id,base | {'jobId':params['jobId'],'prepareKey':params['prepareKey'],'page':page,'hasMore':paged and page==0,'candidates':candidates[page:page+1] if paged else candidates})
            elif method in ('native.remote.review','native.remote.replaceReview'):
                if method.endswith('.review'): remote_form=params['metadata']
                ids=params['candidateIds']; options=[v[:32] for v in ids]
                digest=hashlib.sha256(json.dumps(['remoteSelection/v1',options,remote_form],sort_keys=True,separators=(',',':'),ensure_ascii=False).encode()).hexdigest()
                remote_job |= {'state':'reviewing','sequence':str(int(remote_job['sequence'])+1),'review':{'requestKey':params['requestKey'],'phase':'ready','reviewToken':'e'*32,'selectionDigest':digest,'modId':'installed-variant','summary':{'selectedCandidateIds':ids,'selectedOptionIds':options,'readyCount':str(count),'selectedCount':str(len(ids)),'retainedCount':str(count-len(ids)),'uniquePayloadBytes':str(count*2048),'retainedLogicalBytes':str((count-len(ids))*2048),'sourceBytes':'1024','diagnosticCount':'0'},'error':None}}
                if method.endswith('replaceReview'):
                    digest=hashlib.sha256(json.dumps(['remoteReplaceSelection/v1',remote_job['purpose'],params['profileId'],params['modId'],params['targetDigest'],remote_job['initialRevision'],params['expectedRevision'],remote_job['cohort']['members'][0]['fileId']=='13' and fingerprint2 or fingerprint,remote_job['cohort']['fingerprint'],ids,options,'8'*64],sort_keys=True,separators=(',',':')).encode()).hexdigest()
                    remote_job['review'].pop('modId');remote_job['review'] |= {'afterRecordDigest':'8'*64,'selectionDigest':digest}
                if os.environ.get('INSTALL_LINK_CASE')=='review-failure' or (os.environ.get('INSTALL_LINK_CASE')=='bubble-error' and (not remote_review_failed or os.environ.get('INSTALL_LINK_REPEAT_ERROR'))):
                    remote_review_failed=True
                    remote_job['review']={'requestKey':params['requestKey'],'phase':'failed','reviewToken':None,'selectionDigest':None,'modId':None,'summary':None,'error':{'kind':'invalidSelection','message':'Preparation failed for this candidate.' if os.environ.get('INSTALL_LINK_CASE')=='bubble-error' else 'Choose another variant.','retryAfterSeconds':None}}
                send(request_id,base | {'jobId':params['jobId'],'requestKey':params['requestKey'],'state':'pending'})
            elif method == 'native.remote.prepareRelease':
                remote_job |= {'prepareKey':None,'review':None,'state':'downloaded','sequence':str(int(remote_job['sequence'])+1)}
                send(request_id,base | {'jobId':params['jobId'],'prepareKey':params['prepareKey'],'state':'released'})
            elif method in ('native.remote.commit','native.remote.replaceCommit'):
                assert remote_form
                if remote_snapshot is not None:
                    send(request_id,remote_snapshot); continue
                remote_committed=True
                if remote_job.get('purpose')!='append': remote_saved_file=remote_job['cohort']['members'][0]['fileId']
                if remote_job['state']!='installed': remote_added+=count
                if remote_job.get('modId'): remote_updated_mods.add(remote_job['modId'])
                remote_revision=int(remote_job['reviewRevision'])+1
                remote_job |= {'state':'installed','sequence':str(int(remote_job['sequence'])+1),'operationId':params['operationId']}
                send(request_id,base | {'jobId':params['jobId'],'operationId':params['operationId'],'state':'pending'})
            elif method == 'native.remote.resolveRelease': send(request_id,base | {'requestKey':params['requestKey'],'released':True})
            else: raise AssertionError('Unexpected remote action: '+method)
            continue
        if mode == 'native-performance' and method == 'native.performance.get':
            send(request_id, {'selection':'balanced','detected':{'state':'stock'},'presets':[{'id':'balanced','name':'Balanced','author':'Community','description':'Reduces some visual effects while keeping the game readable.','entryCount':2,'categoryGroups':[{'id':'visual','label':'Visuals'}],'entries':[{'key':'ConVars/effects','label':'Effects','from':'high','to':'medium','description':'Visual effects quality','category':'visual','allowed':True},{'key':'Engine/blocked','label':'Engine section','from':None,'to':'1','description':'','category':'other','allowed':False}]}]}); continue
        if method == 'hello':
            result = paired_hello()
            if mode == 'native-read-only': result['readOnly'] = True
            send(request_id, result)
            continue
        handled, value = paired_startup(method, params, native=mode!='native-family-legacy-import')
        if handled:
            send(request_id, value); continue
        if mode == 'native-family-legacy-import' and method == 'library.snapshot':
            send(request_id, snapshot('default')); continue
        if mode == 'native-slow-preview':
            if method == 'library.snapshot':
                send(request_id, snapshot('default')); continue
            assert params['dataDir'].endswith('/native')
            if method == 'native.importPreview':
                time.sleep(5.4)
                send(request_id, {'token': 'review-token', 'profileCount': 0, 'modCount': 0, 'availableModCount': 0, 'unavailableModCount': 0, 'totalBytes': 0, 'warnings': [], 'gamePath': '/synthetic/game', 'profiles': [], 'unavailableMods': [], 'summariesTruncated': False})
                continue
        assert params['dataDir'].endswith('/native') or (mode.startswith('native-local') and params['dataDir'].endswith('/modlock')), 'native routing lost isolation'
        result = snapshot('default') | {'sourceKind': 'native', 'storeVersion': 1, 'revision': '1',
                'pendingChanges': True, 'gamePath': '/synthetic/game', 'appliedProfileId': None,
                'canRestore': False, 'recoveryRequired': False,
                'mods': [{'id': 'image-mod', 'remoteId': '123', 'title': 'Artwork', 'author': 'Synthetic',
                          'hero': 'Ivy', 'category': 'Skins', 'status': 'installed', 'enabled': True,
                          'appliedEnabled': False, 'available': True, 'unavailableReason': None,
                          'installOrder': None, 'installedVpks': [], 'missingVpks': []}]}
        if os.environ.get('INSTALL_LINK_CASE')=='page-linked-bulk-inactive':
            result['activeProfileId']='fast'
            for profile in result['profiles']: profile['isActive']=profile['id']=='fast'
        if mode == 'native-links':
            result |= {'storeVersion':4,'packageVersion':2,'fonts':{'version':1,'installed':None,'previous':None}}
            if page_linked:
                browse=os.environ.get('INSTALL_LINK_CASE','').startswith('browse-linked')
                appending=browse and remote_job and remote_job.get('purpose')=='append'
                result['mods']=[dict(result['mods'][0],id='page-linked',title=remote_form['name'],author=remote_form['author'],category=remote_form['category'],description=remote_form['description'],enabled=True,remoteSubmission=submission,link='https://gamebanana.com/mods/42',linkEligible=os.environ.get('INSTALL_LINK_CASE')=='legacy-link',linkReason='',optionCount=(2+remote_added if browse and (not remote_added or appending) else max(1,remote_added)),selectedOptionIds=['c'*32] if not remote_added or appending else ['d'*32])]
            if os.environ.get('INSTALL_LINK_CASE','').startswith('page-linked-bulk') or os.environ.get('INSTALL_LINK_CASE')=='batch-auto': result['mods'].append(dict(result['mods'][0],id='unrelated',remoteId='other',title='Other mod'))
            if remote_job and os.environ.get('INSTALL_LINK_CASE')=='rebase': result['revision']='2'
            if remote_committed:
                result['revision']=str(remote_revision)
                if not page_linked: result['mods'].append(dict(result['mods'][0],id='installed-variant',title=remote_form['name'],author=remote_form['author'],category=remote_form['category'],description=remote_form['description'],enabled=False,remoteSubmission=submission,optionCount=remote_added,selectedOptionIds=['c'*32]))
                if method == 'native.operation':
                    remote_snapshot=result
                    send(request_id,{'operationId':params['operationId'],'status':'succeeded','result':result}); continue
        if mode.startswith('native-local'):
            result['selectedProfileId'] = params.get('profileId','default')
            finished = Path(params['dataDir']) / 'local-finished.json'
            if finished.exists(): result['revision']='2'
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
                send(request_id,{'operationId':params['operationId'],'status':'succeeded','result':result}); continue
            if method == 'native.applyPreview': time.sleep(5.4)
        if mode == 'native-invalid-bool': result['pendingChanges'] = 'true'
        if mode == 'native-invalid-revision': result['revision'] = '18446744073709551616'
        if mode == 'native-storage-stall' and (Path(params['dataDir']) / 'revision-advanced').exists():
            result['revision'] = '2'
        if mode in ('native-family-review', 'native-family-legacy-import', 'native-family-legacy-apply') and method in ('native.importPreview', 'native.applyPreview'):
            send(request_id, json.loads((Path(params['dataDir']) / 'review.json').read_text()))
            continue
        if mode == 'native-family-unknown':
            root = Path(params['dataDir']); root.mkdir(exist_ok=True)
            result['storeVersion'] = 2
            finished = root / 'finished.json'
            if finished.exists():
                result['revision'] = '2'; result['mods'][0]['enabled'] = False
            if method == 'native.setEnabled':
                with (root / 'dispatches').open('a') as trace: trace.write('write\n')
                finished.write_text(json.dumps({'operationId': params['operationId']}))
                result['storeVersion'] = 3
                send(request_id, result); time.sleep(.1); sys.exit(0)
            if method == 'native.operation':
                assert params['operationId'] == json.loads(finished.read_text())['operationId']
                (root / 'outcome-read').touch()
                if not (root / 'allow-outcome').exists(): result['storeVersion'] = 3
                result['revision'] = '1'  # Historical data must not replace the latest snapshot.
                send(request_id, {'operationId': params['operationId'], 'status': 'succeeded', 'result': result})
                continue
        if mode.startswith('native-edit-') or mode.startswith('native-restore-lost'):
            # The first write loses its reply; '-stored' saves it first. Later
            # requests behave like a healthy helper that refuses stale revisions.
            root = Path(params['dataDir']); root.mkdir(exist_ok=True)
            with (root / 'trace').open('a') as trace: trace.write(method + '\n')
            saved = root / 'saved.json'
            if mode.startswith('native-restore-lost'): result['canRestore'] = True
            stored = json.loads(saved.read_text()) if saved.exists() else {'revision': 1, 'enabled': True, 'operations': []}
            result['revision'] = str(stored['revision']); result['mods'][0]['enabled'] = stored['enabled']
            if mode.startswith('native-edit-'):
                result['warnings'] = [{'code': 'retainedWarning', 'message': 'An existing library warning.'}]
            if mode.endswith('-snapshot-error') and method == 'native.snapshot' and (root / 'lost').exists() and not (root / 'allow-snapshot').exists():
                print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32020, 'message': 'Snapshot temporarily unavailable', 'data': {'kind': 'nativeIo'}}}), flush=True)
                continue
            if method == 'native.operation':
                known = params['operationId'] in stored['operations']
                send(request_id, {'operationId': params['operationId'], 'status': 'succeeded', 'result': result} if known else {'operationId': params['operationId'], 'status': 'unknown'})
                continue
            if method == 'native.applyPreview':
                send(request_id, apply_review(params['profileId']))
                continue
            if method in ('native.setEnabled', 'native.restore'):
                if params['expectedRevision'] != result['revision']:
                    print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32020, 'message': 'Changed elsewhere', 'data': {'kind': 'staleRevision'}}}), flush=True)
                    continue
                first = mode != 'native-edit-ok' and not (root / 'lost').exists()
                if first: (root / 'lost').write_text('1')
                if not first or mode != 'native-edit-lost-unstored':
                    stored['revision'] += 1; stored['operations'].append(params['operationId'])
                    if method == 'native.setEnabled': stored['enabled'] = params['enabled']
                    saved.write_text(json.dumps(stored))
                    result['revision'] = str(stored['revision']); result['mods'][0]['enabled'] = stored['enabled']
                if first:
                    if mode.endswith('-held'):
                        (root / 'held').write_text('1')
                        deadline = time.monotonic() + 10
                        while not (root / 'release').exists() and time.monotonic() < deadline:
                            time.sleep(.01)
                    if mode.endswith(('-limit', '-internal')):
                        print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32603, 'message': 'Post-dispatch serialization failed', 'data': {'kind': 'responseTooLarge' if mode.endswith('-limit') else 'internalError'}}}), flush=True)
                    else: print('malformed reply after dispatch', flush=True)
                    sys.exit(0)
            send(request_id, result); continue
        if mode in ('native-disconnect', 'native-response-limit', 'native-internal-error'):
            root = Path(params['dataDir']); root.mkdir(exist_ok=True)
            finished = root / 'finished.json'
            if finished.exists():
                result['revision'] = '2'; result['mods'][0]['enabled'] = False
            assert method != 'native.operation', 'A lost edit has no outcome to look up'
            if method == 'native.setEnabled':
                with (root / 'dispatches').open('a') as trace: trace.write('write\n')
                if mode == 'native-disconnect':
                    print('malformed reply after dispatch', flush=True)
                else:
                    print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32603, 'message': 'Post-dispatch serialization failed', 'data': {'kind': 'responseTooLarge' if mode == 'native-response-limit' else 'internalError'}}}), flush=True)
                time.sleep(.7)
                finished.write_text(json.dumps({'operationId': params['operationId']}))
                sys.exit(0)
        if method == 'native.setEnabled':
            assert params['profileId'] == 'default' and params['modId'] == 'image-mod'
            assert params['expectedRevision'] == '1' and not params['enabled']
            if mode == 'native-reject':
                print(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'error': {'code': -32020, 'message': 'Changed elsewhere', 'data': {'kind': 'staleRevision'}}}), flush=True)
                continue
            if mode == 'native-delay':
                dispatch_root = Path(params['dataDir']); dispatch_root.mkdir(parents=True, exist_ok=True)
                (dispatch_root / 'dispatches').write_text('write\n')
            time.sleep(5.4)
            result['revision'] = '2'; result['mods'][0]['enabled'] = False
        send(request_id, result)
        continue
    if request['method'] == 'hello':
        if mode == 'invalid':
            print('not json', flush=True); continue
        if mode == 'oversized':
            sys.stdout.write('x' * (32 * 1024 * 1024 + 1)); sys.stdout.flush(); continue
        if mode == 'truncated':
            sys.stdout.write('{"jsonrpc":'); sys.stdout.flush(); sys.exit(0)
        if mode == 'stderr':
            sys.stderr.write('SYNTHETIC_STDERR_SENTINEL' * 10000); sys.stderr.flush()
        result = paired_hello()
        if mode == 'hello-version': result['protocolVersion'] = 2
        if mode == 'hello-write': result['capabilities'].remove('native.snapshot')
        if mode == 'hello-capabilities': result['capabilities'] = []
        send(request_id, result)
    else:
        if request['method'] == 'native.snapshot':
            print(json.dumps({'jsonrpc':'2.0', 'id':request_id, 'error':{'code':-32020, 'message':'No native document in this source fixture.', 'data':{'kind':'nativeNotImported'}}}), flush=True)
            continue
        handled, value = paired_startup(request['method'], request.get('params', {}), native=False)
        if handled:
            send(request_id, value); continue
        profile = request.get('params', {}).get('profileId', 'default')
        result = snapshot(profile)
        if mode == 'bad-snapshot':
            result['mods'] = [{'id': 'incomplete'}]
        if mode == 'wrong-profile':
            result['selectedProfileId'] = 'slow'
        if mode == 'race' and profile == 'slow':
            threading.Timer(.25, send, args=(request_id, result)).start()
        else:
            send(request_id, result)

if mode.startswith('native-local') and local_job:
    local_job['sequence']=str(int(local_job['sequence'])+1)
    if local_job['phase'] in ('enumerating','inspecting','hashing','ready','cancelRequested'):
        local_job['phase']='interrupted'; local_job['canCancel']=False
    (root/'local-restarted.json').write_text(json.dumps(local_job))
    with (root/'trace.jsonl').open('a') as trace: trace.write(json.dumps({'pid':os.getpid(),'method':'EOF'})+'\n')
