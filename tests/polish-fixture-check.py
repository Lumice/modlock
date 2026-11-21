#!/usr/bin/env python3
"""Focused UI checks on a disposable review-fixture root with retained artwork.

Usage: python3 tests/polish-fixture-check.py APP FIXTURE
Create FIXTURE with tools/review-fixture.py. The grid case requires retained
preview bytes (warm its artwork cache before running with networking disabled)
and a native display session to exercise compositor frame suspension.
No production data directory is accepted.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

app, fixture = map(lambda p: Path(p).resolve(), sys.argv[1:])
if not (fixture / 'environment.json').exists() or not (fixture / '.deadlock-review-fixture').is_file():
    raise SystemExit('Expected a marked disposable review fixture')
env = dict(os.environ, **json.loads((fixture / 'environment.json').read_text()),
           QT_QPA_PLATFORM='offscreen', QT_FORCE_STDERR_LOGGING='1')
for scenario in ['grid-previews', 'updates-offline', 'autoexec-managed', 'downloads-page']:
    scenario_env = env.copy()
    # Exercise native compositor frame suspension through the screenshot path.
    # Offscreen renders continuously and concealed the original regression.
    if scenario == 'grid-previews':
        scenario_env.pop('QT_QPA_PLATFORM', None)
    p = subprocess.run([str(app), '--check-ui', scenario], env=scenario_env, capture_output=True, text=True, timeout=30)
    print(p.stdout, end='', flush=True)
    if p.returncode:
        raise SystemExit(f'{scenario}: {p.returncode}\n{p.stderr}')
