#!/usr/bin/env python3
"""Item 121 regressions using a disposable protocol peer and production QML."""
import argparse
import json
import os
from pathlib import Path
from paired_helper import apply_review
import subprocess
import tempfile

p = argparse.ArgumentParser()
p.add_argument('binary', type=Path)
p.add_argument('--captures', type=Path)
a = p.parse_args()
peer_source = Path(__file__).with_name('controlled-helper.py').read_text()
peer_source = peer_source.replace('import json\n', 'import sys\nsys.path.insert(0, '+repr(str(Path(__file__).resolve().parent))+')\nimport json\n', 1)
# Use the established native fixture; intercept only these scenarios' responses.
peer_source = peer_source.replace("    request_id = request['id']", """    request_id = request['id']
    if request['method'] == 'native.snapshot' and os.environ.get('FIX789_LOADING'):
        time.sleep(.4)
    if request['method'] == 'native.game.detect':
        send(request_id, {'detectedGamePath':'', 'candidates':[]}); continue
    if request['method'] == 'native.installed.scan':
        send(request_id, {'items':[{'id':'unidentified', 'path':'/synthetic/unidentified_dir.vpk', 'packages':[], 'sizeBytes':2048, 'alreadyInLibrary':False, 'error':None}]}); continue
    if request['method'] == 'native.applyPreview':
        send(request_id, json.loads(os.environ['FIX789_REVIEW'])); continue
    if request['method'] == 'native.performance.select':
        preset = request['params'].get('presetId')
        if preset != '' and preset not in ('balanced',):
            print(json.dumps({'jsonrpc':'2.0', 'id':request_id, 'error':{'code':-32602, 'message':'Invalid parameters'}}),flush=True); continue
        Path(os.environ['FIX789_SELECTION']).write_text(json.dumps(request['params']))
        request['_selection'] = True
        request['method'] = 'native.snapshot'
""")
peer_source = peer_source.replace("    line = json.dumps({'jsonrpc'", """    if isinstance(result, dict):
        if 'selectedProfileId' in result and 'sourceKind' in result:
            result.update(json.loads(os.environ.get('FIX789_SNAPSHOT', '{}')))
            if request.get('_selection'): result['revision'] = '2'
        if 'selection' in result and os.environ.get('FIX789_INHERITED'):
            result.update(selection='', explicitSelection=False, detected={'state':'preset', 'presetId':'balanced'})
        if 'selection' in result and os.environ.get('FIX789_EXPLICIT_NONE'):
            result.update(selection='', explicitSelection=True, detected={'state':'preset', 'presetId':'balanced'})
    line = json.dumps({'jsonrpc'""")
review = apply_review('default', count=1, total=2048, warnings=[dict(code='fileConflict', message='Mod A wins over Mod B for materials/test.vtex_c (lower package order).'),
              dict(code='additionalConflicts', message='23 additional overlapping resources follow the same deterministic package order.')])
