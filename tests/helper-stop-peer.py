#!/usr/bin/python3
"""Synthetic paired JSON-RPC peer for item 53 local helper-stop verification."""
import hashlib
import json
import sys
from pathlib import Path

from paired_helper import hello as paired_hello, observe, startup as paired_startup

STATE_DIR = Path(sys.argv[0]).resolve().parent if not Path(sys.argv[0]).is_symlink() else Path(sys.argv[0]).parent
LOCAL_CONTROLS = {}


def send(request_id, result):
    observe(result)
    sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': request_id, 'result': result}) + '\n')
    sys.stdout.flush()


def send_error(request_id, code, message, kind):
    sys.stdout.write(json.dumps({
        'jsonrpc': '2.0',
        'id': request_id,
        'error': {'code': code, 'message': message, 'data': {'kind': kind}},
    }) + '\n')
    sys.stdout.flush()


def make_snapshot(params):
    return {
        'protocolVersion': 1,
        'sourceKind': 'native',
        'storeVersion': 1,
        'sourcePath': '/synthetic/state.json',
        'revision': '1',
        'activeProfileId': 'default',
        'selectedProfileId': params.get('profileId') or 'default',
        'pendingChanges': True,
        'gamePath': '/synthetic/game',
        'appliedProfileId': None,
        'canRestore': True,
        'recoveryRequired': (STATE_DIR / 'recovery-required').exists(),
        'profiles': [{'id': 'default', 'name': 'Default', 'isActive': True, 'modCount': 1}],
        'mods': [{
            'id': 'image-mod',
            'remoteId': '123',
            'title': 'Artwork',
            'author': 'Synthetic',
            'hero': 'Ivy',
            'category': 'Skins',
            'status': 'installed',
            'enabled': True,
            'appliedEnabled': False,
            'available': True,
            'unavailableReason': None,
            'installOrder': None,
            'installedVpks': [],
            'missingVpks': [],
        }],
        'warnings': [],
    }


def make_control(method, params, value):
    payload = {k: v for k, v in params.items() if k not in ('dataDir', 'controlId')}
    digest = hashlib.sha256(json.dumps(
        {'rootPath': params['dataDir'], 'input': {'method': method, 'params': payload}},
        sort_keys=True, separators=(',', ':'), ensure_ascii=False,
    ).encode()).hexdigest()
    LOCAL_CONTROLS[params['controlId']] = {
        'version': 1,
        'controlId': params['controlId'],
        'state': 'succeeded',
        'inputSha256': digest,
        'value': value,
        'error': None,
    }
    return {
        'version': 1,
        'accepted': True,
        'controlId': params['controlId'],
        'inputSha256': digest,
    }


for line in sys.stdin:
    request = json.loads(line)
    request_id = request['id']
    method = request['method']
    params = request.get('params', {})

    fail_file = STATE_DIR / 'fail-on-method'
    if fail_file.exists() and fail_file.read_text().strip() == method:
        sys.exit(1)

    if method == 'hello':
        send(request_id, paired_hello())
        continue

    if method == 'native.launch.control' and params.get('controlId') in LOCAL_CONTROLS:
        send(request_id, LOCAL_CONTROLS[params['controlId']])
        continue

    if method == 'native.launch.barriers' and (STATE_DIR / 'legacy-launch-barrier').exists():
        value = {
            'version': 1,
            'barriers': [{
                'version': 1,
                'rootPath': params['dataDir'],
                'journalId': '0' * 32,
                'epoch': '7',
                'attemptId': 'a' * 32,
                'recordRevision': '0',
                'stage': 'dispatchIntent',
                'pairSha256': 'b' * 64,
                'legacy': True,
                'markerTime': None,
            }],
        }
        send(request_id, make_control(method, params, value))
        continue

    if method == 'native.launch.query':
        send(request_id, make_control(method, params, {
            'version': 1,
            'epoch': params['epoch'],
            'attemptId': params['attemptId'],
            'state': 'unknown',
        }))
        continue

    handled, value = paired_startup(method, params, native=True)
    if handled:
        send(request_id, value)
        continue

    if method == 'native.setEnabled' and (STATE_DIR / 'spoof-wire-error').exists():
        send_error(request_id, -32020, 'helperStopped: spoofed wire error', 'helperStopped')
        continue

    if method in ('native.snapshot', 'native.recoverySnapshot'):
        send(request_id, make_snapshot(params))
        continue

    if method == 'native.operation':
        send(request_id, {
            'operationId': params['operationId'],
            'status': 'succeeded',
            'result': make_snapshot(params),
        })
        continue

    send_error(request_id, -32601, f'Unexpected method in helper-stop-peer: {method}', 'unsupportedAction')
