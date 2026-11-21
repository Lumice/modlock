"""Production LibraryModel/LaunchController readers and writes, each in a fresh process."""
import hashlib, json, os, shutil, subprocess, sys, tempfile
from pathlib import Path
settings = "Modlock can't use its settings folder (~/.config/dl-mod-manager). Check that it is a normal folder you own."
failures=[]
for case in ('fresh','fresh-umask-0002','legacy-ignored','linked-app','linked-state','linked-pending','linked-health','unsearchable-state','unsearchable-health','fixed-link'):
    with tempfile.TemporaryDirectory(prefix='state-safety-') as tmp:
        root=Path(tmp); env=os.environ.copy()
        for key in set(k for k in env if k.startswith('XDG_')) | {'HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS'}:
            path=root/key;path.mkdir(mode=0o700);env[key]=str(path)
        library=root/'native'; key=hashlib.sha256(str(library).encode()).hexdigest()
        config=Path(env['XDG_CONFIG_HOME']); old=config/'lumi/Deadlock Native/pending'; neutral=config/'dl-mod-manager/state'
        error=''; locked=None
        if case=='legacy-ignored':
            old.mkdir(parents=True)
            for suffix in ('.json','.launch.json','.launch-settings.json'):
                (old/(key+suffix)).write_bytes(b'{"unfinished":true}\n')
            history=old.parent/'launch-health';history.mkdir()
            (history/(key+'.json')).write_text(json.dumps({'root':str(library),'enabled':False}))
        if case=='fixed-link':
            target=root/'target';target.mkdir();error=settings
            (config/'dl-mod-manager').symlink_to(target)
        if case.startswith('linked-'):
            target=root/'target';target.mkdir();error=settings
            linked={'linked-app':config/'dl-mod-manager','linked-state':neutral,'linked-pending':neutral/'pending','linked-health':neutral/'launch-health'}[case]
            linked.parent.mkdir(parents=True,exist_ok=True);linked.symlink_to(target)
        if case in ('unsearchable-state','unsearchable-health'):
            locked=neutral if case=='unsearchable-state' else neutral/'launch-health'
            locked.mkdir(parents=True);error=settings
        if locked: locked.chmod(0o700)
        before={p.relative_to(old.parent):p.read_bytes() for p in old.parent.rglob('*') if p.is_file()}
        if locked: locked.chmod(0o600)
        try:
            command=[sys.argv[1],'state-safety']
            if case=='legacy-ignored' and shutil.which('strace'):
                command=['strace','-f','-qq','-e','trace=%file','-o',str(root/'file-access.log')]+command
            result=subprocess.run(command,env=env|{'SAFETY_LIBRARY':str(library),'SAFETY_ERROR':error,'SAFETY_FIX_LINK':'1' if case=='fixed-link' else ''},capture_output=True,text=True,timeout=25,
                                  preexec_fn=(lambda: os.umask(0o002)) if case=='fresh-umask-0002' else None)
        finally:
            if locked: locked.chmod(0o700)
        if result.returncode:
            failures.append(case+': '+result.stdout+result.stderr)
            print('FAIL',failures[-1],flush=True)
            continue
        if case=='legacy-ignored' and (root/'file-access.log').exists():
            assert str(config/'lumi') not in (root/'file-access.log').read_text(), 'legacy state was inspected'
        assert before=={p.relative_to(old.parent):p.read_bytes() for p in old.parent.rglob('*') if p.is_file()},case+' changed legacy bytes'
        if error: assert not [p for p in neutral.rglob('*') if p.is_file()],case+' wrote neutral state while blocked'
        if case.startswith('linked-') or case=='fixed-link': assert not list(target.iterdir()),case+' wrote through link'
        if case=='fresh-umask-0002': assert not [p for p in neutral.rglob('*') if p.is_dir() and p.stat().st_mode & 0o022], 'state folder is group/other writable'
        if case=='fresh': assert not any('lumi' in p.relative_to(root).as_posix() for p in root.rglob('*'))
        if case=='fixed-link':
            restart=subprocess.run([sys.argv[1],'state-safety'],env=env|{'SAFETY_LIBRARY':str(library),'SAFETY_ERROR':''},capture_output=True,text=True,timeout=25)
            assert restart.returncode==0, restart.stdout+restart.stderr
        print('PASS',case,flush=True)

assert not failures, '\n'.join(failures)
