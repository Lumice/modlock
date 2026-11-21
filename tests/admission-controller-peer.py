#!/usr/bin/python3
"""Paired startup and one-off request trace; no filesystem/game mutation."""
import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_helper import hello, startup

root = Path(sys.argv[0]).parent
for line in sys.stdin:
    request = json.loads(line)
    method, params = request['method'], request.get('params', {})
    with (root/'wire').open('a') as trace:
        trace.write(method+'\n')
    if method == 'hello':
        value = json.loads((root/'hello.json').read_text()) if (root/'hello.json').exists() else hello()
    elif method == 'native.snapshot':
        value = dict(protocolVersion=1, sourceKind='native', sourcePath='', storeVersion=1, revision='1',
                     activeProfileId='default', selectedProfileId='default', profiles=[dict(id='default', name='Default', isActive=True, modCount=0)],
                     mods=[], warnings=[], pendingChanges=False, gamePath='', appliedProfileId=None, canRestore=False, recoveryRequired=False)
    elif method.startswith(('native.storage.', 'native.maintenance.', 'native.profileShare.')):
        value = dict(revision='1', token='fixture-token', resume=True, canCancel=True, canClean=True, bytes='1',
                     destination=dict(path=str(root/'destination')), code='fixture-code', canCreate=True)
        if method == 'native.profileShare.export' and params.get('includeNames'):
            value['code'] = 'fixture-with-local-name'
    else:
        handled, value = startup(method, params)
        assert handled, method
    print(json.dumps(dict(jsonrpc='2.0', id=request['id'], result=value)), flush=True)
