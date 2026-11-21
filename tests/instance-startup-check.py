#!/usr/bin/env python3
"""Discovery and a held instance lock must fail promptly and plainly."""
import fcntl
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading

app = Path(sys.argv[1]).resolve()
root = Path(tempfile.mkdtemp(prefix='i117-'))
try:
    env = {'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'QT_QPA_PLATFORM': 'offscreen',
           'QT_QUICK_BACKEND': 'software', 'QT_FORCE_STDERR_LOGGING': '1',
           'MODLOCK_UI_TEST_ROOT': str(root), 'PYTHONDONTWRITEBYTECODE': '1'}
    for key in ['HOME', 'TMPDIR', 'XDG_RUNTIME_DIR', 'XDG_DATA_HOME',
                'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME',
                'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS']:
        # Shorten the runtime leaf; the full socket path is checked below.
        folder = root / ('r' if key == 'XDG_RUNTIME_DIR' else key)
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env['DBUS_SESSION_BUS_ADDRESS'] = 'unix:path=' + str(root / 'no-bus')
    # Refuse driver dispatch without a sandbox or with a library that resolves
    # outside it, even when all of the usual application paths are disposable.
    unmarked = env.copy()
    del unmarked['MODLOCK_UI_TEST_ROOT']
    with tempfile.TemporaryDirectory(prefix='modlock-outside-', dir=root.parent) as outside:
        escape = root / 'outside'
        escape.symlink_to(outside, target_is_directory=True)
        for driver_env, args in [(unmarked, ['--demo']), (env, ['--data-dir', str(escape / 'library')])]:
            run = subprocess.run([str(app), '--check-ui', 'runtime-paths', *args],
                                 env=driver_env, capture_output=True, text=True, timeout=8)
            assert run.returncode == 1 and 'UI tests require disposable paths' in run.stderr, run.stdout + run.stderr
        assert not list(Path(outside).iterdir())
    selected_helper = root / 'selected-helper'
    for name, program, message in [
            ('helper-error', 'import sys\nsys.stderr.write("Cannot inspect <library> & saved recovery.\\n")\nsys.exit(1)\n',
             'Cannot inspect <library> & saved recovery.\nPass --data-dir <folder> to choose the library explicitly.'),
            ('helper-empty', 'print(\'{"root":""}\')\n',
             'The selected helper did not return a library folder. Pass --data-dir <folder> to choose it explicitly.'),
            ('helper-missing', None,
             "The selected helper couldn't discover the library folder. Pass --data-dir <folder> to choose it explicitly.")]:
        if program is None:
            selected_helper.unlink()
        else:
            selected_helper.write_text('#!/usr/bin/python3\n' + program)
            selected_helper.chmod(0o700)
        run = subprocess.run([str(app), '--helper', str(selected_helper)],
                             env=env, capture_output=True, text=True, timeout=8)
        assert run.returncode == 1, run.stdout + run.stderr
        assert message in run.stderr, run.stderr
        for key in ['XDG_DATA_HOME', 'XDG_CONFIG_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR']:
            assert list(Path(env[key]).iterdir()) == [], (name, key)
        print('PASS selected discovery', name, flush=True)
    library = root / 'library'
    directory = Path(env['XDG_RUNTIME_DIR']) / 'deadlock-native'
    directory.mkdir(mode=0o700)
    socket_path = directory / hashlib.sha256(str(library).encode()).hexdigest()[:24]
    # Linux sun_path has 108 bytes, including the terminating NUL.
    assert len(os.fsencode(socket_path)) < 108, 'AF_UNIX socket path is too long for this fixture; use a shorter TMPDIR.'
    lock = socket_path.with_name(socket_path.name + '.lock')
    message = "Another copy of Modlock, or the older DL Mod Manager, is already running but isn't responding. Close it, then open Modlock again."
    with lock.open('w') as held:
        fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
        # Valid Qt metadata for this live holder; no owner identification needed.
        held.write(f'{os.getpid()}\n{Path(sys.executable).resolve().name}\n{os.uname().nodename}\n')
        held.flush()
        for name, args in [('offscreen', []), ('check-ui', ['--check-ui', 'native-install-link-forwarded'])]:
            run = subprocess.run([str(app), '--data-dir', str(library)] + args,
                                 env=env, capture_output=True, text=True, timeout=5)
            assert run.returncode == 1, run.stderr
            assert "Could not contact or start the app's private instance socket." in run.stderr, run.stderr
            assert message in run.stderr, run.stderr
            print('PASS', name, flush=True)
        # Automated runs skip the instance broker, so the held lock must not
        # stop them; each one reaches its own result instead.
        helper = root / 'native-links'
        helper.symlink_to(Path(__file__).with_name('controlled-helper.py'))
        missing = ['--helper', str(root / 'missing-helper')]
        library.mkdir()
        for name, args, code, output in [
                ('smoke-test', missing + ['--smoke-test'], 13, ''),
                ('screenshot', missing + ['--screenshot', str(root / 'shot.png')], 0, 'UI check finished'),
                ('check-animation', missing + ['--check-animation'], 0, 'PASS: test strip visibly moved'),
                ('check-ui', ['--helper', str(helper), '--check-ui', 'native-install-link-invalid',
                              'dlmm:https://evil.test/mmdl/12,Mod,42'], 0, 'PASS UI')]:
            run = subprocess.run([str(app), '--data-dir', str(library)] + args,
                                 env=env, capture_output=True, text=True, timeout=30)
            assert "private instance socket" not in run.stderr, (name, run.stderr)
            assert run.returncode == code and output in run.stdout + run.stderr, (name, run.returncode, run.stdout, run.stderr)
            print('PASS automated', name, flush=True)
    # Demo uses its own discovered library; once its window exists, no
    # instance socket may have been opened for it.
    demo = subprocess.Popen([str(app), '--demo', '--rows', '3'], env=env | {'QT_LOGGING_RULES': 'qt.scenegraph.general=true'},
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    try:
        timer = threading.Timer(15, demo.kill)
        timer.start()
        seen = []
        for line in demo.stderr:
            seen.append(line)
            if 'qt.scenegraph.general: Loading backend' in line:
                break
        timer.cancel()
        assert seen and 'Loading backend' in seen[-1], ''.join(seen)
        assert sorted(p.name for p in directory.iterdir()) == [lock.name], sorted(directory.iterdir())
        print('PASS automated demo', flush=True)
    finally:
        demo.kill()
        demo.communicate()
finally:
    shutil.rmtree(root)
