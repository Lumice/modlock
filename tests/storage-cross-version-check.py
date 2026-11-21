#!/usr/bin/env python3
"""Exercise the actual legacy reader against disposable moved and unmoved stores."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile


def rpc(helper, method, data, **params):
    request = dict(jsonrpc='2.0', id=1, method=method,
                   params=dict(dataDir=str(data), **params))
    result = subprocess.run([str(helper)], input=json.dumps(request) + '\n',
                            text=True, capture_output=True, timeout=30, check=True)
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, help='Also check the Qt refusal banner')
    parser.add_argument('--helper', required=True, type=Path)
    parser.add_argument('--old-helper', type=Path)
    args = parser.parse_args()
    if args.old_helper is not None:
        assert args.old_helper.is_file(), f'Expected pre-rename helper is missing: {args.old_helper}'
        print('Old helper:', args.old_helper, 'sha256:', hashlib.sha256(args.old_helper.read_bytes()).hexdigest())
    with tempfile.TemporaryDirectory(prefix='dlmm-storage-version-') as tmp:
        root = Path(tmp)
        data = root / 'library'
        target = root / 'other drive'
        data.mkdir(mode=0o700)
        target.mkdir(mode=0o700)
        (data / 'payloads').mkdir(mode=0o700)
        (data / 'payloads/fixture.vpk').write_bytes(b'fixture bulk bytes')
        doc = dict(storeVersion=1, revision='1', sourcePath='/synthetic/state.json',
                   gamePath='', activeProfileId='default', profiles={'default': dict(
                       id='default', name='Fixture', protected=True, mods=[])},
                   importLedger=dict(manifests=[], sourcePayloads=[]), applied=None,
                   canRestore=False, recoveryRequired=False)
        dbpath = data / 'library.sqlite3'
        with sqlite3.connect(dbpath) as db:
            db.execute('CREATE TABLE library (id INTEGER PRIMARY KEY, body TEXT NOT NULL)')
            db.execute('CREATE TABLE operations (id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL)')
            db.execute('CREATE TABLE journals (id TEXT PRIMARY KEY, body TEXT NOT NULL)')
            db.execute('INSERT INTO library VALUES (1,?)', (json.dumps(doc),))
        dbpath.chmod(0o600)
        def ok(method, **params):
            response = rpc(args.helper, method, data, **params)
            assert 'result' in response, response
            return response['result']
        storage_aware = False
        if args.old_helper:
            old_view = rpc(args.old_helper, 'native.snapshot', data)
            assert 'result' in old_view, old_view
            storage_aware = 'storageRoot' in old_view['result']
            print('PASS unmoved fixture: old native.snapshot readable')
        else:
            ok('native.snapshot')
            print('PASS unmoved fixture: current native.snapshot readable')
        review = ok('native.storage.review', destination=str(target))
        assert review['source']['driveUsedBytes'] > 0
        assert review['destination']['driveUsedBytes'] > 0
        # Exact pre-copy journal checkpoint: new helper will resume this to
        # completion. In production Store writes this and Store5 atomically.
        journal = dict(kind='storageMove', version=1, before=doc,
                       source=dict(path=str(data), identity=review['sourceIdentity']),
                       destination=dict(path=str(target), identity=review['destinationIdentity']),
                       files=review['files'], input=dict(method='native.storage.move', params=dict(operationId='move')))
        with sqlite3.connect(dbpath) as db:
            db.execute('INSERT INTO journals VALUES (?,?)', ('move', json.dumps(journal)))
            db.execute('UPDATE library SET body=?', (json.dumps(doc | dict(storeVersion=5, storageDocumentVersion=1)),))
        def library_state():
            with sqlite3.connect(dbpath) as db:
                return json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
        def payload_state():
            return {str(p): p.read_bytes() for base in (data, target) for p in base.rglob('*.vpk')}
        def refused(label):
            # Old helpers create launch tables before refusing this store version.
            # Schema housekeeping is harmless; library content and payloads must survive.
            before = library_state(), payload_state()
            assert before[1], 'fixture payload must remain present'
            response = rpc(args.old_helper, 'native.snapshot', data)
            if storage_aware:
                assert 'result' in response,response
                assert (library_state(),payload_state())==before
                print('PASS',label,'pre-rename storage-aware snapshot remains readable')
                return
            assert response.get('error', {}).get('data', {}).get('kind') == 'unsupportedVersion', response
            assert (library_state(), payload_state()) == before
            print('PASS', label, 'old native.snapshot:', json.dumps(response['error']))
            for method in ('native.maintenance.scan', 'native.recover'):
                response = rpc(args.old_helper, method, data, **(dict(operationId='old-recovery', expectedRevision='1') if method == 'native.recover' else {}))
                assert response.get('error', {}).get('data', {}).get('kind') == 'unsupportedVersion', response
                print('PASS', label, method + ':', json.dumps(response['error']))
                assert (library_state(), payload_state()) == before
                assert payload_state(), 'refusing helper must preserve the payload'
        if args.old_helper:
            refused('pending move')
        ok('native.storage.resume')
        assert not (data / 'payloads/fixture.vpk').exists()
        assert list(target.rglob('fixture.vpk'))[0].read_bytes() == b'fixture bulk bytes'
        with sqlite3.connect(dbpath) as db:
            assert json.loads(db.execute('SELECT body FROM library').fetchone()[0])['storeVersion'] == 5
            assert db.execute('SELECT COUNT(*) FROM journals').fetchone()[0] == 0
        if args.old_helper:
            refused('moved fixture')
        ok('native.snapshot')
        print('PASS moved fixture: current native.snapshot readable; payload preserved')
        if args.binary and args.old_helper and not storage_aware:
            env = os.environ | dict(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                                    XDG_DATA_HOME=str(root / 'xdg'), XDG_CONFIG_HOME=str(root / 'config'),
                                    XDG_CACHE_HOME=str(root / 'cache'))
            result = subprocess.run([str(args.binary), '--helper', str(args.old_helper),
                                     '--data-dir', str(data), '--check-ui', 'storage-refusal'],
                                    env=env, text=True, capture_output=True, timeout=25)
            assert result.returncode == 0, result.stdout + result.stderr
            print(result.stdout.strip())


if __name__ == '__main__':
    main()
