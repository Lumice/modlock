#!/usr/bin/env python3
"""Focused UI checks for Modlock 08 motion touches in dark and light palettes."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

sys.path.insert(0, str(Path(__file__).resolve().parent))
from paired_helper import _controls, hello as paired_hello, observe, startup as paired_startup  # noqa: E402


def run_peer() -> None:
    seq = 0
    cancel_file = None

    def load_cancelled() -> set[str]:
        if cancel_file and cancel_file.exists():
            return set(json.loads(cancel_file.read_text()))
        return set()

    def mark_cancelled(attempt_id: str) -> None:
        if not cancel_file:
            return
        ids = load_cancelled()
        ids.add(attempt_id)
        cancel_file.write_text(json.dumps(sorted(ids)))

    def make_attempt(epoch: str, attempt_id: str, rev: str, is_cancelled: bool) -> dict:
        return dict(
            version=1,
            epoch=epoch,
            attemptId=attempt_id,
            recordRevision=rev,
            state='terminal' if is_cancelled else 'admitted',
            ready=None,
            preparation=None,
            handoff=None,
            outcome=dict(status='cancelled', reason='beforeDispatch') if is_cancelled else None,
        )

    for raw in sys.stdin:
        req = json.loads(raw)
        method = req['method']
        p = req.get('params', {})
        root = p.get('dataDir', '')
        if root and cancel_file is None:
            cancel_file = Path(root) / 'peer-cancelled.json'

        if method == 'hello':
            result = paired_hello()
        elif method == 'native.game.detect':
            result = dict(detectedGamePath='/synthetic/game', candidates=['/synthetic/game'])
        elif method == 'native.snapshot':
            selected = p.get('profileId') or 'p1'
            mods = [
                dict(
                    id='m1', remoteId='1001', title='Midnight Haze', author='Studio',
                    hero='Haze', category='Skins', status='installed', enabled=True,
                    appliedEnabled=True, available=True, unavailableReason=None,
                    installOrder=0, installedVpks=[], missingVpks=[], addedAt='2026-10-25T10:00:00Z',
                ),
            ]
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
                profiles=[dict(id='p1', name='Default', isActive=True, modCount=len(mods))],
                mods=mods,
            )
        elif method == 'native.launch.begin':
            payload = {k: v for k, v in p.items() if k not in ('dataDir', 'controlId')}
            digest = hashlib.sha256(json.dumps(dict(rootPath=root, input=dict(method=method, params=payload)),
                                              sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
            is_cancelled = p['attemptId'] in load_cancelled()
            attempt = make_attempt(p['epoch'], p['attemptId'], '1', is_cancelled)
            _controls[p['controlId']] = dict(version=1, controlId=p['controlId'], state='succeeded',
                                             inputSha256=digest, value=attempt, error=None)
            result = dict(version=1, accepted=True, controlId=p['controlId'], inputSha256=digest)
        elif method == 'native.launch.query':
            payload = {k: v for k, v in p.items() if k not in ('dataDir', 'controlId')}
            digest = hashlib.sha256(json.dumps(dict(rootPath=root, input=dict(method=method, params=payload)),
                                              sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()).hexdigest()
            is_cancelled = p['attemptId'] in load_cancelled()
            attempt = make_attempt(p['epoch'], p['attemptId'], '2' if is_cancelled else '1', is_cancelled)
            _controls[p['controlId']] = dict(version=1, controlId=p['controlId'], state='succeeded',
                                             inputSha256=digest, value=attempt, error=None)
            result = dict(version=1, accepted=True, controlId=p['controlId'], inputSha256=digest)
        elif method == 'native.launch.status':
            seq += 1
            is_cancelled = p['attemptId'] in load_cancelled()
            attempt = make_attempt(p['epoch'], p['attemptId'], str(seq), is_cancelled)
            result = dict(
                version=1,
                epoch=p['epoch'],
                attemptId=p['attemptId'],
                sequence=str(seq),
                phase='cancelled' if is_cancelled else 'reviewing',
                attempt=attempt,
                preparationState='notConfirmed' if is_cancelled else 'reviewing',
                handoffState='notDispatched',
                observationState='notStarted',
                progress=None,
                route=None,
                launcher=None,
                error=None,
                cancelRequested=is_cancelled,
            )
        elif method == 'native.launch.cancel':
            mark_cancelled(p['attemptId'])
            result = dict(version=1, epoch=p['epoch'], attemptId=p['attemptId'], status='requested')
        else:
            handled, value = paired_startup(method, p)
            if not handled:
                sys.stderr.write(f'Unexpected method in motion-play peer: {method}\n')
                sys.exit(2)
            result = value

        observe(result)
        sys.stdout.write(json.dumps(dict(jsonrpc='2.0', id=req['id'], result=result)) + '\n')
        sys.stdout.flush()


def run_case(binary: str, case: str, theme: str, capture_dir: str | None) -> None:
    with tempfile.TemporaryDirectory(prefix=f'm08-{case}-') as scratch:
        root = Path(scratch)
        env = {
            k: v for k, v in os.environ.items()
            if not k.startswith('XDG_') and k not in ('DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'QML_DISK_CACHE_PATH')
        }
        for n, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
                                 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR')):
            p = root / str(n)
            p.mkdir(mode=0o700)
            env[key] = str(p)
        env['MODLOCK_UI_TEST_ROOT'] = str(root)
        env.update(
            QT_QPA_PLATFORM='offscreen',
            QT_QUICK_BACKEND='software',
            QT_FORCE_STDERR_LOGGING='1',
            MOTION_THEME=theme,
            DBUS_SESSION_BUS_ADDRESS='unix:path=' + str(root / 'no-bus'),
        )
        if capture_dir:
            env['MOTION_CAPTURE_DIR'] = capture_dir
        if case == 'motion-play-ignition':
            data_dir = root / 'native'
            data_dir.mkdir(parents=True)
            peer = root / 'motion-play-peer'
            peer.symlink_to(Path(__file__).resolve())
            args = [binary, '--helper', str(peer), '--data-dir', str(data_dir), '--check-ui', case]
        else:
            args = [binary, '--demo', '--rows', '30', '--check-ui', case]
        result = subprocess.run(args, env=env, capture_output=True, text=True, timeout=45)
        if result.returncode != 0 or 'PASS UI motion' not in result.stdout:
            sys.exit(f'{case} ({theme}) failed ({result.returncode}):\n{result.stdout}\n{result.stderr}')
        print(f'{case} {theme}: {result.stdout.strip()}', flush=True)


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(f'Usage: {sys.argv[0]} <modlock-binary> [scenario ...]')
    binary = sys.argv[1]
    scenarios = sys.argv[2:] or ['motion-enter-stagger', 'motion-toggle-flash', 'motion-play-ignition']
    capture_dir = os.environ.get('MOTION_CAPTURE_DIR') or None
    for scenario in scenarios:
        for theme in ('dark', 'light'):
            run_case(binary, scenario, theme, capture_dir)


if __name__ == '__main__':
    if sys.argv[1:] == ['--data-root']:
        print(json.dumps({'root': str(Path(os.environ.get('XDG_DATA_HOME') or str(Path.home() / '.local/share')) / 'modlock')}))
        sys.exit(0)
    if os.path.basename(sys.argv[0]) == 'motion-play-peer':
        run_peer()
        sys.exit(0)
    main()
