#!/usr/bin/python3
"""Private STEP6 protocol fixture. Trace identities/methods, never arguments."""
import hashlib
import json
import os
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup
import sys
import time

controls = {}
owner = False


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()


def wait_file(path):
    deadline = time.monotonic() + 15
    while not path.exists():
        if time.monotonic() > deadline:
            sys.exit(72)
        time.sleep(.005)


def send(identity, value):
    observe(value)
    print(json.dumps({'jsonrpc': '2.0', 'id': identity, 'result': value}, ensure_ascii=False), flush=True)


for line in sys.stdin:
    request = json.loads(line)
    identity, method, params = request['id'], request['method'], request.get('params', {})
    if method == 'hello':
        send(identity, paired_hello())
        continue
    root = Path(params['dataDir'])
    root.mkdir(parents=True, exist_ok=True)
    mode = (root/'mode').read_text() if (root/'mode').exists() else 'ordinary'
    with (root/'calls.jsonl').open('a') as trace:
        trace.write(json.dumps({'method': method, 'pid': os.getpid(),
                                'expected': params.get('expectedSettingsRevision'),
                                'settingsRevision': params.get('settingsRevision')}) + '\n')
    saved = root/'settings.json'
    settings = json.loads(saved.read_text()) if saved.exists() else {'version': 1, 'revision': '0', 'additionalArguments': 'old'}
    if not method.startswith('native.launch.') and method != 'native.game.status':
        handled, value = paired_startup(method, params)
        if handled:
            send(identity, value); continue
    if method == 'native.snapshot':
        send(identity, {'protocolVersion': 1, 'sourcePath': '', 'sourceKind': 'native', 'storeVersion': 1,
            'revision': '1', 'activeProfileId': 'default', 'selectedProfileId': params.get('profileId') or 'default',
            'profiles': [{'id': p, 'name': p, 'isActive': p == 'default', 'modCount': 0} for p in ('default', 'other')],
            'mods': [], 'warnings': [], 'pendingChanges': True, 'gamePath': '/synthetic/game',
            'appliedProfileId': None, 'canRestore': False, 'recoveryRequired': False})
    elif method == 'native.game.status':
        send(identity, {'version': 1, 'state': 'idle', 'scanComplete': True, 'processes': [], 'stop': None})
    elif method == 'native.launch.owner':
        send(identity, {'version': 1, 'state': 'open' if owner else 'closed', 'busy': False, 'active': False, 'childRetained': False})
    elif method == 'native.launch.close':
        owner = False
        send(identity, {'version': 1, 'status': 'closing'})
    elif method == 'native.launch.control':
        result = controls[params['controlId']]
        if mode.endswith('get-control-refusal') and (root/'lost').exists() and result.get('value') and 'additionalArguments' in result['value']:
            blocked = root/'blocked-read-control'
            if not blocked.exists(): blocked.write_text(params['controlId'])
            if blocked.read_text() == params['controlId']:
                print(json.dumps({'jsonrpc': '2.0', 'id': identity, 'error': {'code': -32020, 'message': 'Synthetic read control refused.', 'data': {'kind': 'nativeIo'}}}), flush=True)
                continue
        if mode in ('control-refusal', 'unknown-control') and result.get('value') and 'settingsSha256' in result['value'] and not (root/'control-refused').exists():
            (root/'control-refused').touch()
            if mode == 'control-refusal':
                print(json.dumps({'jsonrpc': '2.0', 'id': identity, 'error': {'code': -32020, 'message': 'Synthetic control read refused.', 'data': {'kind': 'nativeIo'}}}), flush=True)
            else:
                send(identity, {'version': 1, 'controlId': params['controlId'], 'state': 'unknown', 'inputSha256': None, 'value': None, 'error': None})
        else:
            send(identity, result)
    elif method.startswith('native.launch.'):
        payload = {k: v for k, v in params.items() if k not in ('dataDir', 'controlId')}
        input_hash = digest({'rootPath': str(root), 'input': {'method': method, 'params': payload}})
        error = None
        if method == 'native.launch.open':
            if mode == 'open-refusal':
                error = {'kind': 'nativeIo', 'message': 'Owner admission refused. Try again.'}
                value = None
            else:
                owner = True
                value = {'version': 1, 'epoch': '1', 'admitted': '0', 'settingsWrites': '0'}
        elif method == 'native.launch.barriers':
            value = {'version': 1, 'barriers': []}
        elif method == 'native.launch.settings.get':
            if (root/'lost').exists():
                (root/'read-started').touch()
                if (root/'hold-read').exists():
                    wait_file(root/'release-read')
                if (root/'fail-read').exists():
                    error = {'kind': 'nativeIo', 'message': 'Synthetic settings read refused.'}
                if (root/'dead-read').exists():
                    print('synthetic lost read reply', flush=True)
                    continue
            value = settings
        elif method == 'native.launch.settings.set':
            (root/'set-started').touch()
            if mode == 'stale' and not (root/'lost').exists():
                (root/'lost').touch()
                settings = {'version': 1, 'revision': '4', 'additionalArguments': 'external'}
                saved.write_text(json.dumps(settings))
            if params['expectedSettingsRevision'] != settings['revision']:
                error = {'kind': 'staleLaunchSettings', 'message': 'Launch settings changed; reload before saving.'}
                value = None
            elif mode.startswith('refused') and not (root/'lost').exists():
                (root/'lost').touch()
                error = {'kind': 'launchBusy' if mode.endswith('busy') else 'gameRunning', 'message': 'Close Deadlock before saving.'}
                value = None
            else:
                next_settings = {'version': 1, 'revision': str(int(settings['revision']) + 1), 'additionalArguments': params['additionalArguments']}
                value = {'version': 1, 'revision': next_settings['revision'], 'settingsSha256': digest(next_settings)}
                if mode.startswith('lost') and not (root/'lost').exists():
                    (root/'lost').touch()
                    # Make the client retire this writer while it still owns work.
                    print('synthetic lost settings reply', flush=True)
                    wait_file(root/'release-writer')
                    if 'unstored' not in mode:
                        saved.write_text(json.dumps(next_settings))
                        (root/'operation.json').write_text(json.dumps({'version': 1, 'epoch': params['epoch'], 'operationId': params['operationId'], 'status': 'succeeded',
                            'input': {'method': method, 'params': {'epoch': params['epoch'], 'operationId': params['operationId'], 'expectedSettingsRevision': params['expectedSettingsRevision'], 'argumentsSha256': hashlib.sha256(params['additionalArguments'].encode()).hexdigest()}}, 'receipt': value}))
                    if 'same-ui' in mode:
                        saved.write_text(json.dumps({'version': 1, 'revision': '4', 'additionalArguments': 'external'}))
                    (root/'writer-finished').touch()
                    sys.exit(0)
                if (root/'hold-set').exists():
                    wait_file(root/'release-writer')
                saved.write_text(json.dumps(next_settings))
        elif method == 'native.launch.settings.operation':
            value = json.loads((root/'operation.json').read_text()) if (root/'operation.json').exists() else {'version': 1, 'epoch': params['epoch'], 'operationId': params['operationId'], 'status': 'unknown'}
        elif method == 'native.launch.begin':
            # Assert the reviewed UI revision reached Play; never start a game.
            assert params['settingsRevision'] == settings['revision']
            value = None
            error = {'kind': 'syntheticStop', 'message': 'Synthetic Play boundary reached.'}
        else:
            raise AssertionError('Unexpected launch call: ' + method)
        controls[params['controlId']] = {'version': 1, 'controlId': params['controlId'],
            'state': 'failed' if error else 'succeeded', 'inputSha256': input_hash, 'value': None if error else value, 'error': error}
        send(identity, {'version': 1, 'accepted': True, 'controlId': params['controlId'], 'inputSha256': input_hash})
    else:
        raise AssertionError('Unexpected call: ' + method)
