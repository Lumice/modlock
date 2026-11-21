#!/usr/bin/env python3
"""Poster presentation and real offscreen pointer checks, without a provider."""
import os
from pathlib import Path
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix='pc-') as scratch:
    for theme in ('dark', 'light'):
        for width in (780, 1000):
            root=Path(scratch)/(theme+str(width));root.mkdir(mode=0o700)
            env={k:v for k,v in os.environ.items() if not k.startswith('XDG_') and k not in ('DISPLAY','WAYLAND_DISPLAY','DBUS_SESSION_BUS_ADDRESS')}
            for n,key in enumerate(('HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS','TMPDIR')):
                p=root/str(n);p.mkdir(mode=0o700);env[key]=str(p)
            env['MODLOCK_UI_TEST_ROOT'] = str(scratch)
            env.update(QT_QPA_PLATFORM='offscreen',QT_QUICK_BACKEND='software',POSTER_THEME=theme,POSTER_WIDTH=str(width),DBUS_SESSION_BUS_ADDRESS='unix:path='+str(root/'no-bus'))
            capture=os.environ.get('POSTER_CAPTURE_DIR')
            if capture: env['POSTER_CAPTURE_DIR']=str(Path(capture)/(theme+str(width)))
            result=subprocess.run([sys.argv[1],'--demo','--check-ui','poster-controls'],env=env,capture_output=True,text=True,timeout=15)
            if result.returncode or 'PASS UI poster controls' not in result.stdout:
                sys.exit(f'{theme} {width}: {result.returncode}\n{result.stdout}{result.stderr}')
            print(theme,width,result.stdout.strip(),flush=True)
