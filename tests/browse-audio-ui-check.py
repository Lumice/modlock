#!/usr/bin/env python3
"""Browse sound previews through the normal QML controls, dark and light.

Uses disposable demo state, offscreen software rendering, a fake network
transport and a fake audio output (see tests/browseaudiocheck.h). Set
BROWSE_AUDIO_CAPTURE_DIR to keep window captures."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

fixtures = Path(__file__).resolve().parent / 'fixtures' / 'audio'
for theme in ('dark', 'light'):
    with tempfile.TemporaryDirectory(prefix='dl-browse-audio-') as temporary:
        root = Path(temporary)
        env = os.environ.copy()
        for variable, name in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'),
                               ('XDG_DATA_HOME', 'data'), ('XDG_CACHE_HOME', 'cache'),
                               ('XDG_RUNTIME_DIR', 'runtime')]:
            directory = root / name
            directory.mkdir(mode=0o700)
            env[variable] = str(directory)
        env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                   AUDIO_THEME=theme, AUDIO_FIXTURES=str(fixtures))
        env.pop('AUDIO_CAPTURE_DIR', None)
        if os.environ.get('BROWSE_AUDIO_CAPTURE_DIR'):
            env['AUDIO_CAPTURE_DIR'] = os.environ['BROWSE_AUDIO_CAPTURE_DIR']
        result = subprocess.run([sys.argv[1], '--demo', '--check-ui', 'browse-audio'],
                                env=env, capture_output=True, text=True, timeout=60)
        if result.returncode or 'PASS UI Browse audio' not in result.stdout:
            raise SystemExit(f'{theme}: {result.returncode}\n{result.stdout}{result.stderr}')
        print(result.stdout.strip(), flush=True)
