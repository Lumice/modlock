#!/usr/bin/env python3
import shutil
"""Native UI workflows against real helper/SQLite/VPK bytes in disposable roots."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import struct
import subprocess
import tempfile
import selectors
import time

BEFORE = b'// preserve exact\r\n"GameInfo"\r\n{\r\n FileSystem\r\n {\r\n  SearchPaths\r\n  {\r\n   Game citadel/addons // old\r\n   Game_LowViolence citadel_lv\r\n   Game citadel\r\n   Mod citadel\r\n   Game core\r\n  }\r\n }\r\n}\r\n'


def logical_document(raw):
    if raw.get('storeVersion') == 5:
        version=raw.pop('storageDocumentVersion')
        raw['storeVersion']=raw.pop('gameDocumentVersion') if version==5 else version
    return raw


def vpk():
    tree = b'vmdl_c\0models\0tiny\0' + struct.pack('<IHHIIH', 7, 0, 0x7fff, 0, 0, 0xffff) + b'\0\0\0'
    return struct.pack('<III', 0x55aa1234, 1, len(tree)) + tree


def fingerprints(paths):
    return {str(p): (hashlib.sha256(p.read_bytes()).hexdigest(), p.stat().st_mtime_ns) if p.exists() else None for p in paths}


def family_vpk(disabled=False):
    entries = [('models/a.vmdl_c', 9 if disabled else 7, 0, 0, 3, b'pre'),
               ('materials/a.vmat_c', 8, 17, 0, 4, b''),
               ('textures/inline.vtex_c', 10, 0x7fff, 0, 4, b'i'),
               ('sounds/preload.vsnd_c', 11, 65535, 0xffffffff, 0, b'only')]
    tree = b''
    for path, crc, archive, offset, length, preload in entries:
        directory, file = path.rsplit('/', 1); name, extension = file.rsplit('.', 1)
        tree += extension.encode() + b'\0' + directory.encode() + b'\0' + name.encode() + b'\0'
        tree += struct.pack('<IHHIIH', crc, len(preload), archive, offset, length, 0xffff) + preload + b'\0\0'
    tree += b'\0'
    return struct.pack('<IIIIIII', 0x55aa1234, 2, len(tree), 4, 0, 0, 0) + tree + b'body'


class FamilyUi:
    def __init__(self, args, root, label, missing=False):
        self.args, self.root, self.label = args, root, label
        self.source = root / 'source/state.json'; self.game = root / 'steam/Deadlock'; self.data = root / 'native'
        self.addons = self.game / 'game/citadel/addons'; self.source.parent.mkdir(parents=True); self.addons.mkdir(parents=True)
        self.info = self.game / 'game/citadel/gameinfo.gi'; self.info.write_bytes(BEFORE)
        self.active, self.disabled = family_vpk(), family_vpk(True)
        names = ['renamed-directory.vpk', 'unrelated-a.vpk', 'unrelated-b.vpk']
        disabled_names = ['remote-b_other_dir.vpk', 'remote-b_other_000.vpk', 'remote-b_other_017.vpk']
        self.paths = []
        for group, directory in [(names, self.active), (disabled_names, self.disabled)]:
            for name, content in zip(group, [directory, b'one', b'two!']):
                path = self.addons / name; path.write_bytes(content); self.paths.append(path)
        if missing: self.paths[2].unlink()
        manifest = self.addons / '.dmm.json'
        manifest.write_text(json.dumps({'version': 3, 'mods': {
            'remote-a': {'enabled': True, 'order': 0, 'shard': 1, 'currentVpks': names, 'disabledVpks': [], 'originalVpkNames': ['skin_dir.vpk', 'skin_000.vpk', 'skin_017.vpk']},
            'remote-b': {'enabled': False, 'order': 1, 'shard': 1, 'currentVpks': [], 'disabledVpks': disabled_names, 'originalVpkNames': ['other_dir.vpk', 'other_000.vpk', 'other_017.vpk']}}}))
        unknown = self.addons / 'unknown.txt'; unknown.write_bytes(b'preserve unknown')
        mods = [{'id': 'local-a', 'remoteId': 'remote-a', 'name': '<b>family</b>', 'author': '<b>author</b>', 'status': 'installed', 'installOrder': 0},
                {'id': 'local-b', 'remoteId': 'remote-b', 'name': 'Disabled family', 'status': 'disabled', 'installOrder': 1},
                {'id': 'missing', 'remoteId': 'missing', 'name': 'Missing fixture', 'status': 'disabled'}]
        self.source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': mods, 'profiles': {'default': {'name': 'Default', 'mods': mods}}}}}))
        self.paths += [self.source, manifest, unknown]
        self.originals = fingerprints(self.paths)
        self.env = os.environ | {'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software', 'QT_FORCE_STDERR_LOGGING': '1', 'QT_LOGGING_RULES': '*.debug=false', 'XDG_CACHE_HOME': str(root / 'cache'), 'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_DATA_HOME': str(root / 'xdg-data'), 'UI_NATIVE_GAME': str(self.game), 'UI_NATIVE_SOURCE': str(self.source), 'UI_NATIVE_FAMILY': '1', 'UI_NATIVE_AVAILABLE': '1' if missing else '2', 'UI_NATIVE_UNAVAILABLE': '2' if missing else '1', 'UI_NATIVE_IMPORT_COUNTS': f'1,1,3,2,{len(self.disabled)+7}' if missing else f'2,2,4,2,{len(self.active)+len(self.disabled)+7}', 'UI_NATIVE_APPLY_COUNTS': f'1,1,3,2,{len(self.active)+7}'}
        self.env['UI_NATIVE_IMPORT_SUMMARY'] = '1 package · 1 unique directory VPKs · 2 companion files · 3 files to copy' if missing else '2 packages · 2 unique directory VPKs · 2 companion files · 4 files to copy'
        self.env['UI_NATIVE_APPLY_SUMMARY'] = '1 selected package choices · 1 unique directory VPKs · 2 companion files · 3 files to copy'
        if missing: self.env['UI_NATIVE_MISSING'] = '1'

    def document(self):
        with sqlite3.connect(self.data / 'library.sqlite3') as db:
            return logical_document(json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0]))

    def marker(self):
        return self.root / 'config/dl-mod-manager/state/pending' / (hashlib.sha256(str(self.data).encode()).hexdigest() + '.json')

    def run(self, scenario, source=False, change=None, helper=None):
        env = self.env.copy()
        if self.args.capture_dir:
            captures = self.args.capture_dir / self.label / scenario; captures.mkdir(parents=True, exist_ok=True)
            env['UI_NATIVE_CAPTURES'] = str(captures.resolve())
        command = [str(self.args.binary.resolve()), '--helper', str((helper or self.args.helper).resolve() if helper is None else helper), '--data-dir', str(self.data), '--check-ui', scenario]
        if source: command += ['--source', str(self.source)]
        if change:
            ack = self.root / 'mutation-ack'; env['UI_NATIVE_MUTATION_ACK'] = str(ack)
            process = subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            prefix = ''
            try:
                with selectors.DefaultSelector() as selector:
                    selector.register(process.stdout, selectors.EVENT_READ)
                    deadline = time.monotonic() + 20
                    while 'READY_FOR_FAMILY_APPLY_MUTATION' not in prefix:
                        assert time.monotonic() < deadline, 'family mutation handshake timed out'
                        if selector.select(.1):
                            line = process.stdout.readline()
                            assert line, 'UI exited before review handshake'
                            prefix += line
                change(); ack.touch()
                out, err = process.communicate(timeout=20)
                output = prefix + out + err
                assert process.returncode == 0 and 'PASS UI' in output, output
            finally:
                if process.poll() is None: process.kill(); process.communicate()
        else:
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=30)
            assert result.returncode == 0 and 'PASS UI' in result.stdout + result.stderr, f'{self.label}/{scenario}: {result.returncode}\n{result.stdout}{result.stderr}'
        assert fingerprints(self.paths) == self.originals, f'{self.label}/{scenario}: source evidence changed'
        print(f'PASS native integration {self.label}/{scenario}', flush=True)

    def owned(self):
        doc = self.document(); assert doc['storeVersion'] == 2
        mods = doc['profiles']['default']['mods']
        assert sorted(m['available'] for m in mods) == [False, True, True]
        by_id = {m['source']['id']: m for m in mods}
        assert by_id['local-a']['enabled'] and not by_id['local-b']['enabled']
        assert not by_id['missing']['available'] and not by_id['missing']['packages']
        files = {}
        for mod_id, directory in [('local-a', self.active), ('local-b', self.disabled)]:
            packages = by_id[mod_id]['packages']; assert len(packages) == 1
            package = packages[0]; assert [c['archiveIndex'] for c in package['companions']] == [0, 17]
            for reference, content in [(package['directory'], directory)] + [(c['file'], data) for c, data in zip(package['companions'], [b'one', b'two!'])]:
                assert reference['sha256'] == hashlib.sha256(content).hexdigest() and reference['size'] == len(content)
                files[reference['id'] + '.vpk'] = content
        assert len(files) == 4
        actual = {p.name: p.read_bytes() for p in (self.data / 'payloads').iterdir()}
        assert actual == files, 'immutable member union/content mismatch'
        return by_id



def family_checks(args, root):
    family = FamilyUi(args, root / 'family', 'family')
    family.run('native-controls-source', True); family.run('native-import-cancel', True)
    assert not (family.data / 'library.sqlite3').exists() and family.info.read_bytes() == BEFORE
    family.run('native-import', True); family.owned(); family.run('native-controls-native'); family.run('native-edit')
    doc = family.document(); copied = next(p for p in doc['profiles'].values() if p['name'] == 'Renamed fixture')
    assert copied['id'] != 'default' and {m['source']['id'] for m in copied['mods']} == {'local-a', 'local-b', 'missing'}
    family.owned(); family.run('native-apply-cancel')
    assert family.document()['applied'] is None and family.info.read_bytes() == BEFORE
    assert not (family.game / 'game/citadel/deadlock_native').exists()
    family.run('native-apply'); doc = family.document(); assert doc['applied']['profileId'] == copied['id']
    generation = family.game / 'game/citadel/deadlock_native' / doc['applied']['generation']
    assert {p.name for p in generation.iterdir()} == {'shard01', 'manifest.json'}
    assert {p.name for p in (generation / 'shard01').iterdir()} == {'pak01_dir.vpk', 'pak01_000.vpk', 'pak01_017.vpk'}
    for name, content in [('pak01_dir.vpk', family.active), ('pak01_000.vpk', b'one'), ('pak01_017.vpk', b'two!')]: assert (generation / 'shard01' / name).read_bytes() == content
    assert json.loads((generation / 'manifest.json').read_text())['version'] == 3
    family.run('native-snapshot'); family.run('native-controls-applied'); family.run('native-restore'); family.run('native-snapshot-restored')
    assert family.info.read_bytes() == BEFORE and family.document()['applied'] is None
    assert next(m for m in family.document()['profiles'][copied['id']]['mods'] if m['source']['id'] == 'local-a')['enabled']
    family.run('native-source', True)

    missing = FamilyUi(args, root / 'missing', 'missing', missing=True)
    missing.run('native-import-cancel', True)
    assert not (missing.data / 'library.sqlite3').exists() and missing.info.read_bytes() == BEFORE

    changed = FamilyUi(args, root / 'changed', 'changed')
    changed.run('native-import', True); changed.run('native-edit')
    def replace_companion():
        package = next(m for m in changed.document()['profiles']['default']['mods'] if m['source']['id'] == 'local-a')['packages'][0]
        payload = changed.data / 'payloads' / (package['companions'][1]['file']['id'] + '.vpk')
        saved = changed.root / 'original-inode'; payload.rename(saved); payload.write_bytes(saved.read_bytes()); payload.chmod(0o400)
        assert saved.stat().st_ino != payload.stat().st_ino
    changed.run('native-apply-changed', change=replace_companion)
    assert changed.info.read_bytes() == BEFORE and changed.document()['applied'] is None
    assert not (changed.game / 'game/citadel/deadlock_native').exists()
    with sqlite3.connect(changed.data / 'library.sqlite3') as db: assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0

    recovery = FamilyUi(args, root / 'recovery', 'recovery')
    recovery.run('native-import', True)
    initial = recovery.document(); revision = initial['revision']; before = recovery.info.read_bytes()
    meta = recovery.info.stat()
    before_identity = dict(device=meta.st_dev, inode=meta.st_ino, size=meta.st_size, modifiedSeconds=meta.st_mtime_ns//10**9, modifiedNanos=meta.st_mtime_ns%10**9)
    # Reviews are session-local: preserve the preview and Apply in the same helper.
    process = subprocess.Popen([str(args.helper.resolve())], env=recovery.env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    def call(method, params):
        process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': params | {'dataDir': str(recovery.data)}}) + '\n'); process.stdin.flush()
        response = json.loads(process.stdout.readline()); assert 'error' not in response, response
        return response['result']
    try:
        preview = call('native.applyPreview', {'profileId': 'default', 'gamePath': str(recovery.game), 'deploymentLayout': 'shards-v1'})
        params = {'token': preview['token'], 'expectedRevision': revision, 'operationId': 'interrupted-family-apply', 'deploymentLayout': 'shards-v1'}
        call('native.apply', params)
        process.stdin.close(); process.wait(timeout=15); assert process.returncode == 0
    finally:
        if process.poll() is None: process.kill(); process.wait()
    document = recovery.document(); after = recovery.info.read_bytes(); applied = document['applied']
    # Recreate the current durable post-rename boundary from actual writer
    # output. Historical flat recovery is covered by the real-old-helper suite.
    projected = json.loads(json.dumps(document)); projected['revision'] = revision
    nodes = []
    for name, owner in [('', applied['directoryIdentity']), *applied['shardIdentities'].items()]:
        nodes.append(dict(kind='directory', name=name, device=owner['device'], inode=owner['inode'], **({'fsid': owner['fsid']} if 'fsid' in owner else {})))
    for name, owner in applied['identities'].items():
        nodes.append(dict(kind='file', name=name, device=owner['device'], inode=owner['inode'], complete=True, **({'fsid': owner['fsid']} if 'fsid' in owner else {})))
    # Persisted identity object ordering is canonical, while journal staging
    # order follows directory packages then their companions and manifest.
    names = ['', *applied['shardIdentities']]
    for shard in applied['manifest']['shards']:
        for slot, package in enumerate(shard['packages'], 1):
            names.append(f"shard{shard['index']:02}/pak{slot:02}_dir.vpk")
            names.extend(f"shard{shard['index']:02}/pak{slot:02}_{member['archiveIndex']:03}.vpk" for member in package['companions'])
    names.append('manifest.json'); nodes.sort(key=lambda row: names.index(row['name']))
    meta = recovery.info.stat()
    temp_identity = dict(device=meta.st_dev, inode=meta.st_ino, size=0, modifiedSeconds=meta.st_mtime_ns//10**9, modifiedNanos=meta.st_mtime_ns%10**9)
    journal = dict(kind='game', version=5, expectedRevision=revision, input={'method': 'native.apply', 'params': params},
                   citadel=str(recovery.game/'game/citadel'), before=before.decode(), after=after.decode(),
                   beforeSha256=hashlib.sha256(before).hexdigest(), afterSha256=hashlib.sha256(after).hexdigest(),
                   beforeIdentity=before_identity, beforeDocument=initial, document=projected,
                   workspaceExisted=False, temp='.native-gameinfo-'+'a'*32, generation=applied['generation'],
                   libraryVersionBefore=initial['storeVersion'], libraryVersionAfter=4,
                   packageVersion=initial.get('packageVersion', initial['storeVersion']),
                   fontStateDigest=hashlib.sha256(json.dumps(dict(version=1,installed=None,previous=None),sort_keys=True,separators=(',', ':')).encode()).hexdigest(),
                   deploymentIntent=applied['deploymentIntent'], desiredSignature=applied['desiredSignature'],
                   stagedNodes=nodes, tempIdentity=temp_identity)

    with sqlite3.connect(recovery.data / 'library.sqlite3') as db:
        db.execute('UPDATE library SET body=? WHERE id=1', (json.dumps(initial),)); db.execute('DELETE FROM operations WHERE id=?', (params['operationId'],)); db.execute('INSERT INTO journals(id,body) VALUES(?,?)', (params['operationId'], json.dumps(journal)))
    marker = recovery.marker(); marker.parent.mkdir(parents=True, exist_ok=True); marker.write_text(json.dumps({'operationId': params['operationId'], 'method': 6}))
    # Current general recovery records its receipt without a no-op revision bump.
    recovery.env['UI_NATIVE_RECOVERED_REVISION'] = str(int(revision)+1)
    recovery.run('native-family-recover')
    assert recovery.info.read_bytes() == after and recovery.document()['revision'] == str(int(revision)+1)
    assert json.loads(marker.read_text()) == {} and recovery.document()['applied'] == applied
    with sqlite3.connect(recovery.data / 'library.sqlite3') as db:
        assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0
        outcomes = {key: json.loads(body) for key, body in db.execute('SELECT id,result FROM operations')}
        assert outcomes[params['operationId']]['status'] == 'succeeded'
        assert len(outcomes) == 3, 'Apply was replayed or recovery outcome missing'
    recovery.run('native-snapshot')

    legacy = FamilyUi(args, root / 'legacy', 'legacy')
    # Exact frozen pre-Store2 document, not derived from the current importer.
    legacy_body = r'{"storeVersion":1,"revision":"1","sourcePath":"/legacy/state.json","gamePath":"","activeProfileId":"default","profiles":{"default":{"id":"default","name":"Default","protected":true,"unknown":{"keep":true},"mods":[{"source":{"id":"old","remoteId":"old","name":"Old unavailable family","status":"installed","unknown":{"keep":true}},"enabled":true,"available":false,"unavailableReason":"External chunks unsupported","previewHidden":true,"payloads":[]}]}},"importArchive":{"unknown":{"keep":true}},"importLedger":{"sourcePayloads":[]},"applied":null,"canRestore":false,"recoveryRequired":false,"unknown":{"keep":true}}'
    legacy.data.mkdir()
    with sqlite3.connect(legacy.data / 'library.sqlite3') as db:
        db.execute('CREATE TABLE library(id INTEGER PRIMARY KEY CHECK(id=1),body TEXT NOT NULL)'); db.execute('CREATE TABLE operations(id TEXT PRIMARY KEY,input TEXT NOT NULL,result TEXT NOT NULL)'); db.execute('CREATE TABLE journals(id TEXT PRIMARY KEY,body TEXT NOT NULL)'); db.execute('INSERT INTO library VALUES(1,?)', (legacy_body,))
    before_db = (legacy.data / 'library.sqlite3').read_bytes()
    legacy.run('native-legacy-view'); assert (legacy.data / 'library.sqlite3').read_bytes() == before_db
    legacy.run('native-legacy-edit'); doc = legacy.document()
    assert doc['storeVersion'] == 1 and doc['unknown']['keep'] and doc['importArchive']['unknown']['keep']
    assert len(doc['profiles']) == 2 and any(p['name'] == 'Renamed legacy' for p in doc['profiles'].values())
    for profile in doc['profiles'].values():
        mod = profile['mods'][0]; assert mod['payloads'] == [] and 'packages' not in mod and not mod['available'] and not mod['enabled'] and mod['source']['unknown']['keep']
    # Historical Store1 data stays readable/editable above. A fully admitted peer
    # may still return a bad wire summary: import keeps its legacy data shape,
    # while a legacy Apply summary cannot satisfy an explicit sharded request.
    for kind in ['import', 'apply']:
        compatible = FamilyUi(args, root / ('legacy-review-' + kind), 'legacy-review-' + kind)
        compatible.data.mkdir()
        body = {'token': 'legacy', 'gamePath': '/synthetic/game', 'totalBytes': 42, 'warnings': []}
        if kind == 'import':
            body |= {'profileCount': 1, 'modCount': 1, 'availableModCount': 1, 'unavailableModCount': 0, 'profiles': [], 'unavailableMods': [], 'summariesTruncated': False}
        else: body |= {'profileId': 'default', 'fileCount': 7, 'replacedMounts': []}
        (compatible.data / 'review.json').write_text(json.dumps(body))
        peer = compatible.root / ('native-family-legacy-' + kind); peer.symlink_to(Path(__file__).resolve().with_name('controlled-helper.py'))
        compatible.run('native-legacy-review-' + kind, source=kind == 'import', helper=peer)
    print(f'PASS family counts import=2/2/4/2 bytes={len(family.active)+len(family.disabled)+7}; Apply=1/1/3/2 bytes={len(family.active)+7}', flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--helper', type=Path, required=True)
    parser.add_argument('--capture-dir', type=Path, default=os.environ.get('UI_NATIVE_CAPTURE_ROOT'))
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='modlock-ui-') as tmp:
        root = Path(tmp); source = root / 'source/state.json'; game = root / 'steam/Deadlock'
        data = root / 'native'; addons = game / 'game/citadel/addons'
        source.parent.mkdir(); addons.mkdir(parents=True)
        (game / 'game/bin/win64').mkdir(parents=True)
        (game / 'game/citadel/pak01_dir.vpk').write_bytes(vpk())
        payload = addons / 'pak01_dir.vpk'; payload.write_bytes(vpk())
        unknown = addons / 'unknown.txt'; unknown.write_bytes(b'preserve unknown')
        manifest = addons / '.dmm.json'
        manifest.write_text(json.dumps({'version': 3, 'mods': {'remote-a': {'enabled': True, 'order': 0, 'shard': 1, 'currentVpks': ['pak01_dir.vpk'], 'disabledVpks': [], 'originalVpkNames': ['a_dir.vpk']}}}))
        mod = {'id': 'local-a', 'remoteId': 'remote-a', 'name': 'Tiny fixture', 'status': 'installed', 'installOrder': 0}
        unavailable = {'id': 'missing', 'remoteId': 'missing', 'name': 'Missing fixture', 'status': 'disabled'}
        source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'gamePath': '/portal/path/do-not-translate', 'localMods': [mod, unavailable], 'profiles': {'default': {'name': 'Default', 'mods': [mod, unavailable]}}}}}))
        info = game / 'game/citadel/gameinfo.gi'; info.write_bytes(BEFORE)
        originals = fingerprints([source, payload, manifest, unknown])
        env = os.environ | {'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software', 'QT_FORCE_STDERR_LOGGING': '1', 'QT_LOGGING_RULES': '*.debug=false', 'XDG_CACHE_HOME': str(root / 'cache'), 'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_DATA_HOME': str(root / 'xdg-data'), 'UI_NATIVE_GAME': str(game), 'UI_NATIVE_SOURCE': str(source)}
        env |= {'UI_NATIVE_IMPORT_COUNTS': f'1,1,1,0,{len(vpk())}', 'UI_NATIVE_APPLY_COUNTS': f'1,1,1,0,{len(vpk())}', 'UI_NATIVE_IMPORT_SUMMARY': '1 package · 1 unique directory VPKs · 0 companion files · 1 file to copy', 'UI_NATIVE_APPLY_SUMMARY': '1 selected package choices · 1 unique directory VPKs · 0 companion files · 1 file to copy'}
        def run(scenario, source_mode=False, override_helper=None, extra_env=None):
            if args.capture_dir:
                captures = args.capture_dir / 'standalone' / scenario; captures.mkdir(parents=True, exist_ok=True)
                env['UI_NATIVE_CAPTURES'] = str(captures.resolve())
            command = [str(args.binary.resolve()), '--helper', str((override_helper or args.helper).resolve()) if override_helper is None else str(override_helper), '--data-dir', str(data), '--check-ui', scenario]
            if source_mode: command += ['--source', str(source)]
            result = subprocess.run(command, env=env | (extra_env or {}), capture_output=True, text=True, timeout=30)
            assert result.returncode == 0 and 'PASS UI' in result.stdout + result.stderr, f'{scenario}: {result.returncode}\n{result.stdout}{result.stderr}'
            assert fingerprints([source, payload, manifest, unknown]) == originals, f'{scenario}: source payload changed'
            print(f'PASS native integration {scenario}')
        def document():
            with sqlite3.connect(data / 'library.sqlite3') as db:
                return logical_document(json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0]))
        run('native-controls-source', True)
        run('native-import-cancel', True)
        assert not (data / 'library.sqlite3').exists() and info.read_bytes() == BEFORE
        pending = root / 'config/dl-mod-manager/state/pending'
        pending.mkdir(parents=True, exist_ok=True)
        marker = pending / (hashlib.sha256(str(data).encode()).hexdigest() + '.json')
        # Recovery inspection requires an admitted history schema, even when no
        # library document has ever been committed.
        data.mkdir(exist_ok=True)
        with sqlite3.connect(data / 'library.sqlite3') as db:
            db.execute('CREATE TABLE library (id INTEGER PRIMARY KEY CHECK(id=1), body TEXT NOT NULL)')
            db.execute('CREATE TABLE operations (id TEXT PRIMARY KEY, input TEXT NOT NULL, result TEXT NOT NULL)')
            db.execute('CREATE TABLE journals (id TEXT PRIMARY KEY, body TEXT NOT NULL)')
        marker.write_text(json.dumps({'operationId': 'frontend-before-dispatch', 'method': 2}))
        run('native-recovery-no-doc', True)
        with sqlite3.connect(data / 'library.sqlite3') as db:
            # A fresh writer must protect even a pre-import journal from older helpers.
            row = db.execute('SELECT body FROM library WHERE id=1').fetchone()
            assert row is not None and json.loads(row[0]) == dict(storeVersion=5, freshImport=True)
            assert db.execute('SELECT count(*) FROM operations').fetchone()[0] == 1
        assert json.loads(marker.read_text()) == {'operationId': 'frontend-before-dispatch', 'method': 2} and info.read_bytes() == BEFORE
        marker.unlink()  # End this synthetic unknown-identity fixture before the next import.
        run('native-import', True)
        doc = document(); assert doc['revision'] == '1' and doc['applied'] is None and doc['storeVersion'] == 2
        owned = list((data / 'payloads').glob('*.vpk'))
        assert len(owned) == 1 and owned[0].read_bytes() == vpk()
        run('native-controls-native')
        run('native-edit')
        doc = document(); copied = [p for p in doc['profiles'].values() if p['name'] == 'Renamed fixture']
        assert len(copied) == 1 and copied[0]['mods'][0]['enabled'] is True
        assert len(doc['profiles']) == 2 and info.read_bytes() == BEFORE
        run('native-maintenance')
        run('native-error')
        assert info.read_bytes() == BEFORE
        run('native-apply')
        doc = document(); applied = doc['applied']; assert applied and doc['canRestore'] and applied['profileId'] == copied[0]['id']
        generation = game / 'game/citadel/deadlock_native' / applied['generation']
        assert (generation / 'shard01/pak01_dir.vpk').read_bytes() == vpk()
        assert info.read_bytes() != BEFORE and b'Game_LowViolence citadel_lv\r\n' in info.read_bytes()
        run('native-controls-applied')
        fake_bin = root / 'fake-bin'; fake_bin.mkdir()
        steam_log = root / 'steam-calls'
        fake_steam = fake_bin / 'steam'
        fake_steam.write_text('#!/bin/sh\nprintf "%s\\n" "$#" "$@" >> "$FAKE_STEAM_LOG"\n')
        fake_steam.chmod(0o755)
        def play_fingerprints():
            files = [info] + sorted(p for base in (data / 'payloads', generation) for p in base.rglob('*') if p.is_file())
            return fingerprints(files)
        before_play = play_fingerprints()
        run('native-play', extra_env={'PATH': str(fake_bin) + os.pathsep + env['PATH'], 'FAKE_STEAM_LOG': str(steam_log)})
        assert steam_log.read_text().splitlines() == ['1', 'steam://run/1422450//'], 'Play did not dispatch exactly one Steam URI'
        assert play_fingerprints() == before_play, 'Play changed gameinfo or payload fingerprints'
        marker.write_text(json.dumps({'operationId': 'source-before-dispatch', 'method': 3}))
        run('native-recovery-source', True)
        assert document()['applied']['profileId'] == applied['profileId']
        assert json.loads(marker.read_text()) == {'operationId': 'source-before-dispatch', 'method': 3}
        marker.unlink()  # The next scenario starts without the synthetic unknown identity.
        run('native-snapshot')
        run('native-restore')
        assert info.read_bytes() == BEFORE and document()['applied'] is None
        run('native-snapshot-restored')
        # Item 45: simulate the old manager editing a restored checkpoint.
        with sqlite3.connect(data / 'library.sqlite3') as db:
            raw=json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
            raw['importLedger']['manifests']=[]
            db.execute('UPDATE library SET body=? WHERE id=1',(json.dumps(raw),))
        info.write_bytes(BEFORE.replace(b'citadel/addons',b'citadel/addons_foreign'))
        run('native-item45-error')
        info.write_bytes(b'// old manager re-added mount\r\n'+BEFORE)
        takeover_before=info.read_bytes()
        play_revision=document()['revision']
        run('native-item47-play', extra_env={'PATH': str(fake_bin) + os.pathsep + env['PATH'], 'FAKE_STEAM_LOG': str(steam_log)})
        assert steam_log.read_text().splitlines() == ['1', 'steam://run/1422450//'], 'Play dispatched Steam before takeover review'
        assert info.read_bytes()==takeover_before and document()['revision']==play_revision
        with sqlite3.connect(data / 'library.sqlite3') as db:
            assert db.execute('SELECT count(*) FROM journals').fetchone()[0]==0
        run('native-item45-import')
        run('native-item45')
        assert b'citadel/addons' not in info.read_bytes()
        run('native-restore')
        assert info.read_bytes()==takeover_before
        # Folder-change commit response is also exercised by backend unit tests.
        moved = root / 'MovedDeadlock'
        shutil.copytree(game,moved)
        request={'jsonrpc':'2.0','id':1,'method':'native.profile','params':{'dataDir':str(data),'action':'useGameFolder','gamePath':str(moved),'operationId':'item47-folder','expectedRevision':document()['revision']}}
        reply=subprocess.run([str(args.helper.resolve())],input=json.dumps(request)+'\n',env=env,text=True,capture_output=True,timeout=10)
        assert reply.returncode==0 and '"error"' not in reply.stdout,reply.stdout+reply.stderr
        run('native-item47-restore-reason')
        # Return fixture to its earlier state for the remaining scenarios.
        with sqlite3.connect(data / 'library.sqlite3') as db:
            raw=json.loads(db.execute('SELECT body FROM library WHERE id=1').fetchone()[0])
            raw=raw['previousGameFolders'][-1]
            db.execute('UPDATE library SET body=? WHERE id=1',(json.dumps(raw),))
        # Explicit upstream route remains read-only even with a populated native library.
        run('native-source', True)
        assert document()['applied'] is None
        delayed = root / 'native-delay'
        delayed.symlink_to(Path(__file__).resolve().with_name('controlled-helper.py'))
        run('native-close', override_helper=delayed)
        family_checks(args, root / 'families')
        print('PASS native integration all checks')

if __name__ == '__main__':
    main()
