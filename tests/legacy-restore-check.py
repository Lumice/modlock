#!/usr/bin/env python3
"""Restore a real historical helper's checkpoint, including after a reapply."""
import argparse
import importlib.util
import tempfile
import os
import json
import subprocess
import sqlite3
import hashlib
from pathlib import Path
from types import SimpleNamespace

from nativeintegrationcheck import FamilyUi

spec = importlib.util.spec_from_file_location('compat', Path(__file__).with_name('game-format-cross-version-check.py'))
compat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compat)

parser = argparse.ArgumentParser()
parser.add_argument('--old-helper', type=Path, required=True)
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--binary', type=Path)
args = parser.parse_args()
assert args.old_helper.is_file(), f"Expected pre-rename helper is missing: {args.old_helper}"

with tempfile.TemporaryDirectory(prefix='dlmm-legacy-restore-') as temp:
    for sharded in (False, True):
      for reapply in (False, True):
        fixture = FamilyUi(SimpleNamespace(helper=args.helper, binary=None, capture_dir=None), Path(temp) / f'{sharded}-{reapply}', 'legacy')
        old = compat.Helper(args.old_helper.resolve(), fixture.data)
        preview = old.ok('native.importPreview', statePath=str(fixture.source), gamePath=str(fixture.game))
        old.ok('native.importCommit', token=preview['token'], operationId='import', expectedRevision='0')
        originals = {str(p.relative_to(fixture.game)): p.read_bytes() for p in fixture.game.rglob('*') if p.is_file()}
        layout = {'deploymentLayout': 'shards-v1'} if sharded else {}
        for revision in (1, 2):
            preview = old.ok('native.applyPreview', profileId='default', gamePath=str(fixture.game), **layout)
            old.ok('native.apply', token=preview['token'], operationId=f'old-{revision}', expectedRevision=str(revision), **layout)
        old_document = compat.doc(fixture.data)
        assert old_document['applied']['manifest']['version'] == (3 if sharded else 2), old_document['applied']['manifest']
        print('PASS actual historical checkpoint format', old_document['applied']['manifest']['version'], flush=True)
        assert old.ok('native.snapshot')['canRestore'] is True
        assert old_document['previous']['applied'] is not None and 'restoreSemantics' not in old_document
        old.close()
        if args.binary and reapply and sharded:
            binaries=fixture.root/'fake-bin';binaries.mkdir()
            steam=binaries/'steam';log=fixture.root/'steam.json'
            steam.write_text('#!/usr/bin/python3\nimport json,sys\nfrom pathlib import Path\nPath('+repr(str(log))+').write_text(json.dumps(sys.argv[1:]))\n')
            steam.chmod(0o700)
            run=subprocess.run([str(args.binary),'--helper',str(args.helper),'--data-dir',str(fixture.data),'--check-ui','native-play'],env=fixture.env|{'PATH':str(binaries)+os.pathsep+os.environ['PATH'],'UI_NATIVE_CONFIRM_REVIEW':'1'},text=True,capture_output=True,timeout=60)
            assert run.returncode==0,run.stdout+run.stderr
            assert log.exists(), 'Play must dispatch to isolated Steam'
            assert not (fixture.game/'game/citadel/modlock').exists()
            print('PASS actual Play on two read-only legacy generations with hard links available',flush=True)

        helper = compat.Helper(args.helper.resolve(), fixture.data)
        snapshot = helper.ok('native.snapshot')
        assert snapshot['canRestore'] is True, snapshot
        assert helper.ok('native.applyPreview', restoreReview=True) == {'restoreReview': True}
        expected = old_document['previous']['gameinfo'].encode()
        revision = int(snapshot["revision"])
        if reapply:
            expected = fixture.info.read_bytes()
            old_generation = fixture.game / 'game/citadel/deadlock_native' / old_document['applied']['generation']
            old_file = next((old_generation / 'shard01' if sharded else old_generation).glob('*_dir.vpk'))
            old_inode = old_file.stat().st_ino
            preview = helper.ok('native.applyPreview', profileId='default', gamePath=str(fixture.game), deploymentLayout='shards-v1')
            helper.ok('native.apply', token=preview['token'], operationId='new-apply', expectedRevision=str(revision), deploymentLayout='shards-v1')
            revision += 1
            new_document = compat.doc(fixture.data)
            assert new_document['previous']['applied']['generation'] == old_document['applied']['generation']
            assert old_file.exists()
            new_generation = fixture.game / 'game/citadel/deadlock_native' / new_document['applied']['generation']
            new_inode = next((new_generation / 'shard01').glob('*_dir.vpk')).stat().st_ino
            assert (new_inode == old_inode) is sharded, 'flat originals must be copied; immutable shards may be linked'
            assert helper.ok('native.snapshot')['canRestore'] is True
        helper.ok('native.restore', operationId='restore', expectedRevision=str(revision))
        helper.close()
        assert fixture.info.read_bytes() == expected
        for name, content in originals.items():
            if name != 'game/citadel/gameinfo.gi':
                assert (fixture.game/name).read_bytes() == content, name
        print(f'PASS legacy Restore, shards={sharded}, incremental reapply={reapply}, originals intact', flush=True)

