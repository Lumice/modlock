#!/usr/bin/env python3
"""A shared helper must admit history reads after its launch worker retires."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

with tempfile.TemporaryDirectory(prefix='native-retired-') as tmp:
    root = Path(tmp)
    env = {k: v for k, v in os.environ.items() if not k.startswith('XDG_')}
    for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR'):
        path = root / key
        path.mkdir(mode=0o700)
        env[key] = str(path)
    process = subprocess.Popen([sys.argv[1]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    def rpc(method, expect_error=False, **params):
        process.stdin.write(json.dumps(dict(jsonrpc='2.0', id=1, method=method, params=dict(dataDir=str(root/'native'), **params)))+'\n')
        process.stdin.flush()
        reply = json.loads(process.stdout.readline())
        if expect_error:
            assert 'error' in reply, 'Untyped launch revision was accepted'
            return reply['error']
        assert 'result' in reply, reply
        return reply['result']
    def control(method, expect_error=None, **params):
        identity = os.urandom(16).hex()
        rpc(method, controlId=identity, **params)
        deadline = time.monotonic()+10
        while True:
            receipt = rpc('native.launch.control', controlId=identity)
            if receipt['state'] != 'pending':
                if expect_error:
                    assert receipt['state'] == 'failed' and receipt['error']['kind'] == expect_error, 'Wrong launch refusal'
                    return receipt['error']
                assert receipt['state'] == 'succeeded', receipt
                return receipt['value']
            assert time.monotonic() < deadline
            time.sleep(.05)
    try:
        rpc('native.library.initialize', version=1, operationId='a'*32, expectedRevision='0')
        epoch = control('native.launch.open', openId='b'*32)['epoch']
        revision = '0'
        for number, raw in enumerate(('', '  -novid\t"synthetic"  ', '-name 界')):
            receipt = control('native.launch.settings.set', epoch=epoch, operationId=f'settings{number}', expectedSettingsRevision=revision, additionalArguments=raw)
            assert receipt['revision'] == str(int(revision)+1), 'Wrong settings revision'
            current = control('native.launch.settings.get')
            assert current['revision'] == receipt['revision'] and current['additionalArguments'] == raw, 'Helper changed raw launch arguments'
            control('native.launch.settings.set', expect_error='staleLaunchSettings', epoch=epoch, operationId=f'stale{number}', expectedSettingsRevision=revision, additionalArguments='other')
            assert control('native.launch.settings.get') == current, 'Stale save changed settings'
            revision = receipt['revision']
        rpc('native.launch.settings.set', expect_error=True, controlId='d'*32, epoch=epoch, operationId='untyped', expectedSettingsRevision=int(revision), additionalArguments='other')
        control('native.launch.settings.set', expect_error='staleLaunchEpoch', epoch=str(int(epoch)+1), operationId='wrong-owner', expectedSettingsRevision=revision, additionalArguments='other')
        assert control('native.launch.settings.get') == current, 'Stale owner changed settings'
        print('PASS launch settings exact raw text, typed revision, stale revision and owner refusal')
        rpc('native.remote.open', version=1, sessionId='c'*32)
        rpc('native.launch.close')
        deadline = time.monotonic()+10
        while rpc('native.launch.owner')['state'] != 'closed':
            assert time.monotonic() < deadline
            time.sleep(.05)
        assert control('native.launch.barriers')['barriers'] == []
        control('native.launch.settings.get')
        print('PASS retired shared owner admits reconciliation')
    finally:
        process.stdin.close()
        process.wait(timeout=15)
