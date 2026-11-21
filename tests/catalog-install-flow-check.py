#!/usr/bin/env python3
"""Run a download and toggle against the isolated controlled helper."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

source = Path(__file__).with_name('controlled-helper.py').read_text()
source = source.replace('import json\n', 'import sys\nsys.path.insert(0, '+repr(str(Path(__file__).resolve().parent))+')\nimport json\n', 1)
changes = [
    ("remote_revision = 1", "remote_revision = 1\nremote_enabled = False"),
    ("'native.operation','native.modUpdates.get') or method.startswith('native.remote.')", "'native.operation','native.modUpdates.get','native.setEnabled') or method.startswith('native.remote.')"),
    ("enabled=False,remoteSubmission=submission", "enabled=remote_enabled,remoteSubmission=submission"),
    ("        if mode.startswith('native-local'):\n            result['selectedProfileId']", """        if mode == 'native-links' and method == 'native.setEnabled':
            assert params['modId'] == 'installed-variant'
            remote_enabled = params['enabled']
            remote_revision += 1
            result['revision'] = str(remote_revision)
            result['mods'][-1]['enabled'] = remote_enabled
            send(request_id, result)
            continue
        if mode.startswith('native-local'):
            result['selectedProfileId']"""),
]
for before, after in changes:
    assert source.count(before) == 1, before
    source = source.replace(before, after)
with tempfile.TemporaryDirectory(prefix='catalog-install-') as temp:
    root = Path(temp)
    helper = root / 'native-links'
    helper.write_text(source)
    helper.chmod(0o755)
    data = root / 'native'
    data.mkdir()
    env = os.environ | {'CATALOG_INSTALL_HELPER': str(helper), 'CATALOG_INSTALL_DATA': str(data),
                        'INSTALL_LINK_AUTO': '1', 'INSTALL_LINK_CASE': 'bubble', 'INSTALL_LINK_CANDIDATES': '1'}
    subprocess.run([sys.argv[1]], env=env, check=True, timeout=60)
