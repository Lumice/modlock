#!/usr/bin/env python3
"""Exercise Tweaks against demo data and a disposable performance peer."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

app = Path(sys.argv[1]).resolve()
peer_source = Path(__file__).with_name('controlled-helper.py').resolve()
for scenario in ('autoexec-actions', 'crosshair-narrow', 'performance-details'):
    with tempfile.TemporaryDirectory(prefix='dl-tweaks-ui-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        for variable, name in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                               ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                               ('XDG_RUNTIME_DIR', 'runtime')]:
            directory = root / name
            directory.mkdir(mode=0o700)
            env[variable] = str(directory)
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software')
        args = ['--demo']
        if scenario == 'performance-details':
            (root / 'native').mkdir()
            peer = root / 'native-performance'
            peer.symlink_to(peer_source)
            args = ['--helper', str(peer), '--data-dir', str(root / 'native')]
        result = subprocess.run([str(app), *args, '--check-ui', scenario], env=env,
                                capture_output=True, text=True, timeout=30)
        if result.returncode or 'PASS ' not in result.stdout:
            raise SystemExit(f'{scenario}: {result.returncode}\n{result.stdout}{result.stderr}')
        print(result.stdout.strip(), flush=True)
