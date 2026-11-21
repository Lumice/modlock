#!/usr/bin/env python3
"""Verify displayed crosshair values against independently parsed disposable KV3."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
from types import SimpleNamespace
from nativeintegrationcheck import FamilyUi

# Saved keys evidenced by popup_settings.xml and citadel_hero_settings.lst.
FIELDS = dict(zip('themed pipGapStatic gapCollapse width height gap pipOpacity pipOutlineWidth pipOutlineGap pipOutlineOpacity dotSize dotOpacity dotOutlineWidth dotOutlineGap dotOutlineOpacity color.r color.g color.b outlineColor.r outlineColor.g outlineColor.b'.split(),
    ('crosshair_' + suffix for suffix in 'themed pip_gap_static pip_gap_collapse pip_width pip_height pip_gap pip_opacity pip_outline_border pip_outline_gap pip_outline_opacity dot_size dot_opacity dot_outline_border dot_outline_gap dot_outline_opacity color_r color_g color_b outline_color_r outline_color_g outline_color_b'.split())))

def expected_values(source):
    # Bounded fixture reader, independent of the helper's parser. Refuse any
    # unfamiliar crosshair key rather than silently excluding it from checks.
    base = re.search(r'\bbase\s*=\s*\{([^{}]*)\}', source)
    assert base, 'expected a scalar base object'
    def fields(body):
        raw = dict(re.findall(r'\b(crosshair_\w+)\s*=\s*"([^"\n]*)"', body))
        assert not raw.keys() - set(FIELDS.values()), raw.keys() - set(FIELDS.values())
        return {key: json.loads(raw[saved]) for key, saved in FIELDS.items() if saved in raw}
    inherited = fields(base[1])
    assert len(inherited) == len(FIELDS), 'fixture must specify all saved crosshair fields'
    heroes = {name: fields(body) for name, body in re.findall(r'\b(hero_\w+)\s*=\s*\{([^{}]*)\}', source)}
    return {hero: inherited | heroes.get(hero, {}) for hero in ('', 'hero_inferno', 'hero_doorman')}

spec = importlib.util.spec_from_file_location('compat', Path(__file__).with_name('game-format-cross-version-check.py'))
compat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(compat)
parser = argparse.ArgumentParser()
parser.add_argument('binary', type=Path)
parser.add_argument('helper', type=Path)
parser.add_argument('--captures', type=Path)
parser.add_argument('--config-copy', type=Path, help='Already copied KV3; never pass the real game path')
parser.add_argument('--regression', choices=['picker','legacy-import','legacy-export','dpi','gap','hex-colour'])
parser.add_argument('--scale', default='1')
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='crosshair-panel-') as temporary:
    root = Path(temporary)
    env = os.environ.copy()
    for key in ('HOME', 'TMPDIR', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
                'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'):
        directory = root / key
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', QT_SCALE_FACTOR=args.scale)
    os.environ.update(env)
    fixture = FamilyUi(SimpleNamespace(), root / 'fixture', 'crosshair')
    cfg = fixture.game / 'game/citadel/cfg'
    cfg.mkdir()
    settings = cfg / 'citadel_hero_settings.lst'
    if args.config_copy:
        source = args.config_copy.resolve()
        assert source.is_file(), 'provide an already copied configuration file'
        shutil.copyfile(source, settings)
    else:
        defaults = dict(zip(FIELDS, [False, False, False, 9.8, 16, -20, 1, 1, 0, 0.37, 1, 1, 0, 0, 1, 255, 255, 255, 0, 0, 0]))
        base = '\n'.join(f'{FIELDS[key]} = "{json.dumps(value)}"' for key, value in defaults.items())
        settings.write_text('<!-- kv3 encoding:text:version{e21c7f3c-8a33-41c5-9977-a76d3a32aa0d} format:generic:version{7412167c-06e9-4698-aff2-e63eb59037e7} -->\n'
            + '{ base = {' + base + '} heroes = { hero_inferno = { crosshair_dot_size = "5" crosshair_dot_outline_border = "2" } hero_review_unknown = { crosshair_dot_size = "3" } hero_doorman = { crosshair_dot_size = "7" crosshair_dot_outline_border = "2" crosshair_themed = "true" } } }')
    before = settings.read_bytes()
    expected = root / 'expected.json'
    expected.write_text(json.dumps(expected_values(before.decode())))
    env['CROSSHAIR_EXPECTED'] = str(expected)
    if args.captures:
        args.captures.mkdir(parents=True, exist_ok=True)
        env['CROSSHAIR_CAPTURES'] = str(args.captures.resolve())
    helper = compat.Helper(args.helper.resolve(), fixture.data)
    preview = helper.ok('native.importPreview', statePath=str(fixture.source), gamePath=str(fixture.game))
    helper.ok('native.importCommit', token=preview['token'], operationId='import', expectedRevision='0')
    helper.close()
    command = [str(args.binary.resolve()), '--helper', str(args.helper.resolve()), '--data-dir', str(fixture.data), '--check-ui', 'crosshair-review-' + args.regression if args.regression else 'crosshair-values']
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=40)
    print(result.stdout, end='')
    if result.returncode: raise SystemExit(result.stderr)
    assert ('PASS UI crosshair regression ' + args.regression if args.regression else 'PASS UI crosshair values') in result.stdout
    assert settings.read_bytes() == before, 'capture must not edit the copied settings'
    # Controlled regressions need the synthetic fixture; real-copy captures
    # continue to verify the copied values without adding fixture-only scopes.
    if not args.regression and not args.config_copy:
        for case, scale in [('picker','1'), ('legacy-import','1'), ('legacy-export','1'),
                            ('dpi','1'), ('dpi','1.5'), ('dpi','2'), ('gap','1'), ('hex-colour','1')]:
            regression = subprocess.run(command[:-1] + ['crosshair-review-' + case],
                env=env | {'QT_SCALE_FACTOR': scale}, capture_output=True, text=True, timeout=40)
            print(regression.stdout, end='')
            if regression.returncode: raise SystemExit(regression.stderr)
            assert 'PASS UI crosshair regression ' + case in regression.stdout
    report = dict(sourceSha256=hashlib.sha256(before).hexdigest(), values=json.loads(expected.read_text()), output=result.stdout)
    if args.captures:
        (args.captures / 'verified-values.json').write_text(json.dumps(report, indent=2))
