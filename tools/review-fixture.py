#!/usr/bin/env python3
"""Build a disposable, non-demo review library. Never use as a migration tool."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import shlex
import shutil
import sqlite3
import struct
import subprocess
import tempfile

PAGES = 'select settings tweaks autoexec crosshair browse-item apply profiles installed bisect-setup'.split()
MARKER = '.deadlock-review-fixture'


def dump(value):
    return json.dumps(value, ensure_ascii=False, separators=(',', ':'))


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def identity(path):
    s = path.stat()
    return dict(size=s.st_size, device=s.st_dev, inode=s.st_ino,
                modifiedSeconds=s.st_mtime_ns // 10**9, modifiedNanos=s.st_mtime_ns % 10**9,
                mode=s.st_mode, changedSeconds=s.st_ctime_ns // 10**9,
                changedNanos=s.st_ctime_ns % 10**9)


def regular(path):
    if path.is_symlink() or any(p.is_symlink() for p in path.parents):
        raise ValueError(f'Symlink source refused: {path}')
    if not path.is_file():
        raise ValueError(f'Expected regular file: {path}')


def copy(src, dst):
    regular(src)
    dst.parent.mkdir(parents=True, exist_ok=True)
    # Reflink when supported; otherwise physical copy. Never hardlink sources.
    subprocess.run(['cp', '--reflink=auto', '--', str(src), str(dst)], check=True)
    dst.chmod(0o600)


def backup(src, dst):
    regular(src)
    # Immutable avoids even SQLite sidecar creation on the read-only source.
    # Refuse live WAL rather than silently omit its committed records.
    wal = Path(str(src) + '-wal')
    if wal.exists() and wal.stat().st_size:
        raise ValueError(f'Close the source app and checkpoint its WAL first: {src}')
    dst.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(src.as_uri() + '?mode=ro&immutable=1', uri=True) as source:
        with sqlite3.connect(dst) as target:
            source.backup(target)


class Fixture:
    def __init__(self, root, library, game, include_nsfw_artwork=False):
        self.root, self.library, self.game = root, library, game
        self.data = root / 'data' / 'deadlock-native'
        self.fake = root / 'game' / 'Deadlock'
        self.citadel = self.fake / 'game' / 'citadel'
        self.generations = {}
        self.include_nsfw_artwork = include_nsfw_artwork

    def rebase(self, value):
        if isinstance(value, str):
            if value.startswith('/') and '\n' not in value and '..' in Path(value).parts:
                raise ValueError('Traversal source path refused')
            for old, new in [(self.library, self.data), (self.game, self.fake)]:
                if value == str(old) or value.startswith(str(old) + '/'):
                    return str(new) + value[len(str(old)):]
            # Historical import sources can be outside the two roots. Keep them
            # inert and inside the fixture, without reading arbitrary originals.
            if value.startswith('/') and '\n' not in value:
                return str(self.root / 'unavailable-sources') + value
            return value
        if isinstance(value, list):
            return [self.rebase(v) for v in value]
        if isinstance(value, dict):
            return {k: self.rebase(v) for k, v in value.items()}
        return value

    def generation(self, checkpoint):
        manifest = checkpoint['manifest']
        name = manifest['generation']
        if not re.fullmatch(r'[a-f0-9]{32}', name):
            raise ValueError('Unsafe generation ID')
        directory = self.citadel / 'deadlock_native' / name
        if name not in self.generations:
            directory.mkdir(parents=True)
            version = manifest['version']
            if version not in (1, 2, 3):
                raise ValueError('Unsupported generation version')
            if version == 3 and any(s['index'] != i for i, s in enumerate(manifest['shards'], 1)):
                raise ValueError('Invalid shard indexes')
            groups = [(f"shard{s['index']:02}", s['packages']) for s in manifest['shards']] if version == 3 else [('', manifest['files' if version == 1 else 'packages'])]
            files = []
            for prefix, rows in groups:
                (directory / prefix).mkdir(exist_ok=True)
                for slot, row in enumerate(rows, 1):
                    ref = row if version == 1 else row['directory']
                    members = [(f'pak{slot:02}_dir.vpk', ref)]
                    members += [(f"pak{slot:02}_{c['archiveIndex']:03}.vpk", c['file']) for c in row.get('companions', [])]
                    for filename, payload in members:
                        relative = str(Path(prefix) / filename)
                        if not re.fullmatch(r'[a-f0-9]{32}', payload['id']):
                            raise ValueError('Unsafe payload ID')
                        src = self.data / 'payloads' / (payload['id'] + '.vpk')
                        if digest(src) != payload['sha256'] or src.stat().st_size != payload['size']:
                            raise ValueError(f'Payload differs from checkpoint: {src}')
                        copy(src, directory / relative)
                        files.append(relative)
            shards = [p for p, _ in groups if p]
            # Health-check repair overlay: one extra VPK in repairs/.
            repair = (manifest.get('repairs') or {}).get('file') if version == 3 else None
            if repair:
                if not re.fullmatch(r'[a-f0-9]{64}', repair['sha256']):
                    raise ValueError('Unsafe repair hash')
                src = self.data / 'cache' / 'repairs' / (repair['sha256'] + '.vpk')
                if not src.exists():
                    copy(self.library / 'cache' / 'repairs' / src.name, src)
                if digest(src) != repair['sha256'] or src.stat().st_size != repair['size']:
                    raise ValueError(f'Payload differs from checkpoint: {src}')
                (directory / 'repairs').mkdir()
                copy(src, directory / 'repairs' / 'pak01_dir.vpk')
                files.append('repairs/pak01_dir.vpk')
                shards.append('repairs')
            (directory / 'manifest.json').write_text(dump(manifest))
            self.generations[name] = (manifest, files, shards)
        saved, files, shards = self.generations[name]
        if saved != manifest:
            raise ValueError(f'Conflicting manifests for generation {name}')
        checkpoint['identities'] = {p: {k: v for k, v in identity(directory / p).items()
                                     if k in ('size', 'device', 'inode', 'modifiedSeconds', 'modifiedNanos')}
                                    for p in files + ['manifest.json']}
        if manifest['version'] >= 2:
            checkpoint['directoryIdentity'] = {k: v for k, v in identity(directory).items() if k in checkpoint['identities']['manifest.json']}
        if manifest['version'] == 3:
            checkpoint['shardIdentities'] = {p: {k: v for k, v in identity(directory / p).items() if k in checkpoint['directoryIdentity']} for p in shards}

    def materialize_origins(self, value):
        if isinstance(value, list):
            for child in value:
                self.materialize_origins(child)
        elif isinstance(value, dict):
            for origin in value.get('installedGameOrigins', []):
                for proof in origin['files']:
                    path = Path(proof['sourcePath'])
                    if proof.get('state') == 'present' and path.is_relative_to(self.fake) and path.suffix == '.vpk':
                        path.parent.mkdir(parents=True, exist_ok=True)
                        if not path.exists():
                            path.write_bytes(struct.pack('<III', 0x55aa1234, 1, 1) + b'\0')
            for child in value.values():
                self.materialize_origins(child)

    def proofs(self, value):
        if isinstance(value, list):
            for child in value:
                self.proofs(child)
        elif isinstance(value, dict):
            for origin in value.get('installedGameOrigins', []):
                # Historical baselines may name addons removed from the real
                # game. Rebuild a complete mount proof over the fake union.
                proofs = []
                for path in sorted(Path(origin['directory']).glob('*.vpk')):
                    info = {k: v for k, v in identity(path).items() if k in ('size', 'device', 'inode', 'modifiedSeconds', 'modifiedNanos')}
                    proofs.append(dict(sourcePath=str(path), state='present', proofVersion=2,
                                       proofKind='bytes', member=True, directory=False,
                                       sha256=digest(path), identity=info, **info))
                origin['files'] = proofs
            for child in value.values():
                self.proofs(child)
            if isinstance(value.get('manifest'), dict) and 'generation' in value:
                self.generation(value)
            path = next((Path(value[k]) for k in ('sourcePath', 'path') if isinstance(value.get(k), str) and value[k].startswith(str(self.root) + '/')), None)
            if path and path.exists():
                info = identity(path)
                for key in info.keys() & value.keys():
                    value[key] = info[key]
                if isinstance(value.get('identity'), dict):
                    value['identity'] = {k: info[k] for k in value['identity']}
                if 'sha256' in value and path.is_file():
                    value['sha256'] = digest(path)
            for text, sha in [('gameinfo', 'gameinfoSha256'), ('before', 'beforeSha256'), ('after', 'afterSha256')]:
                if isinstance(value.get(text), str) and sha in value:
                    value[sha] = hashlib.sha256(value[text].encode()).hexdigest()

    def build(self):
        backup(self.library / 'library.sqlite3', self.data / 'library.sqlite3')
        db = sqlite3.connect(self.data / 'library.sqlite3')
        if db.execute('select count(*) from journals').fetchone()[0]:
            raise ValueError('Source has pending recovery journals; fixture refuses unfinished transactions.')
        for src in sorted((self.library / 'payloads').iterdir()):
            copy(src, self.data / 'payloads' / src.name)
        for relative in ['cache/catalog-v1.sqlite3', 'local-jobs.sqlite3']:
            if (self.library / relative).exists():
                backup(self.library / relative, self.data / relative)
        copy(self.game / 'game/citadel/gameinfo.gi', self.citadel / 'gameinfo.gi')
        cfg = self.game / 'game/citadel/cfg'
        for src in cfg.rglob('*'):
            if src.is_file() and (src.suffix == '.cfg' or src.name == 'citadel_hero_settings.lst'):
                copy(src, self.citadel / 'cfg' / src.relative_to(cfg))
        for src in (self.game / 'game/citadel').glob('citadel_hero_settings.lst'):
            copy(src, self.citadel / src.name)
        manifest = self.game.parent.parent / 'appmanifest_1422450.acf'
        if manifest.exists():
            copy(manifest, self.fake.parent.parent / manifest.name)
        # Only enumerate addon names. No original addon bytes are read.
        empty_vpk = struct.pack('<III', 0x55aa1234, 1, 1) + b'\0'
        for addon in (self.game / 'game/citadel').glob('addons*'):
            if addon.is_dir() and not addon.is_symlink():
                for src in addon.rglob('*.vpk'):
                    target = self.citadel / src.relative_to(self.game / 'game/citadel')
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(empty_vpk)
        workspace = self.citadel / 'deadlock_native'
        workspace.mkdir(parents=True, exist_ok=True)
        (workspace / 'owner.json').write_text(dump(dict(version=1, root=str(self.data))))
        # Older previews do not hydrate linked library media. Project only
        # saved, public catalog images into empty source image lists in the copy.
        catalog = self.data / 'cache/catalog-v1.sqlite3'
        summaries = {}
        if catalog.exists():
            with sqlite3.connect(catalog) as conn:
                for provider, kind, sid, payload in conn.execute('select provider,submission_type,submission_id,payload from submission'):
                    summaries[(provider, kind, sid)] = json.loads(payload)
        def artwork(value):
            if isinstance(value, list):
                for v in value:
                    artwork(v)
            elif isinstance(value, dict):
                source = value.get('source', {})
                if isinstance(source, dict) and not source.get('images'):
                    key = source.get('nativeRemoteLink', {}).get('submission', {})
                    summary = summaries.get((key.get('provider'), key.get('submissionType'), key.get('submissionId')), {})
                    if summary.get('visibilityKnown') and (not summary.get('isNsfw') or self.include_nsfw_artwork) and not summary.get('isObsolete'):
                        source['images'] = summary.get('images', [])
                for v in value.values():
                    artwork(v)
        for table, in db.execute("select name from sqlite_master where type='table'").fetchall():
            for row in db.execute(f'select * from "{table}"').fetchall():
                for cell in row:
                    if isinstance(cell, str):
                        try:
                            self.materialize_origins(self.rebase(json.loads(cell)))
                        except json.JSONDecodeError:
                            pass
        # Rewrite JSON columns, including saved operation inputs/results and
        # bootstrap/launch bindings. Refuse journals above: recovery evidence is
        # not safely fabricated by a screenshot tool.
        for table, in db.execute("select name from sqlite_master where type='table'").fetchall():
            columns = [r[1] for r in db.execute(f'pragma table_info("{table}")')]
            for row in db.execute(f'select rowid,* from "{table}"').fetchall():
                for col, original in zip(columns, row[1:]):
                    if not isinstance(original, str):
                        continue
                    try:
                        value = self.rebase(json.loads(original))
                    except json.JSONDecodeError:
                        value = self.rebase(original)
                        encoded = value
                    else:
                        self.proofs(value)
                        artwork(value)
                        encoded = dump(value)
                    db.execute(f'update "{table}" set "{col}"=? where rowid=?', (encoded, row[0]))
        # Inode receipts bind payload hashes to the source files' inodes and
        # filesystem. The copies have new inodes, so drop the receipts and let
        # the helper hash the copies again.
        if db.execute("select 1 from sqlite_master where type='table' and name='payload_inode_receipts'").fetchone():
            db.execute('delete from payload_inode_receipts')
        db.commit()
        doc = json.loads(db.execute('select body from library').fetchone()[0])
        db.close()
        # Copy rebasing for local job inputs/bodies; completed jobs are retained.
        jobs = self.data / 'local-jobs.sqlite3'
        if jobs.exists():
            with sqlite3.connect(jobs) as conn:
                for job, inp, body in conn.execute('select id,input,body from jobs').fetchall():
                    if json.loads(body).get('phase') not in ('succeeded', 'cancelled', 'interrupted', 'failed'):
                        raise ValueError(f'Source local job is unfinished: {job}')
                    values = [self.rebase(json.loads(v)) for v in (inp, body)]
                    for v in values:
                        self.proofs(v)
                    conn.execute('update jobs set input=?,body=? where id=?', (*map(dump, values), job))
        # Review an internally consistent deployed state even if Steam changed
        # the real gameinfo after the last Apply. Preserve the copied file too.
        if doc.get('applied'):
            shutil.copyfile(self.citadel / 'gameinfo.gi', self.root / 'source-gameinfo.gi')
            (self.citadel / 'gameinfo.gi').write_text(doc['applied']['gameinfo'])
        return doc


def health_preview_helper(root, app, report_path):
    """Screenshot-only cached health adapter; never used by a normal launch.

    All real library/game RPCs still use the paired helper against the fixture.
    Health responses use supplied evidence because the fake game has no resources.
    This is presentation evidence, not a claim of fresh game validation.
    """
    report = json.loads(report_path.read_text())
    if 'mods' not in report:
        raise ValueError('Health report must contain mods')
    (root / 'seeded-health.json').write_text(dump(report))
    proxy = root / 'health-preview-helper'
    proxy.write_text("#!/usr/bin/env python3\n" +
        "import json, subprocess, sys, threading\n" +
        "from pathlib import Path\n" +
        "report=json.loads(Path(__file__).with_name('seeded-health.json').read_text())\n" +
        "child=subprocess.Popen(" + repr([str(app.with_name('modlock-helper'))]) + ",stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)\n" +
        "lock=threading.Lock()\n" +
        "def emit(line):\n with lock: print(line.strip(),flush=True)\n" +
        "def forward():\n for line in child.stdout: emit(line)\n" +
        "threading.Thread(target=forward,daemon=True).start()\n" +
        "for line in sys.stdin:\n request=json.loads(line)\n data=request.get('params',{}).get('dataDir')\n if data is not None and Path(data).resolve() != Path(__file__).parent / 'data/deadlock-native': raise ValueError('Screenshot helper refuses non-fixture data')\n if request.get('method') in ('native.health.get','native.health.check'):\n  emit(json.dumps(dict(jsonrpc='2.0',id=request['id'],result=dict(running=False,checked=len(report['mods']),total=len(report['mods']),report=report,cached=True))))\n else:\n  child.stdin.write(line); child.stdin.flush()\n" +
        "child.stdin.close(); child.wait()\n")
    proxy.chmod(0o700)
    return proxy


def updates_preview_helper(root, app, snapshot, report_path):
    """Explicit presentation fixture; preserves real profile/revision response binding."""
    report = json.loads(report_path.read_text())
    rows = snapshot.get('mods', [])
    if not rows:
        raise ValueError('Update preview requires a library mod')
    report['mods'][0]['modId'] = rows[min(1, len(rows) - 1)]['id']
    (root / 'seeded-updates.json').write_text(dump(report))
    proxy = root / 'updates-preview-helper'
    proxy.write_text("#!/usr/bin/env python3\n" +
        "import json, subprocess, sys, threading, time\nfrom pathlib import Path\n" +
        "report=json.loads(Path(__file__).with_name('seeded-updates.json').read_text())\n" +
        "child=subprocess.Popen(" + repr([str(app.with_name('modlock-helper'))]) + ",stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)\n" +
        "def forward():\n for line in child.stdout:\n  response=json.loads(line); result=response.get('result',{})\n  if isinstance(result,dict) and 'lastUpdateCheck' in result and 'profileId' in result:\n   result['mods']=report['mods']; result['lastUpdateCheck']=int(time.time())\n  print(json.dumps(response),flush=True)\n" +
        "threading.Thread(target=forward,daemon=True).start()\n" +
        "for line in sys.stdin:\n request=json.loads(line); data=request.get('params',{}).get('dataDir')\n if data is not None and Path(data).resolve() != Path(__file__).parent / 'data/deadlock-native': raise ValueError('Screenshot helper refuses non-fixture data')\n child.stdin.write(line); child.stdin.flush()\n" +
        "child.stdin.close(); child.wait()\n")
    proxy.chmod(0o700)
    return proxy


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog='Existing output is wiped ONLY when marked as a review fixture. Sources are opened read-only. No game override exists: gamePath is rebased in SQLite. Pending journals/live WAL are refused. Fake addon VPKs contain no resources; use for UI review, not gameplay or recovery testing.')
    parser.add_argument('--output', type=Path, help='Fixture root (default: new directory under $TMPDIR)')
    parser.add_argument('--library', type=Path, default=Path.home() / '.local/share/deadlock-native')
    parser.add_argument('--game', type=Path, required=True, help='Deadlock install to copy read-only')
    parser.add_argument('--include-nsfw-artwork', action='store_true', help='Include saved public catalog images flagged NSFW (default: leave those rows masked)')
    parser.add_argument('--app', type=Path, help='Also verify snapshot and screenshot all ten pages using a test-enabled app and its sibling helper')
    parser.add_argument('--health-report', type=Path, help='Seed screenshot health responses from a saved report (presentation only; requires --app)')
    parser.add_argument('--updates-report', type=Path, help='Seed presentation-only update evidence; requires --app')
    args = parser.parse_args()
    if args.updates_report and (not args.app or args.health_report):
        parser.error('--updates-report requires --app and cannot combine with --health-report')
    if args.health_report and not args.app:
        parser.error('--health-report requires --app')
    library, game = args.library.expanduser().resolve(), args.game.expanduser().resolve()
    root = args.output.expanduser().absolute() if args.output else Path(tempfile.mkdtemp(prefix='deadlock-review-'))
    if root.is_symlink() or root.resolve() != root:
        parser.error('Output must have no symlink or traversal components')
    for source in (library, game):
        if root == source or root in source.parents or source in root.parents:
            parser.error('Output and source directories must not overlap')
    if root.exists() and any(root.iterdir()):
        if not (root / MARKER).is_file():
            parser.error('Refusing to wipe unmarked directory')
        shutil.rmtree(root)
    root.mkdir(parents=True, exist_ok=True)
    root.chmod(0o700)
    (root / MARKER).write_text('Disposable review fixture\n')
    fixture = Fixture(root, library, game, args.include_nsfw_artwork)
    doc = fixture.build()
    (fixture.data / ".writer.lock").touch(mode=0o600)
    env = {k: str(root / name) for k, name in [('HOME', 'home'), ('XDG_DATA_HOME', 'data'),
        ('XDG_CONFIG_HOME', 'config'), ('XDG_CACHE_HOME', 'cache'), ('XDG_STATE_HOME', 'state'),
        ('XDG_RUNTIME_DIR', 'run'), ('TMPDIR', 'tmp'), ('QML_DISK_CACHE_PATH', 'cache/qmlcache')]}
    for path in env.values():
        Path(path).mkdir(mode=0o700, exist_ok=True)
    env.update(MODLOCK_UI_TEST_ROOT=str(root), XDG_DATA_DIRS=env['XDG_DATA_HOME'],
               XDG_CONFIG_DIRS=env['XDG_CONFIG_HOME'],
               DBUS_SESSION_BUS_ADDRESS='unix:path=' + str(root / 'no-bus'))
    display = os.environ.get('WAYLAND_DISPLAY')
    runtime = os.environ.get('XDG_RUNTIME_DIR')
    if display and runtime and not Path(display).is_absolute():
        env['WAYLAND_DISPLAY'] = str(Path(runtime) / display)
    (root / 'environment.json').write_text(dump(env))
    print('env ' + ' '.join(f'{k}={shlex.quote(v)}' for k, v in env.items()) + ' ' + shlex.quote(str(args.app or Path('build/modlock'))))
    print(f'Fake game (stored in library.gamePath): {fixture.fake}')
    print(f'Mods: {sum(len(p["mods"]) for p in doc["profiles"].values())}; generations: {len(fixture.generations)}')
    if args.app:
        app = args.app.expanduser().resolve()
        runenv = dict(os.environ, **env, QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software')
        request = dict(jsonrpc='2.0', id=1, method='native.snapshot', params=dict(version=1, dataDir=str(fixture.data)))
        result = subprocess.run([str(app.with_name('modlock-helper'))], input=dump(request)+'\n', text=True, capture_output=True, env=runenv, timeout=45)
        (root / 'snapshot.json').write_text(result.stdout)
        response = json.loads(result.stdout)
        if result.returncode or 'error' in response:
            raise ValueError(f'Fixture snapshot failed: {result.stdout[:1000]}')
        snapshot = response['result']
        if snapshot.get('recoveryRequired') or snapshot.get('pendingReason') == 'externalGameinfoChange' or 'error' in snapshot or snapshot.get('sourceKind') != 'native':
            raise ValueError(f'Fixture snapshot failed: {result.stdout[:1000]}')
        shots = root / 'screenshots'
        shots.mkdir(exist_ok=True)
        preview_helper = health_preview_helper(root, app, args.health_report) if args.health_report else None
        if args.updates_report:
            preview_helper = updates_preview_helper(root, app, snapshot, args.updates_report)
        for page in (["select", "grid"] if args.updates_report else PAGES):
            shot = shots / (page + '.png')
            page_env = dict(runenv, SCREENSHOT_DELAY_MS=str(max(25000, int(runenv.get('SCREENSHOT_DELAY_MS', '0'))))) if page == 'apply' else runenv
            run = subprocess.run([str(app), '--screenshot', str(shot), '--screenshot-page', page] + (['--helper', str(preview_helper)] if preview_helper else []), env=page_env, capture_output=True, text=True, timeout=60)
            (shots / (page + '.log')).write_text(run.stdout + run.stderr)
            if run.returncode or not shot.exists():
                raise ValueError(f'Screenshot failed: {page}; see {shots / (page + ".log")}')
            print(shot, flush=True)


if __name__ == '__main__':
    main()
