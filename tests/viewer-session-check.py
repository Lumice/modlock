#!/usr/bin/python3
"""File-only viewer hosts; the wire peer supplies an empty, ready catalog."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from integrationcheck import raster
from paired_helper import hello, observe, startup


def serve():
    receipts = {}
    for line in sys.stdin:
        req = json.loads(line)
        method, p = req['method'], req.get('params', {})
        if method == 'hello':
            result = hello()
        elif method == 'native.snapshot':
            result = json.loads((Path(p['dataDir']) / 'snapshot.json').read_text())
        elif method.startswith('native.catalog.'):
            key = p.get('requestKey')
            result = dict(version=1, root=p['dataDir'], catalogSession='b'*32)
            if method.endswith('open') or method.endswith('status') and not key:
                result.update(phase='ready', error=None)
            elif method.endswith('close'):
                result.update(phase='closed')
            elif method.endswith('release'):
                continue  # Retain decoded-response fixtures for same-generation updates.
            elif method.endswith('cancel'):
                result.update(requestKey=key, state='released')
            elif method.endswith('begin'):
                receipts[key] = p
                result.update(requestKey=key, viewGeneration=p['viewGeneration'], inputDigest='c'*64, phase='running')
            else:
                r = receipts[key]
                result.update(requestKey=key, viewGeneration=r['viewGeneration'], inputDigest='c'*64,
                              operation=r['operation'], phase='succeeded', progress=None, error=None)
                kind, args = r['operation']['kind'], r['operation']['args']
                if kind in ('collection', 'detail'):
                    result.update(phase='running', result=None)
                    data = None
                elif kind == 'query':
                    data = dict(items=[], total='0', page=0, pageSize=50, preferencesRevision='1', favorites=[])
                elif kind == 'facets':
                    data = []
                elif kind == 'favoritesGet':
                    data = dict(submission=args.get('submission'), favorite=False if args.get('submission') else None, preferencesRevision='1')
                else:
                    result.update(phase='failed', result=None, error=dict(kind='catalogNotFound', message='Fixture absent', retryAfterSeconds=None))
                    data = None
                if data is not None:
                    prefs = '1' if kind in ('collection', 'query', 'favoritesGet') else None
                    result['result'] = dict(identity=dict(kind=kind, args=args, catalogRevision='1', preferencesRevision=prefs),
                        catalogRevision='1', data=data, provenance=dict(source='cache', sourcePin='976637e2d63329ffef6f87eca40e2aea681acc94',
                        fetchedAt=None, lastSuccessfulSyncAt=None, stale=False, incomplete=False, refreshError=None,
                        searchAvailable=False, searchState='unavailable', accounting=dict(databaseBytes='0', sidecarBytes='0',
                        spoolAndControlBytes='0', reserveBytes='1073741824', aggregateQuota=False)))
        else:
            handled, result = startup(method, p)
            if not handled:
                raise AssertionError(method)
        observe(result)
        print(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=result)), flush=True)


def run(binary):
    with tempfile.TemporaryDirectory(prefix='vs-') as d:
        root = Path(d)
        env = os.environ.copy()
        for i, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR')):
            folder = root / str(i)
            folder.mkdir(mode=0o700)
            env[key] = str(folder)
        first, second = root / 'first.png', root / 'second.png'
        raster(first, (210, 45, 65)); raster(second, (30, 115, 220))
        library = root / 'library'; library.mkdir()
        (library / 'snapshot.json').write_text(json.dumps(dict(protocolVersion=1, sourceKind='native', storeVersion=1, revision='1',
            sourcePath=str(library / 'library.json'), activeProfileId='default', selectedProfileId='default', pendingChanges=False,
            canRestore=False, recoveryRequired=False, gamePath='', appliedProfileId=None,
            profiles=[dict(id='default', name='Default', isActive=True, modCount=0)], warnings=[], mods=[])))
        env.update(UI_VIEWER_ROOT=str(library), UI_VIEWER_PEER=str(Path(__file__).resolve()),
                   UI_VIEWER_QML=str(Path(__file__).with_name('viewer-session-fixture.qml').resolve()),
                   UI_PREVIEW_FIRST=first.as_uri(), UI_PREVIEW_SECOND=second.as_uri(),
                   UI_PREVIEW_ENCODED=first.as_uri().replace('/first.png', '/%66irst.png'),
                   QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', QML_DISABLE_DISK_CACHE='1')
        for arg in sys.argv[2:]:
            if arg.startswith('--review-case='):
                env['UI_VIEWER_REVIEW_CASE'] = arg.split('=', 1)[1]
                if env['UI_VIEWER_REVIEW_CASE'] == 'encoded-seed':
                    env['UI_VIEWER_HOST'] = 'local'
        return subprocess.run([binary, '--demo', '--check-ui', 'viewer-data-change' if '--data-change' in sys.argv or os.environ.get('UI_VIEWER_DATA_ONLY') else 'viewer-session'], env=env, timeout=90).returncode


if __name__ == '__main__':
    raise SystemExit(run(sys.argv[1])) if len(sys.argv) >= 2 else serve()
