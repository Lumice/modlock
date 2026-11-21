#!/usr/bin/env python3
"""Check fresh defaults and restoration in a separate app process."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix='appearance-') as scratch:
    env = os.environ.copy()
    for key in ('WAYLAND_DISPLAY', 'DISPLAY', 'QML_DISK_CACHE_PATH'):
        env.pop(key, None)
    for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
                'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'):
        folder = Path(scratch) / key
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software')
    for restart in (False, True):
        if restart:
            env['APPEARANCE_RESTART'] = '1'
        run = subprocess.run([sys.argv[1], '--demo', '--check-ui', 'appearance-preferences'],
                             env=env, capture_output=True, text=True, timeout=25)
        assert run.returncode == 0, run.stdout + run.stderr
        print(run.stdout)
