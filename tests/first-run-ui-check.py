"""Exercise first run in a clean environment, with a disposable review game."""
import importlib.util, json, pathlib, shutil, subprocess, sys, tempfile
from nativeintegrationcheck import BEFORE, vpk
spec=importlib.util.spec_from_file_location('review_fixture',pathlib.Path(__file__).resolve().parents[1]/'tools/review-fixture.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
app=pathlib.Path(sys.argv[1]).resolve(); helper=app.with_name('modlock-helper')
with tempfile.TemporaryDirectory(prefix='item111-') as t:
 root=pathlib.Path(t)
 env={'PATH':'/usr/bin:/bin','QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software',
      'MODLOCK_UI_TEST_ROOT':str(root),'PYTHONDONTWRITEBYTECODE':'1'}
 for key in ['HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_DATA_DIRS','XDG_CONFIG_DIRS']:
  p=root/key;p.mkdir(mode=0o700);env[key]=str(p)
 seed=root/'seed'
 request=dict(jsonrpc='2.0',id=1,method='native.library.initialize',params=dict(version=1,dataDir=str(seed),expectedRevision='0',operationId='a'*32))
 result=subprocess.run([str(helper)],input=json.dumps(request)+'\n',text=True,capture_output=True,env=env,timeout=30)
 assert 'error' not in json.loads(result.stdout),result.stdout
 (seed/'payloads').mkdir(exist_ok=True)
 game=root/'source-game';citadel=game/'game/citadel';citadel.mkdir(parents=True)
 (citadel/'gameinfo.gi').write_bytes(BEFORE.replace(b'citadel/addons',b'citadel'))
 (citadel/'pak01_dir.vpk').write_bytes(vpk());(game/'game/bin/win64').mkdir(parents=True)
 review=root/'review';review.mkdir()
 f=fixture.Fixture(review,seed,game);f.build()
 (f.citadel/'pak01_dir.vpk').write_bytes(vpk());(f.fake/'game/bin/win64').mkdir(parents=True)
 shutil.rmtree(f.citadel/'deadlock_native')
 if len(sys.argv)>2: env['UI_NATIVE_CAPTURES']=str(pathlib.Path(sys.argv[2]).resolve())
 env['ITEM111_GAME']=str(f.fake)
 mod=root/'test-mod.vpk';mod.write_bytes(vpk());env['ITEM111_MOD']=str(mod)
 for args in [['--check-ui','first-run'],['--check-ui','first-run-pick'],['--source',str(root/'missing-state.json'),'--check-ui','first-run-explicit']]:
  # A separate native root keeps the explicit-source case independent.
  run=subprocess.run([str(app),'--data-dir',str(root/('Season #2 50% Mägi? '+args[-1])),*args],env=env,text=True,capture_output=True,timeout=45)
  print(run.stdout+run.stderr, end='')
  assert run.returncode==0,run.stdout+run.stderr
  assert (f.citadel/'gameinfo.gi').read_bytes()==BEFORE.replace(b'citadel/addons',b'citadel')
  assert (f.citadel/'pak01_dir.vpk').read_bytes()==vpk()
