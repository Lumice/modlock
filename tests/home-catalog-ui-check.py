#!/usr/bin/env python3
"""Verify Home saved-catalog shelves, qualifiers, masking, fallbacks, and navigation."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_helper import hello as paired_hello, observe, startup as paired_startup  # noqa: E402


def make_summary(sid: str, title: str, downloads: str, hero: str = 'Ivy', category: str = 'Skins',
                 author: str = 'Modder', nsfw: bool = False, visibility_known: bool = True,
                 images: list[str] | None = None) -> dict:
    sub = dict(provider='gamebanana', submissionType='mod', submissionId=str(sid))
    return dict(
        submission=sub,
        name=title,
        description='Saved catalog description for ' + title,
        author=author,
        authorRemoteId='101',
        profileUrl='https://gamebanana.com/mods/' + str(sid),
        category=category,
        hero=hero,
        isAudio=False,
        isMap=False,
        isNsfw=nsfw if visibility_known else True,
        visibilityKnown=visibility_known,
        isObsolete=False,
        hasFiles=True,
        downloadCount=str(downloads),
        likes='42',
        images=images if images is not None else [],
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


def make_provenance(conf: dict) -> dict:
    return dict(
        source='cache',
        sourcePin='976637e2d63329ffef6f87eca40e2aea681acc94',
        fetchedAt=1700000300,
        lastSuccessfulSyncAt=1700000300,
        stale=bool(conf.get('stale', False)),
        incomplete=bool(conf.get('incomplete', False)),
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


def build_snapshot(root: str, conf: dict) -> dict:
    recent_mods = conf.get('recentMods')
    if recent_mods is None:
        recent_mods = [
            dict(
                id='recent-1', remoteId='1001', title='Midnight Haze', author='Studio',
                hero='Haze', category='Skins', status='installed', enabled=True,
                appliedEnabled=True, available=True, unavailableReason=None,
                installOrder=0, installedVpks=[], missingVpks=[], addedAt='2026-10-25T10:00:00Z',
            ),
            dict(
                id='recent-2', remoteId='1002', title='Gilded Abrasion', author='Craft',
                hero='Abrams', category='Skins', status='installed', enabled=True,
                appliedEnabled=True, available=True, unavailableReason=None,
                installOrder=1, installedVpks=[], missingVpks=[], addedAt='2026-10-24T10:00:00Z',
            ),
            dict(
                id='recent-3', remoteId='1003', title='Velvet Ivy', author='Loom',
                hero='Ivy', category='Skins', status='installed', enabled=False,
                appliedEnabled=False, available=True, unavailableReason=None,
                installOrder=2, installedVpks=[], missingVpks=[], addedAt='2026-10-23T10:00:00Z',
            ),
            dict(
                id='recent-4', remoteId='1004', title='Brass Sight', author='Vane',
                hero='Vindicta', category='UI', status='installed', enabled=True,
                appliedEnabled=True, available=True, unavailableReason=None,
                installOrder=3, installedVpks=[], missingVpks=[], addedAt='2026-10-22T10:00:00Z',
            ),
            dict(
                id='recent-5', remoteId='1005', title='Harbor Echo', author='Port',
                hero='Seven', category='Audio', status='installed', enabled=True,
                appliedEnabled=True, available=True, unavailableReason=None,
                installOrder=4, installedVpks=[], missingVpks=[], addedAt='2026-10-21T10:00:00Z',
            ),
        ]
    return dict(
        protocolVersion=1,
        sourceKind='native',
        storeVersion=1,
        revision='1',
        sourcePath=root + '/library.json',
        activeProfileId='default',
        selectedProfileId='default',
        pendingChanges=False,
        pendingStatus=conf.get('pendingStatus', 'current'),
        pendingReason=conf.get('pendingReason'),
        canRestore=False,
        recoveryRequired=False,
        gamePath='/synthetic/game',
        appliedProfileId='default',
        warnings=[],
        profiles=[dict(id='default', name='Default', isActive=True, modCount=len(recent_mods))],
        mods=recent_mods,
    )


def run_peer() -> None:
    receipts: dict[str, dict] = {}
    released: set[str] = set()
    summaries_by_id: dict[str, dict] = {}

    def load_conf(root: str) -> dict:
        p = Path(root) / 'peer-config.json'
        return json.loads(p.read_text()) if p.exists() else {}

    def record(root: str, entry: dict) -> None:
        if not root:
            return
        Path(root).mkdir(parents=True, exist_ok=True)
        with (Path(root) / 'catalog-trace.jsonl').open('a') as f:
            f.write(json.dumps(entry) + '\n')

    for raw in sys.stdin:
        req = json.loads(raw)
        method = req['method']
        p = req.get('params', {})
        root = p.get('dataDir', '')
        if root and method != 'hello':
            record(root, dict(method=method, key=p.get('requestKey'), op=p.get('operation')))

        if method == 'hello':
            result = paired_hello()
        elif method == 'native.catalog.open':
            conf = load_conf(root)
            sess = hashlib.sha256(root.encode()).hexdigest()[:32]
            if conf.get('failOpen'):
                result = dict(
                    version=1, root=root, catalogSession=sess, phase='failed',
                    error=dict(kind='catalogUnavailable', message='Synthetic catalog unavailable.', retryAfterSeconds=None),
                )
            else:
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
            conf = load_conf(root)
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
                prov = make_provenance(conf)
                if kind == 'query':
                    sort = args.get('sort', 'default')
                    is_home_slice = args.get('pageSize') == 5 and args.get('page') == 0 and r['viewGeneration'] == '0'
                    if is_home_slice and sort == 'releaseDate':
                        items = conf.get('latestItems', [])
                    elif is_home_slice and sort == 'downloadCount':
                        items = conf.get('popularItems', [])
                    else:
                        prov['stale'] = False
                        items = conf.get('browseItems', conf.get('popularItems', []))
                    for item in items:
                        summaries_by_id[item['submission']['submissionId']] = item
                    favs = [dict(submission=item['submission'], favorite=False) for item in items]
                    data = dict(
                        items=items, total=str(len(items)), page=args['page'],
                        pageSize=args['pageSize'], favorites=favs, preferencesRevision='0',
                    )
                elif kind == 'facets':
                    prov['stale'] = False
                    data = [dict(category='Skins', hero='Ivy')]
                elif kind == 'detail':
                    prov['stale'] = False
                    sid = args['submission']['submissionId']
                    summ = summaries_by_id.get(sid) or make_summary(sid, 'Selected Mod', '120')
                    data = dict(
                        state='ready', reason=None, submission=args['submission'],
                        refreshRecommended=False, refreshState='idle',
                        resource=dict(
                            summary=summ, description='Detailed notes for ' + summ['name'],
                            links=[], linksTruncated=False, textFormat='plain', public=True,
                            contentWarnings=[], dependencies=[],
                            metadata=dict(donationLinks=[], mapName=None),
                            sourcePin='976637e2d63329ffef6f87eca40e2aea681acc94',
                        ),
                    )
                elif kind == 'favoritesGet':
                    data = dict(preferencesRevision='0', submission=args['submission'], favorite=False)
                else:
                    data = dict(
                        mode='incremental', snapshot='b' * 32, complete=True,
                        startedAt=1700000300, memberMaxAgeSeconds=604800,
                        collections=dict(
                            state='complete', snapshot='c' * 32, headersComplete=True,
                            membersComplete=False, memberMaxAgeSeconds=604800,
                        ),
                    )
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
            result = build_snapshot(root, conf)
        else:
            handled, value = paired_startup(method, p)
            if not handled:
                sys.stderr.write(f'Unexpected method in home-catalog peer: {method}\n')
                sys.exit(2)
            result = value

        observe(result)
        sys.stdout.write(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=result)) + '\n')
        sys.stdout.flush()


def setup_env(root: Path, hide_nsfw: bool = False) -> tuple[dict[str, str], Path]:
    root.mkdir(parents=True, mode=0o700, exist_ok=True)
    env = {
        k: v for k, v in os.environ.items()
        if not k.startswith('XDG_') and k not in ('DISPLAY', 'WAYLAND_DISPLAY', 'QML_DISK_CACHE_PATH')
    }
    for index, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'TMPDIR')):
        folder = root / str(index)
        folder.mkdir(parents=True, mode=0o700)
        env[key] = str(folder)
    conf_file = Path(env['XDG_CONFIG_HOME']) / 'modlock' / 'settings.conf'
    conf_file.parent.mkdir()
    conf_file.write_text(
        f'[display]\nautomaticApply=false\nhideNsfwMods={"true" if hide_nsfw else "false"}\nreduceMotion=true\n'
    )
    env['MODLOCK_UI_TEST_ROOT'] = str(root)
    env.update(
        XDG_CONFIG_DIRS=env['XDG_CONFIG_HOME'],
        XDG_DATA_DIRS=env['XDG_DATA_HOME'],
        QT_QPA_PLATFORM='offscreen',
        QT_QUICK_BACKEND='software',
        QT_FORCE_STDERR_LOGGING='1',
        QT_LOGGING_RULES='*.debug=false',
    )
    data_dir = root / 'native'
    data_dir.mkdir()
    return env, data_dir


def run_case(binary: str, scratch: Path, name: str, peer_config: dict, extra_env: dict[str, str], expected_stdout: str) -> list[dict]:
    case_root = scratch / name
    case_root.mkdir(mode=0o700)
    env, data_dir = setup_env(case_root, hide_nsfw=False)
    env.update(extra_env)
    (data_dir / 'peer-config.json').write_text(json.dumps(peer_config))
    peer = case_root / 'home-catalog-peer'
    peer.symlink_to(Path(__file__).resolve())
    res = subprocess.run(
        [binary, '--helper', str(peer), '--data-dir', str(data_dir), '--check-ui', 'home-catalog'],
        env=env, capture_output=True, text=True, timeout=25,
    )
    if res.returncode != 0 or expected_stdout not in res.stdout:
        trace_path = data_dir / 'catalog-trace.jsonl'
        trace_txt = trace_path.read_text() if trace_path.exists() else '<none>'
        sys.exit(f'FAIL home-catalog-ui-check {name}: code={res.returncode}\nstdout:\n{res.stdout}\nstderr:\n{res.stderr}\ntrace:\n{trace_txt}')
    trace_lines = (data_dir / 'catalog-trace.jsonl').read_text().splitlines()
    return [json.loads(line) for line in trace_lines]


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f'Usage: {sys.argv[0]} <modlock-binary>')
    binary = sys.argv[1]
    with tempfile.TemporaryDirectory(prefix='hcui-') as scratch_str:
        scratch = Path(scratch_str)

        # 1. Populated Home shelves with partial + stale qualifiers, masked NSFW tile, See all sort, and card click -> Details -> Back.
        latest_items = [
            make_summary('201', 'Chronicle Coat', '240', hero='Ivy', category='Skins', nsfw=True, images=['https://example.invalid/chronicle-coat.png']),
            make_summary('202', 'Clockwork Bow', '180', hero='Grey Talon', category='Weapons'),
            make_summary('203', 'Streetlight Visor', '95', hero='Haze', category='Skins'),
            make_summary('204', 'Copper Gauntlets', '64', hero='Abrams', category='Skins'),
            make_summary('205', 'Signal Beacon', '31', hero='Dynamo', category='UI'),
        ]
        popular_items = [
            make_summary('301', 'Clean HUD Overhaul', '48200', hero=None, category='HUD'),
            make_summary('302', 'Velvet Trench', '21500', hero='Vindicta', category='Skins'),
            make_summary('303', 'Transit Announcer', '15400', hero='Seven', category='Audio'),
            make_summary('304', 'Neon Crosshair Kit', '9800', hero=None, category='UI'),
            make_summary('305', 'Brass Revolver', '6200', hero='Infernus', category='Weapons'),
        ]
        trace_pop = run_case(
            binary, scratch, 'populated',
            dict(stale=True, incomplete=True, latestItems=latest_items, popularItems=popular_items, browseItems=popular_items),
            dict(HOME_CATALOG_MODE='populated', HOME_CATALOG_EXPECT_MASKED='1'),
            'PASS UI home catalog populated, see all, and detail back',
        )
        begins = [e for e in trace_pop if e.get('method') == 'native.catalog.begin']
        query_begins = [e for e in begins if e['op']['kind'] == 'query']
        assert len(query_begins) >= 2, begins[:5]
        assert query_begins[0]['op']['args']['sort'] == 'releaseDate', query_begins[:2]
        assert query_begins[1]['op']['args']['sort'] == 'downloadCount', query_begins[:2]
        assert not any(e['op']['kind'] == 'sync' for e in begins), begins
        print('PASS home-catalog-ui-check populated', flush=True)

        # 2. Stale/incomplete Home only (never triggers auto-sync).
        trace_stale = run_case(
            binary, scratch, 'stale-only',
            dict(stale=True, incomplete=True, latestItems=latest_items, popularItems=popular_items),
            dict(HOME_CATALOG_MODE='populated', HOME_CATALOG_CAPTURE_ONLY='1', HOME_CATALOG_EXPECT_MASKED='1'),
            'PASS UI home catalog capture-only',
        )
        assert not any(e.get('method') == 'native.catalog.begin' and e['op']['kind'] == 'sync' for e in trace_stale), trace_stale
        print('PASS home-catalog-ui-check stale-never-syncs', flush=True)

        run_case(binary, scratch, 'viewport', dict(latestItems=latest_items, popularItems=popular_items),
                 dict(HOME_CATALOG_MODE='viewport'), 'PASS UI home viewport scroll and position-only admission')
        print('PASS home-catalog-ui-check controlled viewport', flush=True)
        run_case(binary, scratch, 'independent-unknown', dict(pendingStatus='unknown', latestItems=[], popularItems=[]),
                 dict(HOME_CATALOG_MODE='unknown'), 'PASS UI home independent unknown')
        print('PASS home-catalog-ui-check independent unknown', flush=True)

        run_case(binary, scratch, 'external-coexist', dict(pendingStatus='unknown', pendingReason='externalGameinfoChange', latestItems=[], popularItems=[]),
                 dict(HOME_CATALOG_MODE='coexist'), 'PASS UI home external notice coexistence')
        print('PASS home-catalog-ui-check external coexistence', flush=True)

        # 3. Empty cache fallback -> Open Browse.
        run_case(
            binary, scratch, 'empty',
            dict(stale=False, incomplete=False, latestItems=[], popularItems=[]),
            dict(HOME_CATALOG_MODE='empty', HOME_CATALOG_CLICK_BROWSE='1'),
            'PASS UI home catalog empty',
        )
        print('PASS home-catalog-ui-check empty', flush=True)

        # 4. Quiet unavailable fallback when catalog open fails.
        run_case(
            binary, scratch, 'unavailable',
            dict(failOpen=True),
            dict(HOME_CATALOG_MODE='empty', HOME_CATALOG_CLICK_BROWSE='1'),
            'PASS UI home catalog empty',
        )
        print('PASS home-catalog-ui-check unavailable', flush=True)


if __name__ == '__main__':
    if Path(sys.argv[0]).name == 'home-catalog-peer':
        run_peer()
    else:
        main()
