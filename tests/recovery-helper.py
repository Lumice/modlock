#!/usr/bin/python3
"""Stateful transport double for interruption/lock-barrier cases only."""
import json
import sys
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup

# These protocol doubles support the same explicit discovery entry point.
if sys.argv[1:]==['--data-root']:
    import os
    print(json.dumps({'root':str(Path(os.environ.get('XDG_DATA_HOME') or str(Path.home()/'.local/share'))/'modlock')}))
    sys.exit(0)

mode = Path(sys.argv[0]).name

def snapshot(revision='4'):
    return {'protocolVersion': 1, 'sourceKind': 'native', 'storeVersion': 4, 'packageVersion': 2,
            'fonts': {'version':1, 'installed':None, 'previous':None},
            'sourcePath': '/synthetic/state.json', 'revision': revision,
            'activeProfileId': 'default', 'selectedProfileId': 'default',
            'profiles': [{'id': 'default', 'name': 'Default', 'isActive': True, 'modCount': 0}],
            'mods': [], 'warnings': [], 'pendingChanges': False, 'gamePath': '/synthetic/game',
            'appliedProfileId': None, 'canRestore': False, 'recoveryRequired': False}

for line in sys.stdin:
    r = json.loads(line); method = r['method']; p = r.get('params', {}); out = None; error = None
    if method == 'hello':
        out = paired_hello()
    else:
        root = Path(p.get('dataDir', str(Path(sys.argv[0]).parent / 'native')))
        assert root.name == 'native', 'native data root was lost'
        root.mkdir(exist_ok=True)
        with (root / 'trace.jsonl').open('a') as trace:
            trace.write(json.dumps(r) + '\n')
        handled, value = paired_startup(method, p, native='no-doc' not in mode)
        if handled: out = value
        elif method == 'native.recovery.inspect':
            out = {'version':1, 'root':str(root), 'state': 'legacyUnimported' if 'no-doc' in mode else 'notRemote',
                   'expectedRevision':'0' if 'no-doc' in mode else None, 'targetOperationId':None,
                   'targetInputSha256':None, 'existingRecovery':None, 'error':None}
        elif method == 'native.snapshot':
            if 'store-error' in mode: error = ('nativeDatabase', 'Unreadable native database')
            elif 'no-doc' in mode: error = ('nativeNotImported', 'No document')
            else: out = snapshot('4')
        elif method == 'library.snapshot':
            error = ('sourceNotFound', 'No source available')
        elif method == 'native.operation':
            record = root / ('operation-' + p['operationId'] + '.json')
            recovery = p['operationId'] != 'original'
            if recovery and record.exists():
                saved = json.loads(record.read_text())
                out = {'operationId':p['operationId'], 'status':'failed' if 'error' in saved else 'succeeded', **saved}
                print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':out}),flush=True)
                continue
            status = ('failed' if 'no-doc' in mode else 'succeeded') if recovery else ('failed' if (root / 'recovered').exists() and 'no-doc' in mode else 'unknown')
            if 'terminal' in mode: status = 'succeeded'
            out = {'operationId': p['operationId'], 'status': status}
            if status == 'failed': out['error'] = {'kind': 'nativeNotImported' if recovery else 'operationInterrupted', 'message': 'Interrupted import reconciled'}
            if status == 'succeeded': out['result'] = snapshot('4' if recovery else '2')  # historical original must not replace revision 4
        elif method == 'native.recover':
            assert p['expectedRevision'] == ('0' if 'no-doc' in mode else '4')
            record = root / ('operation-' + p['operationId'] + '.json')
            if record.exists():
                saved = json.loads(record.read_text())
                if 'error' in saved: error = (saved['error']['kind'],saved['error']['message'])
                else: out = saved['result']
            elif (root / 'busy').exists(): error = ('nativeBusy', 'Another writer still owns the root')
            else:
                (root / 'recovered').touch()
                if 'no-doc' in mode: error = ('nativeNotImported', 'No native document after recovery')
                else: out = snapshot('4')
            record.write_text(json.dumps({'error':{'kind':error[0],'message':error[1]}} if error else {'result':out}))
        else:
            raise AssertionError(method)
    observe(out)
    response = {'jsonrpc': '2.0', 'id': r['id']}
    if error: response['error'] = {'code': -32020, 'message': error[1], 'data': {'kind': error[0]}}
    else: response['result'] = out
    print(json.dumps(response), flush=True)
