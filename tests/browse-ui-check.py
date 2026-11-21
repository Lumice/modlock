#!/usr/bin/env python3
"""Exercise Browse controls with disposable demo state."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

for scenario, mode in (('browse-filters', 'list'), ('browse-cards', 'list'), ('browse-cards', 'grid')):
    for width, height in ((1200, 780), (720, 520)):
        with tempfile.TemporaryDirectory(prefix='dl-browse-ui-') as temporary:
            root = Path(temporary)
            env = os.environ.copy()
            for variable, name in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                                   ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                                   ('XDG_RUNTIME_DIR', 'runtime')]:
                directory = root / name
                directory.mkdir(mode=0o700)
                env[variable] = str(directory)
            env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                       BROWSE_CARD_MODE=mode, UI_REVIEW_WIDTH=str(width), UI_REVIEW_HEIGHT=str(height))
            result = subprocess.run([sys.argv[1], '--demo', '--check-ui', scenario],
                                    env=env, capture_output=True, text=True, timeout=30)
            if result.returncode or 'PASS UI Browse' not in result.stdout:
                raise SystemExit(f'{scenario} {width}x{height}: {result.returncode}\n{result.stdout}{result.stderr}')
            print(f'{width}x{height}: {result.stdout.strip()}', flush=True)
