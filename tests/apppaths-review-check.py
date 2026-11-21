#!/usr/bin/env python3
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

failures=[]
with tempfile.TemporaryDirectory(prefix='apppaths-review-') as tmp:
    for case in ['preferences','root','state','blocked-0','blocked-256','blocked-73','existing-blocked-0','existing-blocked-256','existing-blocked-73','old-helper','sleepy-helper']:
        if len(sys.argv)>2 and sys.argv[2] not in case:continue
        root=Path(tmp)/case;root.mkdir();env=os.environ.copy()
        for key in set(k for k in env if k.startswith('XDG_')) | {'HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS'}:
            path=root/key;path.mkdir(mode=0o700);env[key]=str(path)
        config=Path(env['XDG_CONFIG_HOME']);data=Path(env['XDG_DATA_HOME'])
        old=config/'dl-mod-manager/settings.conf';old.parent.mkdir();original=b'[display]\nshowMaturePreviews=true\n'
        old.write_bytes(original)
        blocked=None
        if 'blocked-' in case:
            older=config/'lumi/Deadlock Native.conf';older.parent.mkdir();older.write_bytes(original)
            blocked=old.parent;blocked.chmod(int(case.split('-')[-1]));args=['blocked-predecessor']
            if case.startswith('existing-'):
                new=config/'modlock/settings.conf';new.parent.mkdir();new.write_bytes(b'[display]\nshowMaturePreviews=true\nkeep=unchanged\n');args=['blocked-existing']
        elif case.endswith('helper'):
            helper=root/'selected-helper'
            helper.write_text('#!/usr/bin/python3\nimport sys,time\nfrom pathlib import Path\nsys.stdin.read()\nPath('+repr(str(root/'stdin-closed'))+').write_text("closed")\n'+('time.sleep(30)\n' if case=='sleepy-helper' else ''))
            helper.chmod(0o700);args=['helper',str(helper)]
        elif case=='preferences':
            (config/'modlock').write_bytes(b'blocked')
            args=['preferences']
        else:
            legacy=data/'deadlock-native';legacy.mkdir();(legacy/'library.sqlite3').write_bytes(b'library marker')
            if case=='root':
                (data/'modlock').mkdir()
                args=['root',str(legacy)]
            else:
                (config/'modlock/state').mkdir(parents=True)
                (config/'dl-mod-manager/state').mkdir()
                args=['state',str(config/'dl-mod-manager/state')]
        start=time.monotonic()
        run=subprocess.run([sys.argv[1],*args],env=env,text=True,capture_output=True,timeout=15)
        if blocked:
            blocked.chmod(0o700)
            assert older.read_bytes()==original
        if case.endswith('helper'):assert time.monotonic()-start<8, 'helper discovery hung'
        if case.endswith('helper'):assert (root/'stdin-closed').exists(), 'stdin was not closed'
        print(case,run.returncode,run.stdout,run.stderr,flush=True)
        if run.returncode:failures.append(case)
        if old.read_bytes()!=original:failures.append(case+" changed legacy bytes")

assert not failures, failures