# Each checkpoint is written by the actual historical executable. Negative
# ownership/content cases must leave both the game and recovery evidence alone.
with tempfile.TemporaryDirectory(prefix='dlmm-flat-proof-') as temp:
  for case in ('original-restore', 'active-bytes', 'previous-bytes', 'foreign-member', 'owner', 'original-bytes'):
    f = FamilyUi(SimpleNamespace(helper=args.helper, binary=None, capture_dir=None), Path(temp)/case, case)
    original = {str(p.relative_to(f.game)): p.read_bytes() for p in f.game.rglob('*') if p.is_file()}
    old = compat.Helper(args.old_helper.resolve(), f.data)
    p = old.ok('native.importPreview', statePath=str(f.source), gamePath=str(f.game))
    old.ok('native.importCommit', token=p['token'], operationId='import', expectedRevision='0')
    for revision in range(1, 2 if case in ('original-restore', 'original-bytes') else 3):
        p = old.ok('native.applyPreview', profileId='default', gamePath=str(f.game))
        old.ok('native.apply', token=p['token'], operationId=f'old-{revision}', expectedRevision=str(revision))
    old.close()
    doc = compat.doc(f.data)
    assert doc['applied']['manifest']['version'] == 2
    workspace = f.game/'game/citadel/deadlock_native'
    if case in ('active-bytes', 'previous-bytes'):
        checkpoint = doc['applied'] if case == 'active-bytes' else doc['previous']['applied']
        target = next((workspace/checkpoint['generation']).glob('*_dir.vpk'))
        meta = target.stat()
        target.chmod(0o600)
        content = bytearray(target.read_bytes()); content[-1] ^= 1; target.write_bytes(content)
        os.utime(target, ns=(meta.st_atime_ns, meta.st_mtime_ns))
    elif case == 'foreign-member':
        (workspace/doc['previous']['applied']['generation']/'foreign.txt').write_bytes(b'keep unknown bytes')
    elif case == 'owner':
        (workspace/'owner.json').write_text(json.dumps({'version': 1, 'root': str(f.root/'foreign-library')}))
    elif case == 'original-bytes':
        target = f.game/'game/citadel/addons/family_dir.vpk'
        if not target.exists(): target = next((f.game/'game/citadel/addons').glob('*_dir.vpk'))
        target.chmod(0o600); target.write_bytes(b'changed original bytes')
    before = {str(p.relative_to(f.game)): p.read_bytes() for p in f.game.rglob('*') if p.is_file()}
    with sqlite3.connect(f.data/'library.sqlite3') as db:
        journals = db.execute('SELECT * FROM journals').fetchall()
    new = compat.Helper(args.helper.resolve(), f.data)
    if case == 'original-restore':
        assert new.ok('native.applyPreview', restoreReview=True) == {'restoreReview': True}
        new.ok('native.restore', operationId='restore-original', expectedRevision=doc['revision'])
        for name, content in original.items(): assert (f.game/name).read_bytes() == content, name
        print('PASS actual flat single-Apply Restore preserves exact original game and upstream files', flush=True)
    else:
        # The historical review is metadata-only; the transaction rechecks all bytes.
        assert new.ok('native.applyPreview', restoreReview=True) == {'restoreReview': True}
        for method, params in [('native.restore', {'operationId': 'refuse', 'expectedRevision': doc['revision']})]:
            result = new.call(method, **params)
            assert 'error' in result, (case, method, result)
            assert result['error']['data']['kind'] in ('externalChange', 'foreignWorkspace', 'externalGameinfoChange'), (case, result)
        assert compat.doc(f.data) == doc
        assert {str(p.relative_to(f.game)): p.read_bytes() for p in f.game.rglob('*') if p.is_file()} == before
        with sqlite3.connect(f.data/'library.sqlite3') as db: assert db.execute('SELECT * FROM journals').fetchall() == journals
        print('PASS actual flat checkpoint refuses changed proof and preserves all bytes:', case, flush=True)
    new.close()
