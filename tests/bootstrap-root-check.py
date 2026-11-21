#!/usr/bin/env python3
"""Historical recovery evidence must participate in shared default-root selection."""
import hashlib,json,os,subprocess,sys,tempfile
from pathlib import Path
helper=Path(sys.argv[1]).resolve();failures=[]
with tempfile.TemporaryDirectory(prefix='bootstrap-root-') as temp:
 for case in ['marker','journal','cleared','marker-both','journal-both','malformed','foreign-both','ordinary-both','malformed-both','unsearchable','oversized-both','duplicate-both','spaced-both']:
  root=Path(temp)/case;root.mkdir();env=os.environ.copy()
  for key in set(k for k in env if k.startswith('XDG_'))|{'HOME','TMPDIR','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_STATE_HOME','XDG_RUNTIME_DIR','XDG_CONFIG_DIRS','XDG_DATA_DIRS'}:
   p=root/key;p.mkdir(mode=0o700);env[key]=str(p)
  env['DBUS_SESSION_BUS_ADDRESS']='unix:path='+str(root/'no-bus')
  data=Path(env['XDG_DATA_HOME']);legacy=data/'deadlock-native';modern=data/'modlock'
  def rpc(method,**params):
   r=subprocess.run([str(helper)],input=json.dumps(dict(jsonrpc='2.0',id=1,method=method,params=params))+'\n',env=env,text=True,capture_output=True,timeout=20,check=True);return json.loads(r.stdout)
  if case.endswith('both'):
   assert 'result' in rpc('native.library.initialize',version=1,dataDir=str(modern),operationId='b'*32,expectedRevision='0')
  if case.startswith('journal'):
   legacy.mkdir();evidence=legacy/'library.sqlite3-journal';evidence.write_bytes(b'historical journal')
  else:
   evidence=Path(env['XDG_CONFIG_HOME'])/'dl-mod-manager/state/pending'/(hashlib.sha256(str(legacy).encode()).hexdigest()+'.json');evidence.parent.mkdir(parents=True,mode=0o700)
   value={} if case=='cleared' else dict(bootstrapVersion=1,root=str(legacy),operationId='a'*32,method=52,expectedRevision='0',params=dict(version=1,operationId='a'*32,expectedRevision='0'))
   if case.startswith('foreign'):value['root']=str(Path(temp)/'elsewhere')
   if case.startswith('ordinary'):value=dict(operationId='pending-legacy',method=4)
   text='{"bootstrapVersion":1,' if case.startswith('malformed') else json.dumps(value,sort_keys=True,separators=(',',':'))
   if case.startswith('oversized'):text+=' '*65536+'garbage'  # valid prefix within the read cap
   if case.startswith('duplicate'):text='{"bootstrapVersion":1,'+text[1:]
   if case.startswith('spaced'):text=json.dumps(value,sort_keys=True)
   evidence.write_text(text);evidence.chmod(0o600)
  before=evidence.read_bytes()
  try:
   result=subprocess.run([str(helper),'--data-root'],env=env,text=True,capture_output=True,timeout=10)
   ignored=case.startswith(('malformed','foreign','ordinary','oversized','duplicate','spaced'))
   if case=='unsearchable':
    evidence.parent.chmod(0o000)
    try:result=subprocess.run([str(helper),'--data-root'],env=env,text=True,capture_output=True,timeout=10)
    finally:evidence.parent.chmod(0o700)
    assert result.returncode!=0 and str(evidence.parent) in result.stderr and 'root' not in result.stdout,result.stdout+result.stderr
   elif ignored:
    assert result.returncode==0 and json.loads(result.stdout)['root']==str(modern),result.stdout+result.stderr
   elif case.endswith('both'):
    assert result.returncode!=0 and all(str(p) in result.stderr for p in (legacy,modern)),result.stdout+result.stderr
   else:
    assert result.returncode==0 and json.loads(result.stdout)['root']==str(modern if case=='cleared' else legacy),result.stdout+result.stderr
   if case=='journal':
    blocked=rpc('native.library.initialize',version=1,dataDir=str(legacy),operationId='c'*32,expectedRevision='0')
    assert blocked.get('error',{}).get('data',{}).get('kind')=='orphanLibrarySidecar',blocked
   assert evidence.read_bytes()==before
   assert not (legacy/'library.sqlite3').exists()
   if not case.endswith('both'):assert not modern.exists()
   print('PASS legacy recovery root',case,flush=True)
  except Exception as error:failures.append(case);print('FAIL',case,str(error),flush=True)
assert not failures,failures
