#!/usr/bin/env python3
"""Verify 03b profile menu relocation: pointer/keyboard activation, selection, disabled edit gate, and focus return."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_helper import hello as paired_hello, observe, startup as paired_startup  # noqa: E402


PROFILES = [
    dict(id='p1', name='Default', isActive=True, modCount=2),
    dict(id='p2', name='Extremely Long Modpack Profile Name For Menu Width Verification 12345', isActive=False, modCount=1),
    dict(id='p3', name='Workshop Picks', isActive=False, modCount=1),
] + [dict(id=f'p{i}', name=f'Profile {i}', isActive=False, modCount=2) for i in range(4, 16)]


def make_mods(profile_id: str) -> list[dict]:
    if profile_id == 'p2':
        return [dict(
            id='m2', remoteId='2002', title='Long Profile Mod', author='Atelier',
            hero='Haze', category='Skins', status='installed', enabled=True,
            appliedEnabled=True, available=True, unavailableReason=None,
            installOrder=0, installedVpks=[], missingVpks=[],
        )]
    return [
        dict(
            id='m1', remoteId='2001', title='Midnight Haze', author='Studio',
            hero='Haze', category='Skins', status='installed', enabled=True,
            appliedEnabled=True, available=True, unavailableReason=None,
            installOrder=0, installedVpks=[], missingVpks=[],
        ),
        dict(
            id='m3', remoteId='2003', title='Velvet Lash', author='Workshop',
            hero='Lash', category='Skins', status='installed', enabled=False,
            appliedEnabled=False, available=True, unavailableReason=None,
            installOrder=1, installedVpks=[], missingVpks=[],
        ),
    ]


def run_peer() -> None:
    readonly = os.environ.get('PROFILE_MENU_MODE') == 'readonly'
    for raw in sys.stdin:
        req = json.loads(raw)
        method = req['method']
        p = req.get('params', {})
        root = p.get('dataDir') or os.environ.get('PROFILE_MENU_TRACE_DIR', '')
        if method in ('native.snapshot', 'library.snapshot') or method.startswith('native.profile'):
            trace_dir = Path(os.environ.get('PROFILE_MENU_TRACE_DIR') or root)
            trace_dir.mkdir(parents=True, exist_ok=True)
            with open(trace_dir / 'trace.jsonl', 'a') as fh:
                fh.write(json.dumps(dict(method=method, params=p)) + '\n')

        if method == 'hello':
            result = paired_hello()
        elif method == 'native.snapshot':
            selected = p.get('profileId') or 'p1'
            mods = make_mods(selected)
            result = dict(
                protocolVersion=1,
                sourceKind='native',
                sourcePath='/synthetic/state.json',
                storeVersion=1,
                revision='1',
                gamePath='/synthetic/game',
                activeProfileId='p1',
                selectedProfileId=selected,
                appliedProfileId='p1',
                pendingChanges=False,
                pendingStatus='current',
                pendingReason=None,
                canRestore=False,
                recoveryRequired=False,
                warnings=[],
                profiles=PROFILES,
                mods=mods,
            )
        elif method == 'library.snapshot':
            selected = p.get('profileId') or 'p1'
            mods = make_mods(selected)
            result = dict(
                protocolVersion=1,
                sourceKind='upstream',
                sourcePath=p.get('statePath', '/synthetic/state.json'),
                storeVersion=33,
                activeProfileId='p1',
                selectedProfileId=selected,
                warnings=[],
                profiles=PROFILES,
                mods=mods,
            )
        else:
            handled, value = paired_startup(method, p, native=not readonly)
            if not handled:
                sys.stderr.write(f'Unexpected method in profile-menu peer: {method}\n')
                sys.exit(2)
            result = value

        observe(result)
        sys.stdout.write(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=result)) + '\n')
        sys.stdout.flush()


def setup_env(root: Path, mode: str, theme: str, capture_dir: str | None) -> dict[str, str]:
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
        PROFILE_MENU_MODE=mode,
        PROFILE_MENU_THEME=theme,
        PROFILE_MENU_TRACE_DIR=str(root / 'trace'),
    )
    if capture_dir:
        env['PROFILE_MENU_CAPTURES'] = capture_dir
    return env


def run_suite(binary: str) -> None:
    capture_dir = os.environ.get('PROFILE_MENU_CAPTURES') or None
    with tempfile.TemporaryDirectory(prefix='pm03b-') as scratch:
        scratch_path = Path(scratch)
        for mode in ('native', 'readonly'):
            for theme in ('dark', 'light'):
                case_root = scratch_path / f'{mode}-{theme}'
                env = setup_env(case_root, mode, theme, capture_dir)
                data_dir = case_root / 'native'
                data_dir.mkdir(parents=True)
                peer = case_root / 'profile-menu-peer'
                peer.symlink_to(Path(__file__).resolve())
                cmd = [binary, '--helper', str(peer), '--data-dir', str(data_dir), '--check-ui', 'profile-menu']
                if mode == 'readonly':
                    source_file = case_root / 'state.json'
                    source_file.write_text('{}')
                    cmd = [binary, '--source', str(source_file), '--helper', str(peer), '--data-dir', str(data_dir), '--check-ui', 'profile-menu']
                result = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=45)
                if result.returncode != 0:
                    sys.exit(f'profile-menu ({mode}/{theme}) failed ({result.returncode}):\n{result.stdout}\n{result.stderr}')
                trace_file = case_root / 'trace' / 'trace.jsonl'
                if not trace_file.exists():
                    sys.exit(f'profile-menu ({mode}/{theme}) wrote no peer trace')
                events = [json.loads(line) for line in trace_file.read_text().splitlines() if line.strip()]
                expected_method = 'library.snapshot' if mode == 'readonly' else 'native.snapshot'
                requested_profiles = [
                    ev['params'].get('profileId')
                    for ev in events
                    if ev['method'] == expected_method and 'profileId' in ev['params']
                ]
                if requested_profiles != ['p15', 'p14', 'p2', 'p1', 'p2']:
                    sys.exit(f'profile-menu ({mode}/{theme}) unexpected viewProfile calls: {requested_profiles!r}')
                if any(ev['method'].startswith('native.profile') for ev in events):
                    sys.exit(f'profile-menu ({mode}/{theme}) issued unexpected profile mutation RPC: {events!r}')
                print(f'PASS profile-menu {mode} {theme}', flush=True)


if __name__ == '__main__':
    if os.path.basename(sys.argv[0]) == 'profile-menu-peer':
        run_peer()
        sys.exit(0)
    if len(sys.argv) < 2:
        sys.exit('Usage: profile-menu-ui-check.py <modlock-binary>')
    run_suite(sys.argv[1])
