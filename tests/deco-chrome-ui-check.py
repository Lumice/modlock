#!/usr/bin/env python3
"""Verify Deco chrome 03 runtime invariants, sheen gating, profile badge/elision, and dark/light captures."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_helper import apply_review, hello as paired_hello, observe, startup as paired_startup  # noqa: E402


def make_summary(sid: str, title: str, downloads: str, hero: str = 'Ivy', category: str = 'Skins') -> dict:
    return dict(
        submission=dict(provider='gamebanana', submissionType='mod', submissionId=str(sid)),
        name=title,
        description='Synthetic catalog entry ' + title,
        author='Modder',
        authorRemoteId='101',
        profileUrl='https://gamebanana.com/mods/' + str(sid),
        category=category,
        hero=hero,
        isAudio=False,
        isMap=False,
        isNsfw=False,
        visibilityKnown=True,
        isObsolete=False,
        hasFiles=True,
        downloadCount=str(downloads),
        likes='42',
        images=[],
        thumbnailUrl=None,
        audioUrl=None,
        tags=['skin'],
        remoteAddedAt=1700000000,
        remoteUpdatedAt=1700000100,
        filesUpdatedAt=1700000200,
        developmentState=None,
        completionPercentage=None,
        hydrated=True,
    )


def make_provenance() -> dict:
    return dict(
        source='cache',
        sourcePin='976637e2d63329ffef6f87eca40e2aea681acc94',
        fetchedAt=1700000300,
        lastSuccessfulSyncAt=1700000300,
        stale=False,
        incomplete=False,
        refreshError=None,
        searchAvailable=True,
        searchState='available',
        accounting=dict(
            databaseBytes='2048',
            sidecarBytes='512',
            spoolAndControlBytes='128',
            reserveBytes='1073741824',
            aggregateQuota=False,
        ),
    )


def run_peer() -> None:
    receipts: dict[str, dict] = {}
    released: set[str] = set()
    latest_items = [
        make_summary('201', 'Brass Lantern Haze', '410', hero='Haze'),
        make_summary('202', 'Velvet Lash Coats', '290', hero='Lash'),
        make_summary('203', 'Noir Ivy Wings', '180', hero='Ivy'),
    ]
    popular_items = [
        make_summary('301', 'After Hours HUD', '9400', hero='General', category='UI'),
        make_summary('302', 'Art Deco Amber Seven', '6100', hero='Seven'),
        make_summary('303', 'Ivory Clockwork Dynamo', '4800', hero='Dynamo'),
    ]

    def load_conf(root: str) -> dict:
        p = Path(root) / 'peer-config.json'
        return json.loads(p.read_text()) if p.exists() else {}

    for raw in sys.stdin:
        req = json.loads(raw)
        method = req['method']
        p = req.get('params', {})
        root = p.get('dataDir', '')

        if method == 'hello':
            result = paired_hello()
        elif method == 'native.catalog.open':
            sess = hashlib.sha256(root.encode()).hexdigest()[:32]
            result = dict(version=1, root=root, catalogSession=sess, phase='ready', error=None)
        elif method == 'native.catalog.close':
            result = dict(version=1, root=root, catalogSession=p['catalogSession'], phase='closed')
        elif method == 'native.catalog.begin':
            key = p['requestKey']
            receipts[key] = p
            result = dict(
                version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                viewGeneration=p['viewGeneration'], inputDigest='a' * 64, phase='running',
            )
        elif method == 'native.catalog.status' and p.get('requestKey'):
            key = p['requestKey']
            r = receipts[key]
            op = r['operation']
            kind = op['kind']
            args = op['args']
            if key in released:
                result = dict(
                    version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                    viewGeneration=r['viewGeneration'], inputDigest='a' * 64,
                    operation=dict(kind=kind, args=None), phase='released', progress=None, result=None, error=None,
                )
            else:
                prefs_rev = '0' if kind in ('query', 'collection', 'favoritesGet', 'favoriteSet') else None
                ident = dict(kind=kind, args=args, catalogRevision='1', preferencesRevision=prefs_rev)
                prov = make_provenance()
                if kind == 'query':
                    sort = args.get('sort', 'default')
                    items = latest_items if sort == 'releaseDate' else popular_items
                    favs = [dict(submission=item['submission'], favorite=False) for item in items]
                    data = dict(
                        items=items, total=str(len(items)), page=args['page'],
                        pageSize=args['pageSize'], favorites=favs, preferencesRevision='0',
                    )
                elif kind == 'facets':
                    data = [dict(category='Skins', hero='Ivy')]
                elif kind == 'favoritesGet':
                    data = dict(preferencesRevision='0', submission=args['submission'], favorite=False)
                else:
                    data = dict(mode='incremental', snapshot='b' * 32, complete=True)
                result = dict(
                    version=1, root=root, catalogSession=p['catalogSession'], requestKey=key,
                    viewGeneration=r['viewGeneration'], inputDigest='a' * 64,
                    operation=op, phase='succeeded', progress=None,
                    result=dict(identity=ident, catalogRevision='1', provenance=prov, data=data), error=None,
                )
        elif method in ('native.catalog.cancel', 'native.catalog.release') and p.get('requestKey'):
            key = p['requestKey']
            released.add(key)
            result = dict(version=1, root=root, catalogSession=p['catalogSession'], requestKey=key, state='released')
        elif method == 'native.snapshot':
            conf = load_conf(root)
            selected = p.get('profileId') or conf.get('selectedProfileId', 'p1')
            mods = [
                dict(
                    id='recent-1', remoteId='1001', title='Midnight Haze', author='Studio',
                    hero='Haze', category='Skins', status='installed', enabled=True,
                    appliedEnabled=True, available=True, unavailableReason=None,
                    installOrder=0, installedVpks=[], missingVpks=[], addedAt='2026-10-25T10:00:00Z',
                ),
                dict(
                    id='recent-2', remoteId='1002', title='Gilded Abrams', author='Atelier',
                    hero='Abrams', category='Skins', status='installed', enabled=True,
                    appliedEnabled=True, available=True, unavailableReason=None,
                    installOrder=1, installedVpks=[], missingVpks=[], addedAt='2026-10-24T10:00:00Z',
                ),
            ]
            result = dict(
                protocolVersion=1,
                sourceKind='native',
                sourcePath='/synthetic/state.json',
                storeVersion=1,
                revision='1',
                gamePath='/synthetic/game',
                activeProfileId=conf.get('activeProfileId', 'p1'),
                selectedProfileId=selected,
                appliedProfileId=conf.get('activeProfileId', 'p1'),
                pendingChanges=bool(conf.get('pendingChanges', False)),
                pendingStatus=conf.get('pendingStatus', 'current'),
                pendingReason=conf.get('pendingReason'),
                canRestore=False,
                recoveryRequired=bool(conf.get('recoveryRequired', False)),
                warnings=[],
                profiles=conf.get('profiles', [dict(id='p1', name='Default', isActive=True, modCount=len(mods))]),
                mods=mods,
            )
        elif method == 'native.applyPreview':
            result = apply_review(p['profileId'])
        else:
            handled, value = paired_startup(method, p)
            if not handled:
                sys.stderr.write(f'Unexpected method in deco-chrome peer: {method}\n')
                sys.exit(2)
            result = value

        observe(result)
        sys.stdout.write(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=result)) + '\n')
        sys.stdout.flush()


def setup_isolated_env(root: Path, theme: str, capture_dir: str | None) -> dict[str, str]:
    root.mkdir(parents=True, mode=0o700, exist_ok=True)
    env = {
        k: v for k, v in os.environ.items()
        if not k.startswith('XDG_') and k not in ('DISPLAY', 'WAYLAND_DISPLAY', 'QML_DISK_CACHE_PATH')
    }
    for index, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
                               'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'TMPDIR')):
        folder = root / str(index)
        folder.mkdir(parents=True, mode=0o700)
        env[key] = str(folder)
    env['MODLOCK_UI_TEST_ROOT'] = str(root)
    env.update(
        XDG_CONFIG_DIRS=env['XDG_CONFIG_HOME'],
        XDG_DATA_DIRS=env['XDG_DATA_HOME'],
        QT_QPA_PLATFORM='offscreen',
        QT_QUICK_BACKEND='software',
        QT_FORCE_STDERR_LOGGING='1',
        DECO_THEME=theme,
    )
    if capture_dir:
        env['DECO_CAPTURE_DIR'] = capture_dir
    return env


def run_suite(binary: str) -> None:
    capture_dir = os.environ.get('DECO_CAPTURE_DIR') or None
    with tempfile.TemporaryDirectory(prefix='dc03-') as scratch:
        scratch_path = Path(scratch)
        for theme in ('dark', 'light'):
            env = setup_isolated_env(scratch_path / f'demo-{theme}', theme, capture_dir)
            result = subprocess.run(
                [binary, '--demo', '--check-ui', 'deco-chrome-demo'],
                env=env, capture_output=True, text=True, timeout=30,
            )
            if result.returncode != 0:
                sys.exit(f'deco-chrome-demo ({theme}) failed ({result.returncode}):\n{result.stdout}\n{result.stderr}')
            print(f'PASS deco-chrome-demo {theme}', flush=True)

        native_root = scratch_path / 'native-dark'
        env = setup_isolated_env(native_root, 'dark', capture_dir)
        data_dir = native_root / 'native'
        data_dir.mkdir(parents=True)
        (data_dir / 'peer-config.json').write_text(json.dumps(dict(
            activeProfileId='p1',
            selectedProfileId='p1',
            pendingChanges=True,
            pendingStatus='pending',
            recoveryRequired=False,
            profiles=[
                dict(id='p1', name='Default', isActive=True, modCount=2),
                dict(id='p2', name='Extremely Long Modpack Profile Name For Layout Truncation Verification 12345', isActive=False, modCount=2),
                dict(id='p3', name='\U0001f98a \u041c\u043e\u0434\u044b', isActive=False, modCount=0),
                dict(id='p4', name='', isActive=False, modCount=0),
            ],
        )))
        peer = native_root / 'deco-peer'
        peer.symlink_to(Path(__file__).resolve())
        result = subprocess.run(
            [binary, '--helper', str(peer), '--data-dir', str(data_dir), '--check-ui', 'deco-chrome-native'],
            env=env, capture_output=True, text=True, timeout=45,
        )
        if result.returncode != 0:
            sys.exit(f'deco-chrome-native failed ({result.returncode}):\n{result.stdout}\n{result.stderr}')
        print('PASS deco-chrome-native', flush=True)


if __name__ == '__main__':
    if sys.argv[1:] == ['--data-root']:
        print(json.dumps({'root': str(Path(os.environ.get('XDG_DATA_HOME') or str(Path.home() / '.local/share')) / 'modlock')}))
        sys.exit(0)
    if os.path.basename(sys.argv[0]) == 'deco-peer':
        run_peer()
        sys.exit(0)
    if len(sys.argv) < 2:
        sys.exit('Usage: deco-chrome-ui-check.py <modlock-binary>')
    run_suite(sys.argv[1])