with tempfile.TemporaryDirectory(prefix='u789-') as temporary:
    root = Path(temporary)
    peer = root/'native-performance'; peer.write_text(peer_source); peer.chmod(0o700)
    env = os.environ.copy()
    env.pop('QML_DISK_CACHE_PATH', None)
    env['MODLOCK_UI_TEST_ROOT'] = str(root)
    for key in ('HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','TMPDIR'):
        folder=root/key; folder.mkdir(); env[key]=str(folder)
    env.pop('DISPLAY',None); env.pop('WAYLAND_DISPLAY',None)
    (root/'r').mkdir(mode=0o700)
    env.update(QT_QPA_PLATFORM='offscreen', QT_QUICK_BACKEND='software', XDG_RUNTIME_DIR=str(root/'r'),
               FIX789_REVIEW=json.dumps(review), FIX789_SELECTION=str(root/'selection.json'))
    cases=[('options-720','fix789-options',{}), ('profiles','fix789-profiles',{}),
           ('inherited-none','fix789-inherited',{}), ('explicit-none','fix789-explicit-none',{}), ('conflicts','fix789-conflicts',{}), ('installed','fix789-installed',{})]
    home_states = {'ready':dict(appliedProfileId='default'), 'empty':dict(appliedProfileId=None,mods=[]),
                   'pending':dict(pendingChanges=True,pendingStatus='pending'), 'unapplied':dict(appliedProfileId=None),
                   'setup':dict(gamePath=''), 'recovery':dict(recoveryRequired=True),
                   'choices':dict(pendingStatus='selectionRequired'), 'unknown':dict(pendingStatus='unknown'),
                   'external':dict(pendingStatus='pending',pendingReason='externalGameinfoChange')}
    headlines = dict(ready='Ready to play', empty='Changes waiting for your next game', pending='Changes waiting for your next game', unapplied='Changes waiting for your next game', setup="Deadlock isn't set up yet", recovery='The previous change needs checking', choices='Choose files for some mods before playing', unknown='Your game setup needs checking', external='Another mod manager or a game update changed the game files')
    themes = ['dark','light'] if a.captures else ['dark']
    if a.captures:
        cases += [('home-'+k,'review-capture',v) for k,v in home_states.items()]
        cases += [('home-loading','item121-loading',{})]
    for theme in themes:
        for name,scenario,patch in cases:
            case = root/(theme+'-'+name); case.mkdir()
            for key in ('HOME','XDG_CONFIG_HOME','XDG_DATA_HOME','XDG_CACHE_HOME','XDG_STATE_HOME'):
                folder=case/key; folder.mkdir(); env[key]=str(folder)
            # A complete KDE palette exercises both production token palettes.
            bg, fg = ('14,12,21','238,232,220') if theme=='dark' else ('241,239,245','27,22,38')
            palette = ''.join('[Colors:'+group+']\nBackgroundNormal='+bg+'\nForegroundNormal='+fg+'\nBackgroundAlternate='+bg+'\nForegroundInactive='+fg+'\nForegroundLink=82,161,231\nForegroundVisited=151,111,211\n' for group in ('Window','View','Button','Selection','Tooltip','Complementary'))
            (Path(env['XDG_CONFIG_HOME'])/'kdeglobals').write_text(palette)
            env.update(APPIMAGE=str(a.binary.resolve()), XDG_CURRENT_DESKTOP='KDE', QT_QPA_PLATFORMTHEME='palette-none', QT_QPA_PLATFORM_PLUGIN_PATH=str(a.binary.resolve().parent/'palette-test-theme'))
            data=root/'native'; data.mkdir(exist_ok=True)
            runenv=env|dict(INSTALL_LINK_TRACE=str(root/'trace'),FIX789_SNAPSHOT=json.dumps(dict(pendingChanges=False,pendingStatus='current',pendingReason=None,appliedProfileId='default')|patch),
                            FIX789_EXPLICIT_NONE='1' if name=='explicit-none' else '', FIX789_LOADING='1' if name=='home-loading' else '', FIX789_INHERITED='1' if name=='inherited-none' else '', UI_REVIEW_PAGE='installed' if name=='installed' else 'home',
                            UI_REVIEW_WIDTH='1200',UI_REVIEW_HEIGHT='780', UI_REVIEW_EXPECT_HOME=headlines.get(name.removeprefix('home-'), '') if name.startswith('home-') else '')
            if a.captures:
                out=a.captures/(theme+'-'+name); out.mkdir(parents=True,exist_ok=True);runenv['UI_NATIVE_CAPTURES']=str(out);runenv['UI_LOADING_CAPTURE']=str(out/'loading.png')
            result=subprocess.run([str(a.binary.resolve()),'--helper',str(peer),'--data-dir',str(data),'--check-ui',scenario],env=runenv,capture_output=True,text=True,timeout=30)
            if a.captures: (out/'run.log').write_text(result.stdout+result.stderr)
            assert result.returncode==0 and 'PASS UI' in result.stdout, (name,result.stdout,result.stderr, (root/'trace').read_text()[-5000:] if (root/'trace').exists() else 'no trace')
            if name in ('inherited-none','explicit-none'): assert json.loads((root/'selection.json').read_text())['presetId']=='', 'None must use the baseline empty selection ID'
            for marker in (Path(env['XDG_CONFIG_HOME'])/'dl-mod-manager/state/pending').glob('*.json'):
                marker.unlink()
            print('PASS',theme,name,flush=True)
