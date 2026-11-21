#!/usr/bin/env python3
"""Targeted metadata-only profile interchange integration, using a fresh library."""
import base64
import zlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile

helper = str(Path(sys.argv[1]).resolve())
with tempfile.TemporaryDirectory(prefix='dlmm-profile-share-') as scratch:
    root = Path(scratch)
    data = root / 'library'
    env = dict(os.environ, XDG_DATA_HOME=str(root/'data'), XDG_CONFIG_HOME=str(root/'config'), XDG_CACHE_HOME=str(root/'cache'))
    def rpc(method, params):
        request = dict(jsonrpc='2.0', id=1, method=method, params=dict(dataDir=str(data), **params))
        result = subprocess.run([helper], input=json.dumps(request)+'\n', text=True, capture_output=True, env=env, timeout=30)
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)
    def value(method, **params):
        r = rpc(method, params)
        assert 'result' in r, r
        return r['result']
    value('native.library.initialize', version=1, expectedRevision='0', operationId='1'*32)
    before = value('native.snapshot', version=1)
    exported = value('native.profileShare.export', profileId='default', includeNames=False)
    code = exported['code']
    assert code.startswith("DLMM1-")
    assert json.loads(zlib.decompress(base64.urlsafe_b64decode(code[6:] + "=" * (-len(code[6:]) % 4)))) == dict(format='dlmm-profile', version=1, mods=[], notShareable=[])
    review = value('native.profileShare.review', code=code)
    assert review['canCreate'] and not review['rows']
    params = dict(code=code, name='Shared test', expectedRevision=review['revision'], operationId='2'*32)
    first = value('native.profileShare.create', **params)
    replay = value('native.profileShare.create', **params)
    assert first == replay
    after = value('native.snapshot', version=1)
    assert int(after['revision']) == int(before['revision']) + 1
    assert len(after['profiles']) == len(before['profiles']) + 1
    assert after.get('appliedProfileId') == before.get('appliedProfileId')
    stale = rpc('native.profileShare.create', dict(params, operationId='3'*32))
    assert stale['error']['data']['kind'] == 'staleRevision', stale
    malformed = rpc('native.profileShare.review', dict(code='hosted-short-code'))
    assert malformed['error']['data']['kind'] == 'invalidSharedProfile', malformed
    missing = dict(format='dlmm-profile',version=1,mods=[dict(submission=dict(provider='gamebanana',submissionType='mod',submissionId='123'),fileId='456',options=['default'],enabled=True)],notShareable=[])
    code = json.dumps(missing)
    review = value('native.profileShare.review', code=code)
    assert not review['canCreate'] and review['rows'][0]['status'] == 'downloadRequired'
    assert review['rows'][0]['title'] == 'GameBanana mod 123'
    # A catalog-only fixture verifies presentation without any provider request.
    with sqlite3.connect(data / 'cache/catalog-v1.sqlite3') as cache:
        columns = cache.execute('PRAGMA table_info(submission)').fetchall()
        row = {c[1]: (0 if c[2] == 'INTEGER' else '') for c in columns}
        row.update(provider='gamebanana', submission_type='mod', submission_id='123', slug='123', name='Fixture title', payload=json.dumps(dict(name='Fixture title', isNsfw=False, thumbnailUrl='https://images.gamebanana.com/img/ss/mods/fixture.jpg')))
        cache.execute('INSERT INTO submission (' + ','.join(row) + ') VALUES (' + ','.join('?' for _ in row) + ')', list(row.values()))
    decorated = value('native.profileShare.review', code=code)['rows'][0]
    assert decorated['title'] == 'Fixture title'
    assert decorated['thumbnailUrl'].endswith('/fixture.jpg')
    bomb = 'DLMM1-' + base64.urlsafe_b64encode(zlib.compress(b' ' * 16385)).decode().rstrip('=')
    assert 'error' in rpc('native.profileShare.review', dict(code=bomb))
    rejected = rpc('native.profileShare.create', dict(code=code,name='Missing',expectedRevision=after['revision'],operationId='4'*32))
    assert rejected['error']['data']['kind'] == 'sharedDownloadRequired', rejected
    assert value('native.snapshot',version=1)['revision'] == after['revision']
    # Regression: association rows use source.id, never an absent envelope id.
    with sqlite3.connect(data / 'library.sqlite3') as db:
        doc = json.loads(db.execute('SELECT body FROM library').fetchone()[0])
        doc['profiles']['default']['mods'] = [dict(source=dict(id='local-fixture', remoteId='local-fixture', kind='local', name='Fixture', author='Author', category='Skins', status='downloaded', installedVpks=[], installOrder=0, createdAt='2026-11-01T22:00:02Z', nativeRemoteLink=dict(version=2, association='userSelected', submission=dict(provider='gamebanana',submissionType='mod',submissionId='123'), receipt=None)), enabled=False, available=False, previewHidden=False, packages=[])]
        db.execute('UPDATE library SET body=?', (json.dumps(doc),))
    updates = value('native.modUpdates.get', profileId='default', finish=False)
    assert updates['mods'][0]['modId'] == 'local-fixture', updates
    print('PASS profile sharing: offline export/review/create, no Apply, replay, stale revision, malformed code, missing download refusal')
