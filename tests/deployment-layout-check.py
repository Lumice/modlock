#!/usr/bin/env python3
"""New Apply requires an explicit current layout; Restore keeps its own review."""
import argparse
import importlib.util
import json
import sqlite3
import tempfile
from pathlib import Path
from types import SimpleNamespace
from nativeintegrationcheck import FamilyUi

spec = importlib.util.spec_from_file_location('compat', Path(__file__).with_name('game-format-cross-version-check.py'))
compat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compat)
parser = argparse.ArgumentParser()
parser.add_argument('--helper', type=Path, required=True)
args = parser.parse_args()

def evidence(f):
    with sqlite3.connect(f.data/'library.sqlite3') as db:
        journals = db.execute('SELECT * FROM journals').fetchall()
        operations = db.execute('SELECT * FROM operations').fetchall()
    return compat.doc(f.data), journals, operations, {str(p.relative_to(f.game)): p.read_bytes() for p in f.game.rglob('*') if p.is_file()}

with tempfile.TemporaryDirectory(prefix='dlmm-layout-') as temp:
    f = FamilyUi(SimpleNamespace(helper=args.helper, binary=None, capture_dir=None), Path(temp), 'layout')
    h = compat.Helper(args.helper.resolve(), f.data)
    p = h.ok('native.importPreview', statePath=str(f.source), gamePath=str(f.game))
    h.ok('native.importCommit', token=p['token'], operationId='import', expectedRevision='0')
    before = evidence(f)
    for label, layout in [('omitted', {}), ('null', {'deploymentLayout': None}), ('unknown', {'deploymentLayout': 'future'}), ('flat', {'deploymentLayout': 'flat'}), ('invalid', {'deploymentLayout': []})]:
        response = h.call('native.applyPreview', profileId='default', gamePath=str(f.game), **layout)
        assert response.get('error', {}).get('data', {}).get('kind') == 'unsupportedDeploymentLayout', (label, response)
        assert 'shards-v1' in response['error']['message'], response
        assert evidence(f) == before, label
        print('PASS unsupported layout preserves game, document, journals and receipts:', label, flush=True)
    p = h.ok('native.applyPreview', profileId='default', gamePath=str(f.game), deploymentLayout='shards-v1')
    assert p['deploymentLayout'] == 'shards-v1'
    for index, layout in enumerate(({}, {'deploymentLayout': None}, {'deploymentLayout': 'flat'})):
        response = h.call('native.apply', token=p['token'], operationId=f'ack-{index}', expectedRevision='1', **layout)
        assert response.get('error', {}).get('data', {}).get('kind') == 'stalePreview', response
        observed = evidence(f)
        assert (observed[0], observed[1], observed[3]) == (before[0], before[1], before[3])
    response = h.call('native.apply', token='f'*32, operationId='bad-token', expectedRevision='1', deploymentLayout='shards-v1')
    assert response.get('error', {}).get('data', {}).get('kind') == 'stalePreview', response
    observed = evidence(f)
    assert (observed[0], observed[1], observed[3]) == (before[0], before[1], before[3])
    with sqlite3.connect(f.data/'library.sqlite3') as db:
        assert sum(json.loads(row[0])["status"] == "succeeded" for row in db.execute("SELECT result FROM operations")) == 1
    result = h.ok('native.apply', token=p['token'], operationId='apply', expectedRevision='1', deploymentLayout='shards-v1')
    assert compat.doc(f.data)['applied']['manifest']['version'] == 3
    assert h.ok('native.apply', token=p['token'], operationId='apply', expectedRevision='1', deploymentLayout='shards-v1') == result
    changed = h.call('native.apply', token=p['token'], operationId='apply', expectedRevision='1', deploymentLayout='flat')
    assert changed.get('error', {}).get('data', {}).get('kind') == 'operationIdConflict', changed
    assert h.ok('native.applyPreview', restoreReview=True) == {'restoreReview': True}
    h.ok('native.restore', operationId='restore', expectedRevision='2')
    h.close()
    assert f.info.read_bytes() == before[3]['game/citadel/gameinfo.gi']
    print('PASS current sharded Apply, layout/token binding, receipt replay, early Restore review and exact Restore', flush=True)
