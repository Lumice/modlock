"""Actual launch-settings editor after stored/unstored lost replies, offline."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import shutil

for mode in ('lost-stored', 'lost-unstored', 'open-refusal', 'stale', 'refused', 'refused-busy'):
    with tempfile.TemporaryDirectory(prefix='s6-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        for number, key in enumerate(sorted(set(k for k in env if k.startswith('XDG_')) | {'HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR'})):
            path = root/str(number)
            path.mkdir(mode=0o700)
            env[key] = str(path)
        library = root/'library'
        library.mkdir()
        (library/'mode').write_text(mode)
        if mode != 'open-refusal': (library/'hold-read').touch()
        pending = Path(env['XDG_CONFIG_HOME'])/'dl-mod-manager/state/pending'
        pending.mkdir(parents=True)
        key = hashlib.sha256(str(library).encode()).hexdigest()
        sentinels = [pending/(key+'.launch-settings.json'+suffix) for suffix in ('', '.lock')]
        expected = b'untouched legacy sentinel\n'
        for path in sentinels:
            path.write_bytes(expected)
            os.utime(path, ns=(1_000_000_000, 2_000_000_000))
        def stamp(path):
            info = path.lstat()
            return (info.st_dev, info.st_ino, info.st_mode, info.st_nlink, info.st_size,
                    info.st_atime_ns, info.st_mtime_ns, info.st_ctime_ns)
        before = [stamp(path) for path in sentinels]
        env |= {'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software', 'STEP6_MODE': mode}
        env.pop('DISPLAY', None)
        env.pop('WAYLAND_DISPLAY', None)
        command = [sys.argv[1], '--helper', str(Path(__file__).with_name('launch-settings-peer.py')),
                   '--data-dir', str(library), '--check-ui', 'step6-launch-settings']
        access = root/'access.log'
        if shutil.which('strace'):
            command = ['strace', '-f', '-qq', '-e', 'trace=%file', '-o', str(access)] + command
        result = subprocess.run(command,
                                env=env, capture_output=True, text=True, timeout=25)
        assert result.returncode == 0, (mode, result.returncode, result.stdout, result.stderr)
        if access.exists():
            assert '.launch-settings.json' not in access.read_text(), 'App inspected an old settings reference/lock'
        assert before == [stamp(path) for path in sentinels], 'Old settings reference/lock stats changed'
        assert all(path.read_bytes() == expected for path in sentinels), 'Old settings reference/lock bytes changed'
        calls = [json.loads(line)['method'] for line in (library/'calls.jsonl').read_text().splitlines()]
        assert calls.count('native.launch.settings.set') == 1
        assert 'native.launch.settings.operation' not in calls
        print('PASS STEP6 editor', mode, flush=True)
