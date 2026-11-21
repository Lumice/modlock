#!/usr/bin/env python3
"""Regression for a restarted helper with recent and stale Steam handoffs."""
import json
import hashlib
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile

from nativeintegrationcheck import BEFORE, vpk

binary, helper = map(lambda p: str(Path(p).resolve()), sys.argv[1:3])
with tempfile.TemporaryDirectory(prefix='native-play-poll-') as temp:
    root = Path(temp)
    game, data, source = root / 'game', root / 'native', root / 'source/state.json'
    addons = game / 'game/citadel/addons'
    source.parent.mkdir(parents=True)
    addons.mkdir(parents=True)
    (game / 'game/bin/win64').mkdir(parents=True)
    (game / 'game/citadel/pak01_dir.vpk').write_bytes(vpk())
    resource=game/'game/core/modlock_assets/pak01_dir.vpk'
    resource.parent.mkdir(parents=True);resource.write_bytes(vpk())
    (addons / 'pak01_dir.vpk').write_bytes(vpk())
    (addons / '.dmm.json').write_text(json.dumps({'version': 3, 'mods': {'remote-a': {'enabled': True, 'order': 0, 'shard': 1, 'currentVpks': ['pak01_dir.vpk'], 'disabledVpks': [], 'originalVpkNames': ['a_dir.vpk']}}}))
    mod = {'id': 'local-a', 'remoteId': 'remote-a', 'name': 'Tiny fixture', 'status': 'installed', 'installOrder': 0}
    source.write_text(json.dumps({'local-config': {'version': 33, 'state': {'activeProfileId': 'default', 'localMods': [mod], 'profiles': {'default': {'name': 'Default', 'mods': [mod]}}}}}))
    (game / 'game/citadel/gameinfo.gi').write_bytes(BEFORE)
    env = os.environ.copy()
    for key in list(env):
        if key.startswith('XDG_'): del env[key]
    for key in ('HOME', 'XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR', 'XDG_CONFIG_DIRS', 'XDG_DATA_DIRS', 'TMPDIR'):
        directory = root / key
        directory.mkdir(mode=0o700)
        env[key] = str(directory)
    seed_helper=os.environ.get("MODLOCK_PRE_RENAME_HELPER",helper)
    assert Path(seed_helper).is_file(),seed_helper
    process = subprocess.Popen([seed_helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    def rpc(method, **params):
        process.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {'dataDir': str(data), **params}}) + '\n')
        process.stdin.flush()
        result = json.loads(process.stdout.readline())
        assert 'result' in result, result
        return result['result']
    review = rpc('native.importPreview', statePath=str(source), gamePath=str(game))
    rpc('native.importCommit', token=review['token'], operationId='import', expectedRevision='0')
    review = rpc('native.applyPreview', profileId='default', gamePath=str(game), deploymentLayout='shards-v1')
    rpc('native.apply', token=review['token'], operationId='initial', expectedRevision='1', deploymentLayout='shards-v1')
    process.stdin.close()
    assert process.wait(timeout=20) == 0
    fake = root / 'bin'; fake.mkdir()
    steam = fake / 'steam'
    steam.write_text('#!/usr/bin/python3\nimport json,os,subprocess,sys,time\nif os.environ.get("UI_NATIVE_CAPTURES"): time.sleep(1)\nwith open(os.environ["STEAM_LOG"],"a") as f: f.write(json.dumps({"t":time.monotonic(),"args":sys.argv[1:]})+"\\n")\nif os.environ.get("STEAM_FAKE_GAME"):\n p=subprocess.Popen([os.environ["STEAM_FAKE_GAME"],"60"])\n open(os.environ["STEAM_FAKE_PID"],"w").write(str(p.pid))\n p.wait()\n')
    steam.chmod(0o700)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', UI_NATIVE_GAME=str(game), UI_NATIVE_SOURCE=str(source),
               PATH=str(fake)+os.pathsep+env['PATH'], STEAM_LOG=str(root/'steam.log'))
    if env.get('UI_NATIVE_CAPTURES'):
        # Hold only disposable test launch dispatch so each progress frame can render.
        wrapper = root / 'capture-helper'
        wrapper.write_text("""#!/usr/bin/python3
import json,subprocess,sys,threading,time
child=subprocess.Popen([sys.argv[0]+'.real'],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
def output():
    for line in child.stdout:
        sys.stdout.write(line);sys.stdout.flush()
reader=threading.Thread(target=output);reader.start()
for line in sys.stdin:
    if json.loads(line).get('method')=='native.launch.commit': time.sleep(1)
    child.stdin.write(line);child.stdin.flush()
child.stdin.close();child.wait();reader.join()
""")
        wrapper.chmod(0o700)
        (root / 'capture-helper.real').symlink_to(helper)
        helper = str(wrapper)
    def ui(scenario, timeout=45, selected_helper=None):
        result = subprocess.run([binary, '--helper', selected_helper or helper, '--data-dir', str(data), '--check-ui', scenario], env=env, capture_output=True, text=True, timeout=timeout)
        assert result.returncode == 0 and 'PASS UI' in result.stdout, f'{scenario}: {result.stdout}\n{result.stderr}'
        print(result.stdout.strip(), flush=True)
    ui('native-launch-seed', selected_helper=seed_helper)
    marker = game / 'game/citadel/.deadlock-native-launch.json'
    assert marker.is_file()
    import sqlite3, time
    # An old marker on this boot is not evidence of an old dispatch.
    old = time.time() - 600
    os.utime(marker, (old, old))
    def barriers(exit_game=None):
        probe = subprocess.Popen([helper], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
        def call(method, **params):
            probe.stdin.write(json.dumps({'jsonrpc': '2.0', 'id': 1, 'method': method, 'params': {'dataDir': str(data), **params}}) + '\n')
            probe.stdin.flush()
            response = json.loads(probe.stdout.readline())
            assert 'result' in response, response
            return response['result']
        def query(control):
            call('native.launch.barriers', controlId=control)
            deadline = time.monotonic() + 15
            while True:
                receipt = call('native.launch.control', controlId=control)
                if receipt['state'] != 'pending': break
                assert time.monotonic() < deadline
                time.sleep(.05)
            assert receipt['state'] == 'succeeded', receipt
            return receipt['value']['barriers']
        rows = query('age-check')
        if exit_game is not None:
            assert len(rows) == 1
            exit_game.terminate(); exit_game.wait(timeout=5)
            rows = query('after-game')
        probe.stdin.close()
        assert probe.wait(timeout=20) == 0
        return rows
    assert len(barriers()) == 1, 'old marker released a same-boot dispatch'
    # Simulate a crash after dispatch admission but before the post-spawn timestamp.
    boot = int(next(line.split()[1] for line in Path('/proc/stat').read_text().splitlines() if line.startswith('btime ')))
    os.utime(marker, (boot - 600, boot - 600))
    with sqlite3.connect(data / 'library.sqlite3') as db:
        journal_id, body = db.execute("SELECT id, body FROM journals WHERE json_extract(body, '$.kind')='launchHandoff'").fetchone()
        journal = json.loads(body)
        evidence=journal['preparedProof']['selectedSources']['sources']
        assert any(row['path']==str(resource) and row['identity']['sha256']=='' for row in evidence), evidence
        complete_evidence = journal['dispatchEvidence'].copy()
        journal['dispatchEvidence']['wallSeconds'] = None
        journal['dispatchEvidence']['bootMillis'] = None
        db.execute('UPDATE journals SET body=? WHERE id=?', (json.dumps(journal, sort_keys=True, separators=(',', ':')), journal_id))
    assert len(barriers()) == 1, 'a same-boot incomplete dispatch released its hold'
    pending_game = root / 'pending-game/deadlock.exe'
    pending_game.parent.mkdir()
    shutil.copyfile('/bin/sleep', pending_game); pending_game.chmod(0o700)
    external_game = subprocess.Popen([str(pending_game), '60'], env=env)
    try:
        assert len(barriers(external_game)) == 1, 'incomplete dispatch was released by an unrelated game cycle'
    finally:
        if external_game.poll() is None: external_game.kill(); external_game.wait()
    with sqlite3.connect(data / 'library.sqlite3') as db:
        journal['dispatchEvidence'] = complete_evidence
        db.execute('UPDATE journals SET body=? WHERE id=?', (json.dumps(journal, sort_keys=True, separators=(',', ':')), journal_id))
    ui('native-launch-recent')
    with sqlite3.connect(data / 'library.sqlite3') as db:
        before = json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])
    assert before['state'] == 'handoff'
    boot = int(next(line.split()[1] for line in Path('/proc/stat').read_text().splitlines() if line.startswith('btime ')))
    old = max(time.time() - 600, boot + 1)
    os.utime(marker, (old, old))
    # Legacy records have no dispatch evidence; marker age cannot prove a reboot.
    with sqlite3.connect(data / 'library.sqlite3') as db:
        journal_id, body = db.execute("SELECT id, body FROM journals WHERE json_extract(body, '$.kind')='launchHandoff'").fetchone()
        journal = json.loads(body)
        evidence = journal.pop('dispatchEvidence')
        assert evidence['bootId'] == Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        assert int(evidence['wallSeconds']) > 0 and int(evidence['bootMillis']) > 0
        db.execute('UPDATE journals SET body=? WHERE id=?', (json.dumps(journal, sort_keys=True, separators=(',', ':')), journal_id))
    assert len(barriers()) == 1, 'same-boot legacy marker released a handoff'
    # An unrelated game cycle is not dispatch evidence for a legacy record.
    fake_game = root / 'deadlock.exe'
    shutil.copyfile('/bin/sleep', fake_game); fake_game.chmod(0o700)
    external_game = subprocess.Popen([str(fake_game), '60'], env=env)
    try:
        assert len(barriers(external_game)) == 1, 'legacy hold was released by an unrelated game cycle'
    finally:
        if external_game.poll() is None: external_game.kill(); external_game.wait()
    old = boot - 600
    os.utime(marker, (old, old))
    assert len(barriers()) == 1, 'legacy hold released based on marker time'
    assert marker.exists()
    external_game = subprocess.Popen([str(fake_game), '60'], env=env)
    try:
        ui('native-launch-clear-legacy-fails')
    finally:
        external_game.terminate(); external_game.wait(timeout=5)
    # A reboot can renumber a Btrfs device while the same library inode and
    # the exact game marker remain in place. Model the user's legacy record.
    def canonical(value):
        return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()
    def digest(value):
        return hashlib.sha256(canonical(value)).hexdigest()
    with sqlite3.connect(data / 'library.sqlite3') as db:
        journal_id, body = db.execute("SELECT id, body FROM journals WHERE json_extract(body, '$.kind')='launchHandoff'").fetchone()
        journal = json.loads(body)
        journal['binding']['rootIdentity'].pop('fsid', None)
        journal['preparedProof']['directories']['root'].pop('fsid', None)
        # Only Btrfs/overlay legacy records admit a changed device number.
        # On tmpfs/ext4 the same mismatch must be refused, so keep the exact
        # device while still checking legacy (no-fsid) launch cleanup there.
        filesystem = subprocess.check_output(['/usr/bin/stat', '-f', '-c', '%T', str(data)], text=True).strip()
        renumbered = filesystem in ('btrfs', 'overlayfs')
        old_device = journal['binding']['rootIdentity']['device'] + int(renumbered)
        journal['binding']['rootIdentity']['device'] = old_device
        journal['preparedProof']['directories']['root']['device'] = old_device
        journal['proofSha256'] = digest(journal['preparedProof'])
        attempt = json.loads(db.execute('SELECT body FROM launch_attempts WHERE id=?', (journal['attemptId'],)).fetchone()[0])
        attempt['preparation']['proofSha256'] = journal['proofSha256']
        marker_body = {key: journal[key] for key in ('version', 'epoch', 'attemptId', 'binding', 'proofSha256')}
        marker_body.update(kind='launchHandoff', journalId=journal_id)
        marker_bytes = canonical(marker_body)
        marker.chmod(0o600)
        marker.write_bytes(marker_bytes)
        marker.chmod(0o400)
        journal['reservation']['bytesSha256'] = hashlib.sha256(marker_bytes).hexdigest()
        db.execute('UPDATE journals SET body=? WHERE id=?', (canonical(journal).decode(), journal_id))
        db.execute('UPDATE launch_attempts SET body=? WHERE id=?', (canonical(attempt).decode(), journal['attemptId']))
    ui('native-launch-clear-legacy')
    print(f'PASS legacy clear on {filesystem}; device-renumbering case {"covered" if renumbered else "not applicable"}', flush=True)
    with sqlite3.connect(data / 'library.sqlite3') as db:
        after = json.loads(db.execute('SELECT body FROM launch_attempts WHERE id=?',(before['attemptId'],)).fetchone()[0])
    assert after['state'] == 'terminal' and after['outcome']['reason'] == 'steamIntentCancelled', after
    assert not marker.exists()
    # A pending request cannot survive reboot, even when the marker is recent.
    ui('native-launch-seed')
    with sqlite3.connect(data / 'library.sqlite3') as db:
        journal_id, body = db.execute("SELECT id, body FROM journals WHERE json_extract(body, '$.kind')='launchHandoff'").fetchone()
        journal = json.loads(body)
        journal['dispatchEvidence']['bootId'] = '00000000-0000-0000-0000-000000000000'
        journal['dispatchEvidence']['wallSeconds'] = None
        journal['dispatchEvidence']['bootMillis'] = None
        db.execute('UPDATE journals SET body=? WHERE id=?', (json.dumps(journal, sort_keys=True, separators=(',', ':')), journal_id))
    assert not barriers(), 'a previous-boot dispatch should resolve'
    # Same-boot expiration uses boot time rather than marker or wall time.
    if time.clock_gettime(time.CLOCK_BOOTTIME) >= 180:
        ui('native-launch-seed')
        with sqlite3.connect(data / 'library.sqlite3') as db:
            journal_id, body = db.execute("SELECT id, body FROM journals WHERE json_extract(body, '$.kind')='launchHandoff'").fetchone()
            journal = json.loads(body)
            assert int(journal['dispatchEvidence']['bootMillis']) >= 180_000
            journal['dispatchEvidence']['bootMillis'] = str(int(journal['dispatchEvidence']['bootMillis']) - 180_000)
            journal['dispatchEvidence']['wallSeconds'] = str(int(time.time()) + 86400)
            db.execute('UPDATE journals SET body=? WHERE id=?', (json.dumps(journal, sort_keys=True, separators=(',', ':')), journal_id))
        ui('native-launch-expired')
        assert not barriers(), 'elapsed boot time should resolve despite a future wall time'
        with sqlite3.connect(data / 'library.sqlite3') as db:
            assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0
            outcome = json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])['outcome']
            assert outcome['reason'] == 'startTimedOut', outcome
        print('PASS restart: current boot, dispatchIntent >180s, no game: automatic startTimedOut, journal removed; no Clear it or Recover', flush=True)
    ui('native-launch-seed')
    os.utime(marker, (old, old))
    fake_game = root / 'deadlock.exe'
    shutil.copyfile('/bin/sleep', fake_game)
    fake_game.chmod(0o700)
    signal = root / 'game-seen'
    env['UI_LAUNCH_GAME_SEEN'] = str(signal)
    game_process = subprocess.Popen([str(fake_game), '60'], env=env)
    app = subprocess.Popen([binary, '--helper', helper, '--data-dir', str(data), '--check-ui', 'native-launch-game-cycle'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        deadline = time.monotonic() + 25
        while not signal.exists() and app.poll() is None and time.monotonic() < deadline:
            time.sleep(.05)
        assert signal.exists(), 'game observation and toggle did not complete'
        game_process.terminate()
        game_process.wait(timeout=5)
        out, err = app.communicate(timeout=30)
        assert app.returncode == 0 and 'PASS UI' in out, out + err
        print(out.strip(), flush=True)
    finally:
        if game_process.poll() is None: game_process.kill(); game_process.wait()
        if app.poll() is None: app.kill(); app.communicate()
    with sqlite3.connect(data / 'library.sqlite3') as db:
        after = json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])
    assert after['outcome']['status'] == 'observed', after
    for retire in (False, True):
        if retire: env['UI_LAUNCH_RETIRE'] = '1'
        else: env.pop('UI_LAUNCH_RETIRE', None)
        import signal as signal_module
        signal.unlink()
        pid_file = root / 'game-pid'
        env.update(STEAM_FAKE_GAME=str(fake_game), STEAM_FAKE_PID=str(pid_file))
        app = subprocess.Popen([binary, '--helper', helper, '--data-dir', str(data), '--check-ui', 'native-launch-live-cycle'], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 25
            while not signal.exists() and app.poll() is None and time.monotonic() < deadline:
                time.sleep(.05)
            if not signal.exists() or not pid_file.exists():
                if app.poll() is None: app.kill()
                out, err = app.communicate()
                raise AssertionError('live game observation and toggle did not complete\n' + out + err)
            os.kill(int(pid_file.read_text()), signal_module.SIGTERM)
            out, err = app.communicate(timeout=30)
            assert app.returncode == 0 and 'PASS UI' in out, out + err
            print(out.strip(), flush=True)
        finally:
            if pid_file.exists():
                try: os.kill(int(pid_file.read_text()), signal_module.SIGTERM)
                except ProcessLookupError: pass
            if app.poll() is None: app.kill(); app.communicate()
        with sqlite3.connect(data / 'library.sqlite3') as db:
            after = json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])
        assert after['outcome']['status'] == 'observed' and after['outcome']['reason'] == 'gameCycleCompleted', after
        with sqlite3.connect(data / 'library.sqlite3') as db:
            assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0
        print('PASS live cycle: gameCycleCompleted, journal cleared, editing unlocked; retired=' + str(retire), flush=True)

    for key in ('STEAM_FAKE_GAME', 'STEAM_FAKE_PID', 'UI_LAUNCH_RETIRE'):
        env.pop(key, None)
    env['UI_NATIVE_CONFIRM_REVIEW'] = '1'
    ui('native-launch-seed')
    ui('native-launch-recent-clear')
    with sqlite3.connect(data / 'library.sqlite3') as db:
        assert db.execute('SELECT count(*) FROM journals').fetchone()[0] == 0
        after = json.loads(db.execute('SELECT body FROM launch_attempts ORDER BY rowid DESC LIMIT 1').fetchone()[0])
        assert after['outcome']['reason'] == 'steamIntentCancelled', after
    print('PASS current-format crash handoff: one Clear it, journal removed, library unlocked', flush=True)
