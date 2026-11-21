#!/usr/bin/env python3
"""Installer ownership, association scope, atomic publication and Exec round-trip."""
import os
from pathlib import Path
import re
import subprocess
import tempfile

repo=Path(__file__).resolve().parent.parent

def exec_arguments(value):
    # Decode the desktop string layer first, then Exec quoting, per the spec.
    escapes={'s':' ','n':'\n','t':'\t','r':'\r','\\':'\\'}
    def unescape(match):
        assert match[1] in escapes, ('invalid desktop string escape',match[0])
        return escapes[match[1]]
    value=re.sub(r'\\(.)',unescape,value)
    tokens=re.findall(r'"(?:\\["`$\\]|[^"\\])*"|[^\s"]+',value)
    assert ''.join(tokens).replace(' ','')==value.replace(' ','')
    args=[]
    for token in tokens:
        if token.startswith('"'):
            token=re.sub(r'\\(["`$\\])',lambda m:m[1],token[1:-1])
        args.append(token.replace('%%','%'))
    return args

with tempfile.TemporaryDirectory(prefix='modlock-packaging-') as tmp:
    for installer in ['tools/install-appimage.sh','packaging/install-desktop-entry.sh']:
      for empty_xdg in [False,True]:
        root=Path(tmp)/(installer.split('/')[0]+str(empty_xdg)+' space " ` $ \\ %');root.mkdir()
        env=os.environ.copy()
        for key in set(k for k in env if k.startswith('XDG_'))|{'HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','XDG_STATE_HOME','XDG_DATA_DIRS','XDG_CONFIG_DIRS'}:
            directory=root/key;directory.mkdir(mode=0o700);env[key]=str(directory)
        env.update(XDG_CURRENT_DESKTOP='niri',DBUS_SESSION_BUS_ADDRESS='unix:path='+str(root/'no-bus'))
        if empty_xdg:env.update(XDG_DATA_HOME='',XDG_CONFIG_HOME='')
        data=Path(env['XDG_DATA_HOME'] or str(Path(env['HOME'])/'.local/share'))
        config=Path(env['XDG_CONFIG_HOME'] or str(Path(env['HOME'])/'.config'));config.mkdir(parents=True,exist_ok=True)
        apps=data/'applications';apps.mkdir(parents=True)
        source=root/'modlock';source.write_bytes(b'#!/bin/sh\nexit 0\n');source.chmod(0o700)
        old=apps/'deadlock-mods.desktop'
        mime=apps/'mimeapps.list';defaults=apps/'defaults.list'
        seed=b'[Default Applications]\nx-scheme-handler/dlmm=deadlock-mods.desktop;\napplication/x-keep=foreign.desktop;\n[Added Associations]\nx-scheme-handler/foreign=deadlock-mods.desktop;\n'
        # An old data-level registration is overridden by xdg-mime's new
        # config-level default; the existing file itself is preserved.
        mime.write_bytes(seed);defaults.write_bytes(seed)
        cwd=root/'cwd';cwd.mkdir()
        for name in ['deadlock-mods.desktop','mimeapps.list']:(cwd/name).write_bytes(b'cwd foreign sentinel')
        for exe in ['/bin/true','/opt/deadlock-native','/opt/independent-fork/deadlock-native --opt']:
            body=f'[Desktop Entry]\nType=Application\nName=DL Mod Manager\nExec={exe} %u\n'.encode();old.write_bytes(body)
            before={p:(p.read_bytes(),p.stat().st_ino,p.stat().st_mtime_ns) for p in [old,mime,defaults]}
            result=subprocess.run([str(repo/installer),str(source)],env=env,cwd=cwd,capture_output=True,text=True)
            assert result.returncode==0,result.stdout+result.stderr
            for p,expected in before.items():assert (p.read_bytes(),p.stat().st_ino,p.stat().st_mtime_ns)==expected,p
            assert result.stdout.splitlines().count('An old launcher is still present: '+str(old))==1,result.stdout
            for name in ['deadlock-mods.desktop','mimeapps.list']:assert (cwd/name).read_bytes()==b'cwd foreign sentinel'
            text=(apps/'modlock.desktop').read_text();assert 'Name=Modlock\n' in text and 'Icon=modlock\n' in text
            argv=exec_arguments(next(line[5:] for line in text.splitlines() if line.startswith('Exec=')))
            target=Path(env['HOME'])/'Applications/Modlock-x86_64.AppImage' if installer.startswith('tools/') else source
            assert argv==['/usr/bin/env',str(target),'%u'],(installer,argv,target)
            for scheme in ['dlmm','deadlock-mod-manager','deadlock-modmanager']:
                registered=subprocess.check_output(['xdg-mime','query','default','x-scheme-handler/'+scheme],env=env,text=True).strip()
                assert registered=='modlock.desktop',(scheme,registered)
        print('PASS old launcher/MIME bytes and CWD preserved; notice, new default and Exec round-trip:',installer,'empty XDG:',empty_xdg,flush=True)
