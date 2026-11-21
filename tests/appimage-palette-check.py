#!/usr/bin/env python3
"""Read-only AppImage palette fallback, exercised through real SystemPalette."""
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix='dl-palette-') as tmp:
    root = Path(tmp)
    env = os.environ.copy()
    for key in set(k for k in env if k.startswith('XDG_')) | {
            'HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
            'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'}:
        folder = root / key.lower()
        folder.mkdir(mode=0o700)
        env[key] = str(folder)
    for key in ('APPIMAGE', 'QML_DISK_CACHE_PATH', 'QT_PLUGIN_PATH', 'QT_QPA_PLATFORM_PLUGIN_PATH'):
        env.pop(key, None)
    # The fallback stands in for the KDE plugin, so declare a KDE session.
    env['XDG_CURRENT_DESKTOP'] = 'KDE'
    env.update(QT_QPA_PLATFORM='offscreen', QT_QPA_PLATFORMTHEME='no-such-theme',
               QT_QUICK_BACKEND='software')
    # The AppImage runtime sets APPIMAGE itself. --appimage-extract-and-run
    # also works on machines without FUSE, and cleans its extraction.
    binary = str(Path(sys.argv[1]).resolve())
    command = [binary] + (['--appimage-extract-and-run'] if binary.endswith('.AppImage') else [])
    last_roles = None
    def run(appimage=False, expected=None, checks=None):
        global last_roles
        current = env.copy()
        if appimage:
            current['APPIMAGE'] = binary
        if expected:
            current['EXPECT_PALETTE'] = expected
        if expected == '#171f2b,#dde6f1,#3d8dd3' and not (checks or {}).get('QT_QPA_PLATFORMTHEME'):
            current['CHECK_PALETTE_HOVER'] = '1'
        current.update(checks or {})
        result = subprocess.run(command + ['--demo', '--check-ui', 'palette'],
                                env=current, capture_output=True, text=True, timeout=20)
        assert result.returncode == 0, result.stdout + result.stderr
        match = re.search(r'PALETTE (#[0-9a-f,#]+)', result.stderr)
        assert match, result.stderr
        if current.get('CHECK_PALETTE_HOVER') or current.get('CHECK_BUILTIN_CONTRAST'):
            print('\n'.join(line for line in result.stderr.splitlines() if not line.startswith('PALETTE_ROLES ')))
        last_roles = re.search(r'PALETTE_ROLES (.+)', result.stderr)[1]
        return match[1]
    # Qt's generic/offscreen theme itself supplies a palette. Explicitly load
    # a null-palette theme to exercise the required absence-of-palette branch.
    if len(sys.argv) < 3:
        raise SystemExit('usage: appimage-palette-check.py BINARY TEST_THEME_DIR [KDE_THEME_DIR]')
    env.update(QT_QPA_PLATFORMTHEME='palette-none',
               QT_QPA_PLATFORM_PLUGIN_PATH=str(Path(sys.argv[2]).resolve()))
    baseline = run()
    fixture = Path(env['XDG_CONFIG_HOME']) / 'kdeglobals'
    scheme = '''[Colors:Window]
BackgroundNormal=23,31,43
ForegroundNormal=221,230,241
[Colors:Window][Inactive]
BackgroundNormal=24,32,44
ForegroundNormal=221,230,241
[ColorEffects:Inactive]
Enable=false
ChangeSelectionColor=false
[Colors:View]
BackgroundNormal=17,24,35
BackgroundAlternate=25,33,45
ForegroundNormal=211,220,231
ForegroundInactive=101,111,121
ForegroundLink=82,161,231
ForegroundVisited=151,111,211
[Colors:Button]
BackgroundNormal=34,43,56
ForegroundNormal=201,210,221
[Colors:Selection]
BackgroundNormal=61,141,211
ForegroundNormal=251,252,253
[Colors:Tooltip]
BackgroundNormal=12,19,29
ForegroundNormal=241,242,243
[Colors:Complementary]
BackgroundNormal=10,15,20
ForegroundNormal=240,245,250
'''
    fixture.write_text(scheme)
    if not binary.endswith('.AppImage'):
        assert run(expected=baseline) == baseline, 'non-AppImage palette overridden'
    run(True, '#171f2b,#dde6f1,#3d8dd3')
    assert fixture.read_text() == scheme, 'kdeglobals was modified'
    default_config = Path(env['HOME']) / '.config'
    default_config.mkdir()
    fixture.rename(default_config / 'kdeglobals')
    config_home = env.pop('XDG_CONFIG_HOME')
    run(True, '#171f2b,#dde6f1,#3d8dd3')
    env['XDG_CONFIG_HOME'] = config_home
    (default_config / 'kdeglobals').rename(fixture)
    fixture.write_text('[Colors:Window]\nBackgroundNormal=23,31,43\nForegroundNormal=999,wrong,-1\n')
    run(True, baseline)
    for contents in ('[Colors:Window]\nBackgroundNormal=bad\n', 'x' * (1024 * 1024 + 1)):
        fixture.write_text(contents)
        run(True, baseline)
    fixture.unlink()
    os.mkfifo(fixture)
    run(True, baseline)
    fixture.unlink()
    run(True, baseline)  # Missing file.
    env['CHECK_PORTAL_DARK'] = '1'
    run(True, '#31363b,#eff0f1,#1d6fa5', {'CHECK_BUILTIN_CONTRAST': '1'})
    fixture.write_text('[Colors:Window]\nBackgroundNormal=bad\nForegroundNormal=0,0,0\n')
    run(True, '#31363b,#eff0f1,#1d6fa5', {'CHECK_BUILTIN_CONTRAST': '1'})
    fixture.write_text(scheme.replace('ForegroundNormal=221,230,241', 'ForegroundNormal=bad'))
    run(True, '#31363b,#eff0f1,#1d6fa5', {'CHECK_BUILTIN_CONTRAST': '1'})
    env.pop('CHECK_PORTAL_DARK')
    print('PASS fixture SystemPalette, no-APPIMAGE, malformed, size cap, FIFO, read-only')

    # Real sessions: the requested theme (kde) isn't bundled, so Qt's generic
    # theme loads and supplies its stock palette. That must still count as
    # "no theme". Empty values ("Color=") are unset in KDE, not invalid.
    fixture.write_text(scheme.replace('[ColorEffects:Inactive]\n', '[ColorEffects:Inactive]\nColor=\nColorAmount=\n'))
    generic = {'QT_QPA_PLATFORMTHEME': 'no-such-theme', 'QT_QPA_PLATFORM_PLUGIN_PATH': ''}
    saved = {k: env.pop(k) for k in ('QT_QPA_PLATFORMTHEME', 'QT_QPA_PLATFORM_PLUGIN_PATH')}
    current_theme = env.copy()
    env.update(generic)
    env.pop('QT_QPA_PLATFORM_PLUGIN_PATH')
    run(True, '#171f2b,#dde6f1,#3d8dd3')
    env.clear(); env.update(current_theme); env.update(saved)
    print('PASS generic-theme session with empty KDE values uses kdeglobals')

    # An empty duplicate clears a required key: the scheme is incomplete.
    fixture.write_text(scheme.replace('BackgroundNormal=23,31,43\n', 'BackgroundNormal=23,31,43\nBackgroundNormal=\n', 1))
    run(True, baseline)
    # Not a KDE session: never use kdeglobals, even with Qt's stock palette.
    fixture.write_text(scheme)
    env['XDG_CURRENT_DESKTOP'] = 'GNOME'
    run(True, baseline)
    env['XDG_CURRENT_DESKTOP'] = 'KDE'
    print('PASS empty duplicate clears a key; kdeglobals only for KDE sessions')

    fixtures = Path(__file__).parent / 'palette-fixtures'
    fixture.write_text((fixtures / 'BreezeLight.colors').read_text())
    run(True, '#eff0f1,#232629,#3daee9', {'CHECK_PORTAL_DARK': '1'})
    print('PASS Breeze Light scheme produces light palette with dark hint')
    fixture.write_text((fixtures / 'BreezeDark.colors').read_text())
    run(True, '#202326,#fcfcfc,#3daee9', {'CHECK_KDE_SHADE': '1'})
    print('PASS Breeze Dark disabled Window matches KDE #1f2124')
    if len(sys.argv) > 2:
        fixture.write_text(scheme)
        theme = {'QT_QPA_PLATFORMTHEME': 'palette-check',
                 'QT_QPA_PLATFORM_PLUGIN_PATH': str(Path(sys.argv[2]).resolve())}
        before = run(checks=theme)
        assert before.split(',')[1] == '#bedcf0', 'test theme did not load'
        run(True, before, dict(theme, EXPECT_ROLE_COLORS=last_roles))
        print('PASS direct-path loaded test platform theme palette preserved')

    if len(sys.argv) > 3:
        theme = {'QT_QPA_PLATFORMTHEME': 'kde',
                 'QT_QPA_PLATFORM_PLUGIN_PATH': str(Path(sys.argv[3]).resolve())}
        before = run(checks=theme)
        assert before.split(',')[1] == '#dde6f1', 'real KDE theme did not load fixture'
        run(True, before, dict(theme, EXPECT_ROLE_COLORS=last_roles))
        print('PASS direct-path real KDE platform theme palette preserved')
