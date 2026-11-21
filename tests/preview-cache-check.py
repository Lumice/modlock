import os, subprocess, sys, tempfile
from pathlib import Path
with tempfile.TemporaryDirectory(prefix='preview-cache-') as tmp:
    root=Path(tmp);env=os.environ.copy()
    for key in set(k for k in env if k.startswith('XDG_')) | {'HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS'}:
        path=root/key;path.mkdir(mode=0o700);env[key]=str(path)
    env.update(QT_QPA_PLATFORM='offscreen',QT_QUICK_BACKEND='software',EXPECT_STATE_ROOT=str(Path(env['XDG_CONFIG_HOME'])/'dl-mod-manager/state'))
    for name in ('library-a','library-b'):
        library=root/name
        result=subprocess.run([sys.argv[1],'--data-dir',str(library),'--check-ui','runtime-paths'],env=env,capture_output=True,text=True,timeout=25)
        assert result.returncode==0,result.stdout+result.stderr
        assert (library/'cache/previews').is_dir(),name+' missing per-library preview cache'
    assert not (Path(env['XDG_CACHE_HOME'])/'dl-mod-manager/previews').exists(),'explicit libraries shared default cache'
    result=subprocess.run([sys.argv[1],'--check-ui','runtime-paths'],env=env,capture_output=True,text=True,timeout=25)
    assert result.returncode==0,result.stdout+result.stderr
    assert (Path(env['XDG_CACHE_HOME'])/'dl-mod-manager/previews').is_dir(),'default neutral preview cache missing'
    assert not any('lumi' in p.relative_to(root).as_posix() for p in root.rglob('*')),'fresh app created lumi path'
    print('PASS separate explicit preview caches and neutral default cache, no fresh lumi paths')
