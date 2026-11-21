#!/usr/bin/env python3
"""Capture active/empty Downloads without a native library or network owner."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix='downloads-ui-') as temporary:
    root = Path(temporary)
    env = os.environ.copy()
    keys = {key for key in env if key.startswith('XDG_')} | {
        'HOME', 'TMPDIR', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
        'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS',
    }
    for key in keys:
        folder = root / key
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    captures = Path(env.get('UI_NATIVE_CAPTURES', str(root / 'screenshots')))
    explicit = any(env.get(key, '0') not in ('', '0') for key in ['DOWNLOADS_RETAINED', 'DOWNLOADS_ERROR'])
    cases = [('', {})] if explicit else [
        ('', {'DOWNLOADS_RETAINED': '0', 'DOWNLOADS_ERROR': '0'}),
        ('retained', {'DOWNLOADS_RETAINED': '1', 'DOWNLOADS_ERROR': '0'}),
        ('profile-error', {'DOWNLOADS_RETAINED': '1', 'DOWNLOADS_ERROR': '1'}),
        ('refresh-error', {'DOWNLOADS_RETAINED': '1', 'DOWNLOADS_ERROR': '2'}),
    ]
    for name, overrides in cases:
        folder = captures / name
        folder.mkdir(parents=True, exist_ok=True)
        current = env | overrides | {'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                                    'UI_NATIVE_CAPTURES': str(folder)}
        result = subprocess.run([sys.argv[1], '--demo', '--rows', '1', '--check-ui',
                                 'downloads-active'], env=current, text=True,
                                capture_output=True, timeout=30)
        print(result.stdout + result.stderr, end='')
        assert result.returncode == 0 and 'PASS UI downloads' in result.stdout, (name, result.returncode, result.stderr)
        for image in ['active-narrow', 'active-wide', 'empty-narrow', 'empty-wide']:
            assert (folder / ('downloads-' + image + '.png')).is_file()
