#!/usr/bin/env python3
"""Exercise debounced library toggles against a disposable game and helper."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import shutil
import struct
import subprocess
import tempfile
import time
from types import SimpleNamespace

from nativeintegrationcheck import FamilyUi

compat_path = Path(__file__).with_name('game-format-cross-version-check.py')
spec = importlib.util.spec_from_file_location('compat', compat_path)
compat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compat)

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--helper', type=Path, required=True)
parser.add_argument('--case', choices=('performance', 'normal', 'warning', 'during', 'off', 'external', 'bulk', 'bulk-mixed', 'error', 'review', 'restore', 'fonts', 'gate', 'gate-stale', 'settings-busy', 'order', 'variant', 'variant-off', 'variant-warning', 'game-toggle', 'game-off', 'game-quit', 'install-enable', 'install-warning'))
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='dlmm-auto-apply-') as temp:
    root = Path(temp)
    for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
                'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'):
        path = root / key
        path.mkdir(mode=0o700)
        os.environ[key] = str(path)
    for case in ('performance', 'normal', 'warning', 'during', 'off', 'external', 'bulk', 'bulk-mixed', 'error', 'review', 'restore', 'fonts', 'gate', 'gate-stale', 'settings-busy', 'order', 'variant', 'variant-off', 'variant-warning', 'game-toggle', 'game-off', 'game-quit', 'install-enable', 'install-warning'):
        if args.case and args.case != case: continue
        warning = case in ('warning', 'variant-warning', 'install-warning')
        fixture = FamilyUi(SimpleNamespace(helper=args.helper, binary=args.binary, capture_dir=None),
                           root / case, case)
        if case == 'performance':
            gameinfo = fixture.game / 'game/citadel/gameinfo.gi'
            text = gameinfo.read_text()
            start, end = text.rsplit('}', 1)
            gameinfo.write_text(start + '\n ConVars\n {\n }\n}' + end)
        if case == 'order':
            source = json.loads(fixture.source.read_text())
            state = source['local-config']['state']
            state['localMods'] = [row for row in state['localMods'] if row['id'] != 'missing']
            state['profiles']['default']['mods'] = [row for row in state['profiles']['default']['mods'] if row['id'] != 'missing']
            fixture.source.write_text(json.dumps(source))
            fixture.paths[3].write_bytes(fixture.disabled.replace(b'models', b'modalt').replace(b'materials', b'otherpath').replace(b'textures', b'othertex').replace(b'sounds', b'otherx'))
        if case == 'bulk': fixture.paths[3].write_bytes(fixture.active)
        if warning:
            source = json.loads(fixture.source.read_text())
            state = source['local-config']['state']
            for row in state['localMods'] + state['profiles']['default']['mods']:
                if row['id'] == 'local-a': row['isBlacklisted'] = True
            fixture.source.write_text(json.dumps(source))
        helper = compat.Helper(args.helper, fixture.data)
        preview = helper.ok('native.importPreview', statePath=str(fixture.source), gamePath=str(fixture.game))
        helper.ok('native.importCommit', token=preview['token'], operationId='import', expectedRevision='0')
        variant_mod = None
        if case.startswith('variant'):
            folder = root / case / 'variant-input'; folder.mkdir()
            (folder / 'first_dir.vpk').write_bytes(compat.vpk())
            (folder / 'second_dir.vpk').write_bytes(compat.vpk().replace(b'tiny', b'more'))
            job = 'a' * 32
            helper.ok('native.local.prepare', jobId=job, profileId='default', expectedRevision='1', roots=[str(folder)])
            deadline = time.monotonic() + 20
            while True:
                status = helper.ok('native.local.status', jobId=job)
                assert status['phase'] not in ('failed', 'interrupted'), status
                if status['phase'] == 'ready': break
                assert time.monotonic() < deadline, status
                time.sleep(.02)
            candidates = helper.ok('native.local.status', jobId=job, includeCandidates=True)['candidateSummary']
            assert len(candidates['defaultSelectedCandidateIds']) == 2, candidates
            review = helper.ok('native.local.review', jobId=job, candidateIds=sorted(candidates['defaultSelectedCandidateIds']), metadata=candidates['defaultMetadata'])
            variant_mod = review['modId']
            helper.ok('native.local.commit', jobId=job, token=review['token'], profileId='default', expectedRevision='1', operationId='b' * 32)
            helper.ok('native.setEnabled', profileId='default', modId=variant_mod, enabled=True, expectedRevision='2', operationId='variant-enable')
        elif case.startswith('install'):
            pass  # local-a stays enabled and deployed; the UI installs an overlapping mod.
        elif case != 'order':
            helper.ok('native.setEnabled', profileId='default', modId='local-a', enabled=False,
                      operationId='disable', expectedRevision='1')
        else:
            helper.ok('native.setEnabled', profileId='default', modId='local-b', enabled=True,
                      operationId='enable-second', expectedRevision='1')
        started = time.monotonic()
        preview = helper.ok('native.applyPreview', profileId='default', gamePath=str(fixture.game), deploymentLayout='shards-v1')
        preview_ms = (time.monotonic() - started) * 1000
        started = time.monotonic()
        helper.ok('native.apply', token=preview['token'], operationId='initial', expectedRevision='3' if case.startswith('variant') else '1' if case.startswith('install') else '2', deploymentLayout='shards-v1')
        apply_ms = (time.monotonic() - started) * 1000
        restore_gameinfo = fixture.info.read_bytes()  # The next Apply checkpoints this setup.
        helper.close()
        scenario = {'performance': 'native-auto-performance', 'game-toggle': 'native-auto-game-toggle', 'game-off': 'native-auto-game-off', 'game-quit': 'native-auto-game-quit', 'warning': 'native-auto-warning', 'during': 'native-auto-during-apply',
                    'off': 'native-auto-off', 'external': 'native-auto-external', 'bulk': 'native-auto-bulk',
                    'bulk-mixed': 'native-auto-bulk-mixed', 'error': 'native-auto-error',
                    'review': 'native-auto-restore-review', 'restore': 'native-auto-restore-run', 'fonts': 'native-auto-fonts', 'gate': 'native-auto-gate', 'gate-stale': 'native-auto-gate', 'settings-busy': 'native-settings-busy', 'order': 'native-auto-order', 'install-enable': 'native-auto-install-enable', 'install-warning': 'native-auto-install-enable'}.get(case, 'native-auto-variant' if case.startswith('variant') else 'native-auto-apply')
        ui_helper = args.helper
        environment = fixture.env.copy()
        if variant_mod:
            environment['UI_VARIANT_MOD'] = variant_mod
            environment['UI_VARIANT_CASE'] = case
        if case.startswith('install'):
            if case == 'install-warning': environment['UI_INSTALL_WARNING'] = '1'
            # Same resource paths as local-a with different content: an ordinary overlap.
            folder = root / case / 'install-input'; folder.mkdir()
            tree = b''
            for path, crc in (('materials/a.vmat_c', 99), ('textures/inline.vtex_c', 98)):
                directory, file = path.rsplit('/', 1); name, extension = file.rsplit('.', 1)
                tree += extension.encode() + b'\0' + directory.encode() + b'\0' + name.encode() + b'\0'
                tree += struct.pack('<IHHIIH', crc, 0, 0x7fff, 0, 4, 0xffff) + b'\0\0'
            tree += b'\0'
            (folder / 'overlap_dir.vpk').write_bytes(struct.pack('<IIIIIII', 0x55aa1234, 2, len(tree), 4, 0, 0, 0) + tree + b'body')
            environment['UI_INSTALL_FOLDER'] = str(folder)
        if case in ('during', 'error', 'gate', 'gate-stale', 'settings-busy'):
            ui_helper = root / 'delayed-helper.py'
            ui_helper.write_text('''#!/usr/bin/python3\nimport json, os, subprocess, sys, threading, time\np = subprocess.Popen([os.environ["DMM_REAL_HELPER"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)\ninput_lock = threading.Lock()\ngate_armed = threading.Event()\ndef forward(line):\n    with input_lock: p.stdin.write(line); p.stdin.flush()\ndef output():\n    for line in p.stdout:\n        sys.stdout.write(line); sys.stdout.flush()\nthreading.Thread(target=output, daemon=True).start()\nfor line in sys.stdin:\n    request = json.loads(line)\n    trace = os.environ.get("DMM_GATE_TRACE")\n    if trace:\n        with open(trace, "a") as out: out.write(request.get("method", "") + "\\n")\n    if os.environ.get("DMM_REJECT_TOGGLE") and request.get("method") == "native.setEnabled":\n        print(json.dumps({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32020,"message":"Synthetic failure","data":{"kind":"nativeIo"}}}),flush=True); continue\n    marker = os.environ.get("DMM_GATE_MARKER", "")\n    if marker and request.get("method") == "native.applyPreview":\n        def preview(line=line):\n            if not gate_armed.wait(15): os._exit(71)\n            forward(line)\n        threading.Thread(target=preview, daemon=True).start(); continue\n    if os.environ.get("DMM_GATE_CASE") and request.get("method") == "native.applyPreview": time.sleep(.5)\n    if marker and os.path.exists(marker) and request.get("method") == "native.launch.settings.get":\n        # Hold admission without blocking preview requests behind this line.\n        gate_armed.set()\n        def release(line=line, marker=marker):\n            deadline = time.monotonic() + 15\n            while os.path.exists(marker):\n                if time.monotonic() >= deadline: os._exit(70)\n                time.sleep(.01)\n            forward(line)\n        threading.Thread(target=release, daemon=True).start(); continue\n    if request.get("method") == "native.apply": time.sleep(1)\n    forward(line)\np.stdin.close(); p.wait()\n''')
            ui_helper.chmod(0o700)
            environment['DMM_REAL_HELPER'] = str(args.helper)
            if case == 'error': environment['DMM_REJECT_TOGGLE'] = '1'
            if case == 'settings-busy': environment['DMM_GATE_CASE'] = '1'
            if case in ('gate', 'gate-stale'):
                environment['DMM_GATE_CASE'] = '1'
                environment['DMM_GATE_MARKER'] = environment['UI_NATIVE_GATE_MARKER'] = str(root / 'hold-launch')
            if case == 'gate-stale':
                environment['UI_NATIVE_GATE_REFRESH'] = '1'
                environment['DMM_GATE_TRACE'] = str(root / 'gate-trace')
        command = [str(args.binary), '--helper', str(ui_helper), '--data-dir', str(fixture.data), '--check-ui', scenario]
        if case in ('game-toggle', 'game-off', 'game-quit'):
            executable = root / case / 'deadlock.exe'
            shutil.copyfile('/bin/sleep', executable); executable.chmod(0o700)
            game_process = subprocess.Popen([str(executable), '60'], env=environment)
            signal = root / case / 'running-edit-ready'
            environment['UI_RUNNING_EDIT_READY'] = str(signal)
            app = subprocess.Popen(command, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            try:
                if case == 'game-quit':
                    out, err = app.communicate(timeout=5); assert game_process.poll() is None, 'game stopped during shutdown'
                    assert app.returncode == 0 and 'PASS UI' in out, out + err
                    print('PASS', scenario, flush=True); continue
                deadline = time.monotonic() + 20
                while not signal.exists() and app.poll() is None and time.monotonic() < deadline:
                    time.sleep(.05)
                if app.poll() is not None:
                    out, err = app.communicate(); raise AssertionError(out + err)
                assert signal.exists(), 'running game observation did not complete'
                with sqlite3.connect(fixture.data / 'library.sqlite3') as db:
                    during = [json.loads(row[0]) for row in db.execute('SELECT input FROM operations')]
                assert sum(entry.get('method') == 'native.apply' for entry in during) == 1, during
                assert sum(entry.get('method') == 'native.setEnabled' for entry in during) == 1, during
                game_process.terminate(); game_process.wait(timeout=5)
                out, err = app.communicate(timeout=30)
                run = SimpleNamespace(returncode=app.returncode, stdout=out, stderr=err)
            finally:
                if game_process.poll() is None: game_process.kill(); game_process.wait()
                if app.poll() is None: app.kill(); app.communicate()
        else:
            run = subprocess.run(command, env=environment, capture_output=True, text=True, timeout=30)
        assert run.returncode == 0 and 'PASS UI' in run.stdout, run.stdout + run.stderr
        for unwanted_warning in ('scheduleRestore is not a function', 'Binding loop detected for property "implicitHeight"'):
            assert unwanted_warning not in run.stderr, run.stderr
        with sqlite3.connect(fixture.data / 'library.sqlite3') as db:
            inputs = [json.loads(row[0]) for row in db.execute('SELECT input FROM operations')]
        applies = [entry for entry in inputs if entry.get('method') == 'native.apply']
        if case.startswith('install'):
            assert sum(entry.get('method') == 'native.local.commit' for entry in inputs) == 1, inputs
            helper = compat.Helper(args.helper, fixture.data)
            check = helper.ok('native.applyPreview', profileId='default', gamePath=str(fixture.game), deploymentLayout='shards-v1')
            helper.close()
            # The enabled set really overlaps; only the warning case has another warning.
            assert sorted({w['code'] for w in check['warnings']}) == (['blacklisted', 'fileConflict'] if warning else ['fileConflict']), check['warnings']
        assert len(applies) == (1 if warning or case in ('off', 'external', 'error', 'settings-busy', 'variant-off', 'game-off') else 3 if case in ('during', 'performance') else 2), applies
        if case in ('review', 'restore'):
            assert sum(entry.get('method') == 'native.restore' for entry in inputs) == (1 if case == 'restore' else 0), inputs
            with sqlite3.connect(fixture.data / 'library.sqlite3') as db:
                assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0
            if case == 'restore': assert fixture.info.read_bytes() == restore_gameinfo
        if case == 'gate-stale': assert Path(environment['DMM_GATE_TRACE']).read_text().splitlines().count('native.applyPreview') == 2
        if case == 'normal' or case.startswith('install'):
            print(f'SYNTHETIC_INITIAL_PREVIEW_MS {preview_ms:.1f} APPLY_MS {apply_ms:.1f}', flush=True)
            for line in run.stdout.splitlines():
                if line.startswith(('UI_TIMING_', 'TRACE_')): print(line, flush=True)
        print('PASS', scenario, flush=True)
