#!/usr/bin/python3
"""Seven linked mods, owned thumbnails, no provider traffic; real Library delegates."""
import hashlib
import json
import os
from pathlib import Path
from paired_helper import hello as paired_hello, observe, startup as paired_startup
import struct
import subprocess
import sys
import tempfile
import time
import zlib


def run(binary):
    with tempfile.TemporaryDirectory(prefix='dlmm-flicker-') as scratch:
        root = Path(scratch)
        env = os.environ.copy()
        env['HOME'] = str(root / 'home')
        for key in set(k for k in env if k.startswith('XDG_')) | {
            'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME',
            'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'}:
            env[key] = str(root / key)
            Path(env[key]).mkdir(mode=0o700)
        Path(env['HOME']).mkdir()
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                   QML_DISK_CACHE_PATH=str(root / 'qmlcache'), QT_LOGGING_RULES='*.debug=false', QT_FORCE_STDERR_LOGGING='1')
        library = root / 'library'
        (library / 'artwork').mkdir(parents=True)
        def chunk(kind, data):
            return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))
        image = (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', 32, 32, 8, 2, 0, 0, 0))
                 + chunk(b'IDAT', zlib.compress((b'\0' + b'\x40\xa0\x80' * 32) * 32)) + chunk(b'IEND', b''))
        (library / 'artwork' / ('a' * 32 + '.image')).write_bytes(image)
        artwork = dict(id='a' * 32, sha256=hashlib.sha256(image).hexdigest(), size=len(image), format='png')
        (library / 'artwork.json').write_text(json.dumps(artwork))
        command = [binary, '--helper', str(Path(__file__).resolve()), '--data-dir', str(library),
                   '--check-ui', 'library-flicker']
        completed = subprocess.run(command, env=env, stderr=subprocess.STDOUT, timeout=110)
        trace = [json.loads(line) for line in (library / 'trace.jsonl').read_text().splitlines()] if (library / 'trace.jsonl').exists() else []
        counts = {method: sum(row['method'] == method for row in trace) for method in sorted({r['method'] for r in trace})}
        print('FLICKER helper requests', json.dumps(counts, sort_keys=True))
        # Both explicit checks visit files + changelog for all seven submissions.
        assert sum(row.get('refresh', False) for row in trace) == 28, counts
        renewed = any(row.get('operation', {}).get('args', {}).get('submission', {}).get('submissionId') == '200'
                      for row in trace if row.get('operation'))
        print('FLICKER submission demand renewed', renewed)
        return completed.returncode or (0 if renewed else 1)


def serve():
    receipts = {}
    snapshots = 0
    for line in sys.stdin:
        request = json.loads(line)
        method, p = request['method'], request.get('params', {})
        if method == 'hello':
            result = paired_hello()
        else:
            root = Path(p['dataDir'])
            with (root / 'trace.jsonl').open('a') as trace:
                trace.write(json.dumps(dict(method=method, refresh=p.get('operation', {}).get('args', {}).get('refresh', False), operation=p.get('operation'))) + '\n')
            if method == 'native.snapshot':
                snapshots += 1
                result = dict(protocolVersion=1, sourceKind='native', storeVersion=1, revision='1',
                    sourcePath=str(root / 'library.json'), activeProfileId='default', selectedProfileId='default',
                    pendingChanges=False, canRestore=False, recoveryRequired=False, gamePath='',
                    appliedProfileId=None, profiles=[dict(id='default', name='Default', isActive=True, modCount=7)],
                    warnings=[], mods=[dict(id=f'mod-{i}', remoteId=str(200 if i == 0 and snapshots >= 4 else 100+i), title=f'Linked mod {i}',
                        author='Fixture', hero='Ivy', category='Skins', status='installed', enabled=True,
                        appliedEnabled=False, available=True, unavailableReason=None, installOrder=None,
                        installedVpks=[], missingVpks=[], localArtwork=json.loads((root / 'artwork.json').read_text())) for i in range(7)])
            elif method == 'native.modUpdates.get':
                available = root / 'update-available'
                if p.get('finish'):
                    available.touch()
                result = dict(profileId='default', revision='1', lastUpdateCheck=int(time.time()), mods=[
                    dict(modId=f'mod-{i}', status='available' if i == 0 and available.exists() else 'current',
                        fileId='999', createdAt=123, submission=dict(provider='gamebanana',
                        submissionType='mod', submissionId=str(100+i))) for i in range(7)])
            elif method.startswith('native.catalog.'):
                result = dict(version=1, root=str(root), catalogSession='b'*32)
                key = p.get('requestKey')
                if method.endswith('open') or method.endswith('status') and not key:
                    result.update(phase='ready', error=None)
                elif method.endswith('close'):
                    result.update(phase='closed')
                elif method.endswith(('cancel', 'release')):
                    result.update(requestKey=key, state='released')
                elif method.endswith('begin'):
                    receipts[key] = p
                    result.update(requestKey=key, viewGeneration=p['viewGeneration'], inputDigest='c'*64, phase='running')
                else:
                    r = receipts[key]
                    result.update(requestKey=key, viewGeneration=r['viewGeneration'], inputDigest='c'*64,
                        operation=r['operation'], phase='failed', progress=None, result=None,
                        error=dict(kind='catalogNotFound', message='Controlled absent catalog page', retryAfterSeconds=None))
            else:
                handled, result = paired_startup(method, p)
                if not handled: raise AssertionError(method)
        observe(result)
        print(json.dumps(dict(jsonrpc='2.0', id=request['id'], result=result)), flush=True)


if __name__ == '__main__':
    if len(sys.argv) == 2:
        raise SystemExit(run(sys.argv[1]))
    serve()
