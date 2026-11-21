#!/usr/bin/python3
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import hashlib
import json
import time
with tempfile.TemporaryDirectory(prefix='il-') as temp:
    root=Path(temp)
    (root/'run').mkdir(mode=0o700)
    helper=root/'native-links'
    helper.symlink_to(Path(__file__).with_name('controlled-helper.py'))
    cases = [('native-install-link','dlmm:https://gamebanana.com/mmdl/12,Mod,42'),('native-install-link-invalid','dlmm:https://evil.test/mmdl/12,Mod,42'),('native-install-link-blocked-forwarded','dlmm:https://gamebanana.com/mmdl/12,Mod,42'),('native-install-link-forwarded','dlmm:https://gamebanana.com/mmdl/12,Mod,42')]
    cases += [('native-install-link-auto-'+case,'dlmm:https://gamebanana.com/mmdl/12,Mod,42') for case in ['one','two','paged','metadata','closed','rebase','review-failure','bubble','multi','choice-once','bubble-error','bubble-dismiss','bubble-repeat','multi-changed']]
    cases += [('native-install-link-auto-'+case,'') for case in ['page-linked','page-linked-multi','page-linked-variants','page-linked-bulk','page-linked-bulk-inactive','batch-attention','legacy-link','known-receipt','batch-auto','browse-linked','browse-linked-refused','browse-linked-repeat']]
    selected=os.environ.get('INSTALL_LINK_CASES','').split(',')
    evidence=Path(os.environ['INSTALL_UI_EVIDENCE']) if os.environ.get('INSTALL_UI_EVIDENCE') else None
    for scenario,link in cases:
        if selected!=[''] and scenario not in selected: continue
        started=time.monotonic()
        print('START',scenario,flush=True)
        data=root/scenario/'native'; data.mkdir(parents=True)
        env=os.environ|{'QT_QPA_PLATFORM':'offscreen','QT_QUICK_BACKEND':'software','HOME':str(root/'home'),'XDG_CACHE_HOME':str(root/'cache'),'XDG_STATE_HOME':str(root/'state'),'XDG_RUNTIME_DIR':str(root/'run'),'XDG_CONFIG_HOME':str(root/'config'),'XDG_DATA_HOME':str(root/'data')}
        trace=data.parent/'protocol.jsonl'
        env['INSTALL_LINK_TRACE']=str(trace)
        if '-auto-' in scenario:
            case=scenario.split('-auto-')[1]
            env |= {'UI_NATIVE_LONG_FIXTURE':'1','INSTALL_LINK_AUTO':'1','INSTALL_LINK_CASE':{'bubble-dismiss':'bubble-error','bubble-repeat':'bubble-error','browse-linked-repeat':'browse-linked-refused','batch-attention':'page-linked-bulk'}.get(case,case),'UI_BUBBLE_DISMISS':'1' if case in ('bubble-dismiss','bubble-repeat') else '0','UI_BATCH_ATTENTION':'1' if case == 'batch-attention' else '0','INSTALL_LINK_CANDIDATES':'2' if case in ['two','paged','choice-once'] else '1'}
            if case=='page-linked-variants': env['INSTALL_LINK_CANDIDATES']='2'
            if case=='bubble-repeat': env['UI_BUBBLE_REPEAT']='1'; env['INSTALL_LINK_REPEAT_ERROR']='1'
            if case=='browse-linked-repeat': env['UI_BROWSE_REPEAT']='1'
        if evidence:
            capture=evidence/scenario;capture.mkdir(parents=True,exist_ok=True)
            trace=capture/'protocol.jsonl'
            env |= {'UI_NATIVE_CAPTURES':str(capture),'INSTALL_LINK_TRACE':str(trace)}
        trace.write_text('')
        blocked=scenario=='native-install-link-blocked-forwarded'
        if blocked:
            config=root/'blocked-config';config.mkdir()
            target=root/'link-target';target.mkdir()
            (config/'dl-mod-manager').symlink_to(target)
            env['XDG_CONFIG_HOME']=str(config)
            env['INSTALL_LINK_EXPECT_BLOCKED']='1'
        if scenario.endswith('forwarded'):
            runtime=root/('rb' if blocked else 'r'); runtime.mkdir(mode=0o700)
            env['XDG_RUNTIME_DIR']=str(runtime)
            command=[sys.argv[1],'--helper',str(helper),'--data-dir',str(data)]
            server=subprocess.Popen(command+['--check-ui','native-install-link-forwarded' if blocked else scenario],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
            try:
                socket=runtime/'deadlock-native'/hashlib.sha256(str(data).encode()).hexdigest()[:24]
                deadline=time.monotonic()+8
                while not socket.exists() and time.monotonic()<deadline:
                    assert server.poll() is None,server.communicate()
                    time.sleep(.02)
                forwarded=subprocess.run(command+[link],env=env,capture_output=True,text=True,timeout=5)
                assert forwarded.returncode==0,(forwarded.stdout,forwarded.stderr)
                out,err=server.communicate(timeout=20)
                assert server.returncode==0 and 'PASS UI' in out,(out,err)
                if blocked: assert not list(target.iterdir())
            finally:
                if server.poll() is None: server.kill();server.communicate()
            print('PASS',scenario, '%.2fs' % (time.monotonic()-started),flush=True)
            continue
        p=subprocess.run([sys.argv[1],'--helper',str(helper),'--data-dir',str(data),'--check-ui','native-install-link' if '-auto-' in scenario else scenario]+([link] if link else []),env=env,capture_output=True,text=True,timeout=45)
        if evidence: (capture/'output.log').write_text(p.stdout+p.stderr)
        assert p.returncode==0 and 'PASS UI' in p.stdout,(scenario,p.stdout,p.stderr)
        requests=[json.loads(line) for line in trace.read_text().splitlines()] if '-auto-' in scenario else []
        assert not any(r.get('method')=='native.apply' for r in requests), 'A download UI fixture must not write nonexistent game files'
        if scenario.endswith('-batch-auto'):
            starts=list({r['params']['jobId']:r['params'] for r in requests if r.get('method')=='native.remote.replaceStart'}.values())
            assert [r['modId'] for r in starts]==['page-linked','unrelated'],starts
            assert all(r['purpose']=='update' for r in starts),starts
        if '-browse-linked' in scenario:
            starts=[r['params'] for r in requests if r.get('method')=='native.remote.replaceStart']
            assert starts and all(r['purpose']=='append' for r in starts),starts
        if '-page-linked-bulk' in scenario:
            print('PASS',scenario, '%.2fs' % (time.monotonic()-started),flush=True)
            continue
        if '-page-linked' in scenario:
            starts=list({r['params']['jobId']:r['params'] for r in requests if r.get('method')=='native.remote.replaceStart'}.values())
            assert [r['purpose'] for r in starts]==(['update','append'] if scenario.endswith('-multi') else ['update']),starts
            assert all(r['modId']=='page-linked' for r in starts)
            assert not any(r.get('method') in ['native.remote.start','native.remote.commit'] for r in requests)
            targets=[r['response']['target'] for r in requests if isinstance(r.get('response'),dict) and r['response'].get('target')]
            assert targets[0]['receipt'] is None and targets[-1]['receipt']['submission']['submissionId']=='42'
            assert targets[-1]['receipt']['rootFileId']==starts[0]['fileId']
            assert targets[-1]['selectedOptionIds']==['d'*32]
            assert all(t['enabled'] for t in targets)
        if scenario.endswith('-metadata'):
            reviews=[r['params']['metadata'] for r in requests if r.get('method')=='native.remote.review']
            expected={'name':'Browse mod','author':'Browse author','category':'Skins','description':'A plain submission description.'}
            assert reviews and all(form==expected for form in reviews),reviews
        if scenario.endswith(('-two','-paged')):
            assert not any(r.get('method')=='native.remote.review' for r in requests),'unconfirmed variant selection was reviewed'
        if scenario.endswith(('-two','-paged','-review-failure')):
            assert not any(r.get('method')=='native.remote.commit' for r in requests),'unconfirmed variant selection was committed'
        if scenario.endswith('-multi-changed'):
            assert {r['params']['fileId'] for r in requests if r.get('method') in ['native.remote.start','native.remote.replaceStart']}=={'12'},'changed file was downloaded'
        if scenario.endswith('-choice-once'):
            assert len([r for r in requests if r.get('method')=='native.remote.review'])==1,'archive choice was requested more than once'
        if scenario.endswith('-multi'):
            starts=[r for r in requests if r.get('method') in ['native.remote.start','native.remote.replaceStart']]
            assert len({r['params']['fileId'] for r in starts})==2, starts
            assert any(r.get('method')=='native.remote.replaceStart' and r['params']['purpose']=='append' for r in requests)
            assert len({r['params']['requestKey'] for r in requests if r.get('method')=='native.remote.resolve'})==2,'next file did not receive a fresh provider check'
        print('PASS',scenario, '%.2fs' % (time.monotonic()-started),flush=True)
