#!/usr/bin/env python3
"""Run MediaCard ownership guards with short and 1,541-byte upstream IDs."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

from integrationcheck import raster

with tempfile.TemporaryDirectory(prefix='md-') as scratch:
    base = Path(scratch)
    env = {k: v for k, v in os.environ.items() if not k.startswith('XDG_') and k not in ('DISPLAY', 'WAYLAND_DISPLAY')}
    for i, key in enumerate(('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'TMPDIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS')):
        folder = base / str(i)
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', QML_DISABLE_DISK_CACHE='1')
    image = base / 'preview.png'
    raster(image, (210, 45, 65))
    helper = Path(__file__).with_name('library-media-peer.py').resolve()
    for identity in (('界'*513 + 'ab',) if '--long-only' in sys.argv else ('row', '界'*513 + 'ab')):
        roots = [base / ('r' + str(len(identity)) + suffix) for suffix in ('a', 'b')]
        for root in roots:
            root.mkdir()
            snapshot = dict(protocolVersion=1, sourceKind='native', storeVersion=1, revision='1',
                sourcePath=str(root / 'library.json'), activeProfileId='default', selectedProfileId='default',
                pendingChanges=False, canRestore=False, recoveryRequired=False, gamePath='', appliedProfileId=None,
                profiles=[dict(id='default', name='Default', isActive=True, modCount=2)], warnings=[],
                mods=[dict(id=id_, remoteId=remote, title=id_[:10], author='Fixture', hero='Ivy', category='Skins',
                    status='installed', enabled=True, appliedEnabled=False, available=True, unavailableReason=None,
                    installOrder=None, installedVpks=[], missingVpks=[]) for id_, remote in [('anchor', ''), (identity, '1000')]])
            (root / 'snapshot.json').write_text(json.dumps(snapshot))
        env.update(UI_DEMAND_ID=identity, UI_DEMAND_ROOT1=str(roots[0]), UI_PREVIEW_FIRST=image.as_uri(), UI_DEMAND_ROOT2=str(roots[1]), UI_DEMAND_HELPER=str(helper))
        result = subprocess.run([sys.argv[1], '--demo', '--rows', '2', '--check-ui', 'media-demand'], env=env, timeout=20)
        if result.returncode:
            raise SystemExit(result.returncode)
        print('PASS demand identity UTF-8 bytes:', len(identity.encode()), flush=True)
