#!/usr/bin/env python3
"""Controlled gallery receipts; never starts a helper or contacts a provider."""
import hashlib
import json
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup
import sys
import time

receipts = {}
for line in sys.stdin:
    request = json.loads(line)
    method, p = request['method'], request.get('params', {})
    if method == 'hello':
        result = paired_hello()
    else:
        root = Path(p['dataDir'])
        with (root / 'trace.jsonl').open('a') as trace:
            trace.write(json.dumps(request) + '\n')
        if method == 'native.snapshot':
            time.sleep(0.2)  # sample masking throughout a real model refresh
            result = json.loads((root / 'snapshot.json').read_text())
        elif method.startswith('native.catalog.'):
            key = p.get('requestKey')
            result = dict(version=1, root=str(root),
                          catalogSession=hashlib.sha256(str(root).encode()).hexdigest()[:32])
            if method.endswith('open') or method.endswith('status') and not key:
                result.update(phase='ready', error=None)
            elif method.endswith('close'):
                result.update(phase='closed')
            elif method.endswith(('cancel', 'release')):
                result.update(requestKey=key, state='released')
            elif method.endswith('begin'):
                receipts[key] = p
                result.update(requestKey=key, viewGeneration=p['viewGeneration'],
                              inputDigest='c'*64, phase='running')
            else:
                r = receipts[key]
                submission = r['operation']['args']['submission']
                complete = root / ('complete-' + submission['submissionId'])
                result.update(requestKey=key, viewGeneration=r['viewGeneration'], inputDigest='c'*64,
                              operation=r['operation'], phase='running', progress=None, result=None, error=None)
                if complete.exists():
                    summary = dict(submission=submission, name='Gallery', description='', author='Fixture',
                        authorRemoteId=None, profileUrl='https://gamebanana.com/mods/' + submission['submissionId'],
                        category='Skins', hero='Ivy', isAudio=False, isMap=False, isNsfw=True,
                        visibilityKnown=True, isObsolete=False, hasFiles=False, downloadCount='0', likes='0',
                        images=[complete.read_text()], thumbnailUrl=None, audioUrl=None, tags=[],
                        remoteAddedAt=None, remoteUpdatedAt=None, filesUpdatedAt=None,
                        developmentState=None, completionPercentage=None, hydrated=False)
                    result.update(phase='succeeded', result=dict(state='partial', reason=None,
                        submission=submission, resource=dict(summary=summary, textFormat='plain'),
                        refreshRecommended=False, refreshState='idle'))
                    data = result['result']
                    pin = '976637e2d63329ffef6f87eca40e2aea681acc94'
                    result['result'] = dict(identity=dict(kind=r['operation']['kind'], args=r['operation']['args'],
                        catalogRevision='1', preferencesRevision=None), catalogRevision='1', data=data,
                        provenance=dict(source='cache', sourcePin=pin, fetchedAt=None, lastSuccessfulSyncAt=None,
                            stale=False, incomplete=False, refreshError=None, searchAvailable=False, searchState='unavailable',
                            accounting=dict(databaseBytes='0', sidecarBytes='0', spoolAndControlBytes='0',
                                reserveBytes='1073741824', aggregateQuota=False)))
        else:
            handled, result = paired_startup(method, p)
            if not handled: raise AssertionError(method)
    observe(result)
    print(json.dumps(dict(jsonrpc='2.0', id=request['id'], result=result)), flush=True)
