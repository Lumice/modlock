#!/usr/bin/env python3
"""Check the Browse hero picker in isolated demo sessions."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

for width, height in [(1200, 780), (720, 520)]:
    with tempfile.TemporaryDirectory(prefix='dl-hero-ui-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        for variable, name in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                               ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                               ('XDG_RUNTIME_DIR', 'runtime')]:
            directory = root / name
            directory.mkdir(mode=0o700)
            env[variable] = str(directory)
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                   UI_REVIEW_WIDTH=str(width), UI_REVIEW_HEIGHT=str(height))
        result = subprocess.run([sys.argv[1], '--demo', '--check-ui', 'browse-hero-picker'],
                                env=env, capture_output=True, text=True, timeout=30)
        if result.returncode or 'PASS UI searchable hero picker' not in result.stdout:
            raise SystemExit(f'{width}x{height}: {result.returncode}\n{result.stdout}{result.stderr}')
        print(f'{width}x{height}: {result.stdout.strip()}', flush=True)
