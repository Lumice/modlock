#!/usr/bin/env python3
"""Exercise the real QML singleton in isolated config/data/cache homes."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

def isolated_env(root):
    env = os.environ.copy(); env.pop("QML_DISK_CACHE_PATH", None)
    for key in set(k for k in env if k.startswith('XDG_')) | {
            'HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME',
            'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS'}:
        folder = root / key.lower()
        folder.mkdir(mode=0o700, parents=True, exist_ok=True)
        env[key] = str(folder)
    env.pop('WAYLAND_DISPLAY', None); env.pop('DISPLAY', None)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software')
    return env

with tempfile.TemporaryDirectory(prefix='pref-') as tmp:
    root = Path(tmp)
    env = isolated_env(root) | {'QT_QPA_PLATFORM': 'offscreen', 'QT_QUICK_BACKEND': 'software',
                        'XDG_CONFIG_HOME': str(root / 'config'), 'XDG_DATA_HOME': str(root / 'data'),
                        'XDG_CACHE_HOME': str(root / 'cache')}
    old = root / 'config/lumi/Deadlock Native.conf'
    new = root / 'config/modlock/settings.conf'

    def run(expected, write, expect_new=True, session=False, extra=None, cwd=None):
        legacy = '2' if session or not expect_new else '0'
        result = subprocess.run([sys.argv[1], '--demo', '--check-ui', 'preferences'],
                                env=env | {'PREFERENCES_EXPECT_MATURE': str(expected),
                                           'PREFERENCES_WRITE_MATURE': str(write),
                                           'PREFERENCES_EXPECT_LEGACY': legacy} | (extra or {}),
                                cwd=cwd, capture_output=True, text=True, timeout=25)
        assert result.returncode == 0, result.stdout + result.stderr
        if expect_new and not session:
            assert new.exists(), 'new preferences file missing'

    run(0, 1)  # Fresh install writes the explicit path.
    assert not old.exists(), 'fresh install created legacy preferences'
    run(1, 0)  # Read back persisted choice across processes.
    new.unlink()
    old.parent.mkdir(parents=True, exist_ok=True)
    original = b'[display]\nshowMaturePreviews=true\n\n[future]\nunknown=value\n'
    old.write_bytes(original)
    run(1, 0)  # Migrate NSFW choice and unrecognised keys.
    assert 'unknown=value' in new.read_text(), 'migration discarded other values'
    assert old.read_bytes() == original, 'migration modified the backup'
    run(0, 1)  # Existing new file wins over the old one.
    assert old.read_bytes() == original
    assert not (root / 'config/Modlock').exists(), 'implicit organisation path used'
    qt_cache = root / 'cache/dl-mod-manager/Deadlock Native'
    assert not qt_cache.is_symlink()
    assert not (root / 'cache/lumi').exists()
    print('PASS fresh preferences, restart, migration, backup, new-file precedence and data root')

    qt_cache.mkdir(parents=True, exist_ok=True)
    marker=qt_cache / 'foreign'
    marker.write_text('keep')
    run(1, 0)
    assert marker.read_text() == 'keep'
    print('PASS conflicting cache directory preserves startup and contents')

    # Immediate predecessor settings are copied verbatim; the source is never changed.
    new.unlink()
    previous=root / 'config/dl-mod-manager/settings.conf'
    previous.parent.mkdir(parents=True)
    previous.write_bytes(original)
    run(1, 0)
    assert previous.read_bytes()==original
    assert 'unknown=value' in new.read_text()
    previous.unlink()

    # Root discovery must stop before broker, library or preferences writes
    # when it cannot search the config/state tree for a pending marker.
    new.unlink()
    def denied_startup():
        message = (f"Modlock can't read its settings folder ({previous.parent}). "
                   "Fix the permissions of this folder and everything inside it, then open Modlock again.")
        before_data = sorted(str(p.relative_to(root / 'data')) for p in (root / 'data').rglob('*'))
        for args in ([], ['--demo', '--check-ui', 'preferences'],
                     ['--screenshot', str(root / 'denied.png')],
                     ['deadlock-mod-manager:https://gamebanana.com/mmdl/123,Mod,123']):
            result = subprocess.run([sys.argv[1]] + args, env=env,
                                    capture_output=True, text=True, timeout=5)
            assert result.returncode == 1, result.stdout + result.stderr
            assert message in result.stderr, result.stderr
        assert sorted(str(p.relative_to(root / 'data')) for p in (root / 'data').rglob('*')) == before_data
        assert not (root / 'denied.png').exists()
        assert not (Path(env['XDG_RUNTIME_DIR']) / 'deadlock-native').exists()

    for existing_new in (False, True):
      for permissions in (0, 0o111):
        saved_new=b'[display]\nshowMaturePreviews=true\nkeep=unchanged\n'
        if existing_new:new.write_bytes(saved_new)
        previous.parent.chmod(permissions)
        try:
            if permissions == 0:
                denied_startup()
            else:
                # Search permission suffices to stat known marker paths. With
                # no state directory, ENOENT proves there is no pending marker.
                run(0,1,expect_new=False,session=True)
            if existing_new:assert new.read_bytes()==saved_new
            else:assert not new.exists()
            assert old.read_bytes()==original
        finally:
            previous.parent.chmod(0o700)
        if existing_new:new.unlink()
    # Searchable predecessor, but a hidden pending directory: cannot rule out
    # either root's marker, so this also fails closed.
    pending = previous.parent / 'state/pending'
    pending.mkdir(parents=True)
    pending.chmod(0)
    previous.parent.chmod(0o111)
    try:
        denied_startup()
        assert not new.exists()
        assert not (root / 'data/modlock').exists()
    finally:
        previous.parent.chmod(0o700)
        pending.chmod(0o700)
    pending.rmdir(); pending.parent.rmdir()
    run(1,0)
    print('PASS config permission failure closes startup; searchable 0111 and hidden pending cases')

    # Existing data stays in place; an empty new directory does not hide the old library.
    # Two library databases are refused (covered by modlock-paths-check.py).
    legacy_data=root / 'data/deadlock-native'
    legacy_data.mkdir(parents=True)
    marker=legacy_data / 'sentinel'
    marker.write_bytes(b'absolute paths must stay')
    (legacy_data/'library.sqlite3').write_bytes(b'library fixture')
    run(0, 1, extra={'PREFERENCES_EXPECT_DATA_ROOT':'deadlock-native'})
    assert marker.read_bytes()==b'absolute paths must stay'
    (root / 'data/modlock').mkdir()
    run(1, 0, extra={'PREFERENCES_EXPECT_DATA_ROOT':'deadlock-native'})
    assert marker.read_bytes()==b'absolute paths must stay'
    (legacy_data/'library.sqlite3').unlink()

    # Migration must never stop startup: a file occupying the new folder's name
    # keeps the app on the existing preferences file instead.
    import shutil
    shutil.rmtree(root / 'config/modlock')
    blocker = root / 'config/modlock'
    blocker.write_text('not a folder')
    old.write_bytes(original)
    run(1, 0, expect_new=False)  # cache conflict left by the previous case
    assert blocker.read_text() == 'not a folder', 'migration touched the blocking file'
    assert old.read_bytes()==original, 'session fallback wrote legacy preferences'
    print('PASS blocked migration falls back to existing preferences and starts')

    previous.write_bytes(original)
    run(1, 0, expect_new=False)
    assert previous.read_bytes()==original, 'session fallback wrote dl-mod-manager/settings.conf'
    run(1, 0, expect_new=False)  # Session changes are discarded at restart.
    assert previous.read_bytes()==original
    previous.unlink()

    # A dangling symlink at the new location must not be followed: fall back
    # to the existing preferences and create nothing outside the config folder.
    blocker.unlink()
    outside = root / 'outside-target.conf'
    blocker.symlink_to(root / 'outside-dir')  # dangling folder link
    (root / 'outside-dir').mkdir(exist_ok=True)
    shutil.rmtree(root / 'outside-dir')
    old.write_bytes(original)  # the previous case wrote false
    run(1, 0, expect_new=False)
    assert not (root / 'outside-dir').exists(), 'followed a dangling folder link'
    blocker.unlink()
    (root / 'config/modlock').mkdir()
    new.symlink_to(outside)  # dangling file link at the settings path
    old.write_bytes(original)
    run(1, 0, expect_new=False)
    assert not outside.exists(), 'followed a dangling settings link'
    assert new.is_symlink(), 'removed the foreign link'
    print('PASS dangling links at the new location are never followed')

    # A FIFO at the old location must not block startup.
    new.unlink()
    old.unlink()
    os.mkfifo(old)
    run(0, 1, session=True)
    assert not new.exists(), 'started fresh while old preferences could not be read'
    print('PASS a non-regular old preferences path does not block startup')

    # The new folder is a link to a real folder elsewhere: never write through it.
    os.unlink(old) if old.is_fifo() else None
    shutil.rmtree(root / 'config/modlock')
    elsewhere = root / 'elsewhere'
    elsewhere.mkdir()
    (elsewhere / 'settings.conf').write_text('[display]\nshowMaturePreviews=false\n')
    (root / 'config/modlock').symlink_to(elsewhere)
    old.write_bytes(original)
    run(1, 0, expect_new=False)
    assert (elsewhere / 'settings.conf').read_text() == '[display]\nshowMaturePreviews=false\n', 'wrote through a folder link'
    assert sorted(p.name for p in elsewhere.iterdir()) == ['settings.conf'], 'created a file through a folder link'
    print('PASS a linked new folder is never written through')

    # New location unusable and the old file is a link to a file elsewhere: session only.
    old.unlink()
    outside_old = root / 'outside-old.conf'
    outside_old.write_text('[display]\nshowMaturePreviews=true\n')
    old.symlink_to(outside_old)
    run(0, 1, session=True)
    assert outside_old.read_text() == '[display]\nshowMaturePreviews=true\n', 'wrote through the old file link'
    print('PASS a linked old file is never written through')

    # FIFOs at both locations: no hang, session only.
    old.unlink()
    (root / 'config/modlock').unlink()
    (root / 'config/modlock').mkdir()
    os.mkfifo(old)
    os.mkfifo(new)
    run(0, 1, session=True)
    print('PASS FIFOs at both locations do not block')

    # A FIFO at Qt's lock file next to the settings file must not hang changes.
    old.unlink(); new.unlink()
    new.write_text('[display]\nshowMaturePreviews=true\n')
    lock = Path(str(new) + '.lock')
    os.mkfifo(lock)
    run(0, 1, session=True)
    assert new.read_text() == '[display]\nshowMaturePreviews=true\n', 'used a settings file with a FIFO lock'
    lock.unlink()
    import socket, time
    lock.write_bytes(b'')  # empty leftover lock: Qt would wait ~30 s per save
    started = time.monotonic()
    run(0, 1, session=True)
    assert time.monotonic() - started < 20, 'waited on an unreadable lock'
    lock.unlink()
    lock.write_text(f'999999999\nother\n{socket.gethostname()}\n')  # dead owner: Qt clears it
    run(1, 0)
    assert 'showMaturePreviews=false' in new.read_text(), 'dead-owner lock blocked the save'
    lock.unlink(missing_ok=True)
    new.unlink()
    os.mkfifo(lock)  # no settings file yet, FIFO lock
    run(0, 1, session=True)
    assert not new.exists(), 'created settings next to a FIFO lock'
    lock.unlink()
    print('PASS FIFO, unreadable and dead-owner lock files')

    # No private temporary folder and no safe location: never write a relative file.
    os.mkfifo(old)
    os.mkfifo(new)
    work = root / 'cwd'
    work.mkdir()
    run(0, 1, session=True, extra={'TMPDIR': str(root / 'missing-tmp')}, cwd=work)
    assert list(work.iterdir()) == [], 'wrote preferences into the working folder'
    print('PASS a missing temporary folder keeps preferences in memory only')

    # The config home itself is a link (dotfile managers), given with a trailing slash:
    # follow it once and use the real folder behind it.
    old.unlink(); new.unlink()
    real_home = root / 'real-config'
    shutil.move(str(root / 'config'), real_home)
    (root / 'config').symlink_to(real_home)
    env['XDG_CONFIG_HOME'] = str(root / 'config') + '/'
    run(0, 1)
    assert (real_home / 'modlock/settings.conf').exists(), 'linked config home not used'
    print('PASS a linked config home is followed once')

    # Old file present but its lock is unreadable: never start a fresh file over it.
    shutil.rmtree(real_home / 'modlock')
    old.write_bytes(original)
    Path(str(old) + '.lock').write_bytes(b'')
    run(0, 1, session=True)
    assert not (real_home / 'modlock/settings.conf').exists(), 'hid old preferences behind a fresh file'
    assert old.read_bytes() == original
    print('PASS an unreadable old lock keeps the old preferences reachable')

    # Old folder exists but can't be searched: its contents are unknown, so never start fresh.
    Path(str(old) + '.lock').unlink()
    os.chmod(old.parent, 0o600)
    override = os.access(old.parent, os.X_OK)  # root/CAP_DAC_OVERRIDE can still search it
    try:
        if not override:
            run(0, 1, session=True)
    finally:
        os.chmod(old.parent, 0o700)
    if override:
        print('SKIP unsearchable old folder: running with permission override')
    else:
        assert not (real_home / 'modlock/settings.conf').exists(), 'hid an unsearchable old folder behind a fresh file'
        assert old.read_bytes() == original
        print('PASS an unsearchable old folder keeps the old preferences reachable')
