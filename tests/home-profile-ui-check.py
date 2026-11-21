#!/usr/bin/env python3
"""Assert Home profile claims with a synthetic peer; optionally retain captures."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

cases = [
    ('viewed-choices', 'Choose files for the selected profile', 'Review library'),
    ('active-choices', 'Choose files for some mods before playing', 'Review library'),
    ('viewed-changed', 'Apply the selected profile', 'Apply…'),
    ('viewed-identical', 'Apply the selected profile', 'Apply…'),
    ('viewed-unapplied', 'Apply the selected profile', 'Apply…'),
    ('active-identical', 'Ready to play', 'Play'),
    ('active-unapplied', 'Changes waiting for your next game', 'Play'),
]
with tempfile.TemporaryDirectory(prefix='h127-') as scratch:
    for index, (case, sentence, action) in enumerate(cases):
        for theme in ('dark', 'light'):
            root = Path(scratch) / (str(index) + theme[0])
            env = {k: v for k, v in os.environ.items()
                   if not k.startswith('XDG_') and k not in ('DISPLAY', 'WAYLAND_DISPLAY', 'QML_DISK_CACHE_PATH')}
            for key_index, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'TMPDIR')):
                folder = root / str(key_index)
                folder.mkdir(parents=True, mode=0o700)
                env[key] = str(folder)
            settings = Path(env['XDG_CONFIG_HOME']) / 'modlock' / 'settings.conf'
            settings.parent.mkdir()
            settings.write_text('[display]\nautomaticApply=false\nreduceMotion=true\n')
            env['MODLOCK_UI_TEST_ROOT'] = str(scratch)
            env.update(XDG_CONFIG_DIRS=env['XDG_CONFIG_HOME'], XDG_DATA_DIRS=env['XDG_DATA_HOME'],
                       QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software',
                       HOME_PROFILE_CASE=case, HOME_PROFILE_THEME=theme, HOME_PROFILE_SENTENCE=sentence, HOME_PROFILE_ACTION=action)
            capture = os.environ.get('HOME_PROFILE_CAPTURES')
            if capture:
                folder = Path(capture) / (theme + '-' + case)
                folder.mkdir(parents=True, exist_ok=True)
                env['UI_NATIVE_CAPTURES'] = str(folder)
            peer = root / ('native-local-home-' + case)
            peer.symlink_to(Path(__file__).with_name('model-helper.py').resolve())
            result = subprocess.run([sys.argv[1], '--helper', str(peer), '--data-dir', str(root / 'native'),
                                     '--check-ui', 'home-profile'], env=env, capture_output=True, text=True, timeout=20)
            if not (result.returncode == 0):
                sys.exit(repr((case, theme, result.stdout, result.stderr, (root / 'native' / 'trace.jsonl').read_text())))
            requests = [json.loads(line) for line in (root / 'native' / 'trace.jsonl').read_text().splitlines()]
            previews = [r for r in requests if r.get('method') == 'native.applyPreview']
            expected_preview = case.startswith('viewed-') and case != 'viewed-choices'
            if not (len(previews) == int(expected_preview)):
                sys.exit(repr((case, previews)))
            if previews:
                responses = {(r.get('pid'), r.get('id')) for r in requests if r.get('event') == 'response'}
                if (previews[0]['pid'], previews[0]['id']) not in responses:
                    sys.exit('Home exited before Apply preview acknowledgement: ' + case)
                if not (previews[0]['params']['profileId'] == 'fast'):
                    sys.exit(repr(previews))
            print('PASS Home profile', case, theme, flush=True)
