#!/usr/bin/env python3
"""Operator-only normal rmmod refusal with an open decoder client; never force/retry.

Check is read-only. The operator worker holds a decoder fd, requires a positive
module reference, and attempts plain rmmod exactly once. It never reloads modules.
This qualifies an open idle client, not active streaming or arbitrary hot removal.
"""
import argparse, fcntl, importlib.util, json, os, subprocess, sys, time
from pathlib import Path
ROOT=Path(__file__).resolve().parent

def cycle():
 spec=importlib.util.spec_from_file_location('cycle',ROOT/'qualify-iris-module-cycle.py')
 m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m

def classify(before,held,after,returncode,stderr):
 if held.get('refcount',0)<=0 or after.get('refcount',0)<=0:raise ValueError('missing held module references')
 if any(s.get('boot_id')!=before['boot_id'] or s.get('build_id')!=before['build_id'] for s in (held,after)):raise ValueError('boot or loaded module changed')
 if returncode==0 or 'in use' not in stderr.lower():raise ValueError('normal removal was not specifically refused as busy')
 return {'status':'pass','scope':'normal module removal refused with open idle decoder client only; active streaming unqualified'}

def held_state(helper):
 return {'boot_id':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'build_id':helper.loaded_build_id(),'refcount':int(Path('/sys/module/qcom_iris/refcnt').read_text())}

def worker(evidence):
 if os.geteuid()!=0:raise ValueError('authorized local operator root invocation required')
 c=cycle();helper=c.activation_helper();before=json.loads((evidence/'before.json').read_text());now=c.inspect(helper)
 if now['boot_id']!=before['boot_id']:raise ValueError('boot changed')
 c.require_clean_messages(c.journal('-o','cat'))
 device=subprocess.check_output([sys.executable,str(ROOT/'select-iris-device.py')],text=True).strip()
 fd=os.open(device,os.O_RDWR|os.O_NONBLOCK)
 try:
  held=held_state(helper)
  if held['refcount']<=0 or held['build_id']!=before['build_id'] or held['boot_id']!=before['boot_id']:raise ValueError('module pin not witnessed; no removal attempted')
  (evidence/'held.json').write_text(json.dumps(held,indent=2)+'\n')
  result=subprocess.run(['rmmod','qcom_iris'],capture_output=True,text=True,timeout=10,env=dict(os.environ,LC_ALL='C'))
  after=held_state(helper)
  (evidence/'removal-command.json').write_text(json.dumps({'command':['rmmod','qcom_iris'],'returncode':result.returncode,'stdout':result.stdout,'stderr':result.stderr,'held_after':after},indent=2)+'\n')
  verdict=classify(before,held,after,result.returncode,result.stderr)
 finally:os.close(fd)
 deadline=time.monotonic()+15
 while True:
  try:idle=c.inspect(helper);break
  except ValueError:
   if time.monotonic()>=deadline:raise
   time.sleep(.1)
 (evidence/'after.json').write_text(json.dumps(idle,indent=2)+'\n')
 (evidence/'worker-result.json').write_text(json.dumps(verdict,indent=2)+'\n')

def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=['check','run','worker']);p.add_argument('evidence',type=Path);a=p.parse_args()
 if a.action=='worker':worker(a.evidence);return
 c=cycle();helper=c.activation_helper()
 with helper.open_lease() as lease:
  fcntl.flock(lease,fcntl.LOCK_EX|fcntl.LOCK_NB);state=c.inspect(helper);c.require_clean_messages(c.journal('-o','cat'))
  if a.action=='check':print(json.dumps({'status':'ready','state':state,'no_decoder_open':True,'no_module_operation':True}));return
  if os.geteuid()!=0:raise ValueError('operator sudo required')
  a.evidence.mkdir(parents=True,exist_ok=False);(a.evidence/'before.json').write_text(json.dumps(state,indent=2)+'\n')
  cursor=json.loads(c.journal('-n','1','-o','json'))['__CURSOR']
  command=[str(ROOT/'capture-iris-kernel-log.sh'),'--',sys.executable,str(ROOT/'measure-process-tree.py'),'--seconds','35','--output',str(a.evidence/'process.json'),'--',sys.executable,str(Path(__file__).resolve()),'worker',str(a.evidence)]
  with (a.evidence/'observer.log').open('x') as log:status=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=55).returncode
  messages=c.journal('--after-cursor='+cursor,'-o','cat');(a.evidence/'kernel.log').write_text(messages)
  try:
   c.require_clean_messages(messages);after=c.inspect(helper)
   process=json.loads((a.evidence/'process.json').read_text())
   if status or process.get('exit_status')!=0 or process.get('timed_out') is not False or process.get('lingering_descendants') is not False:raise ValueError('worker/observer failed or did not exit cleanly')
   if after['boot_id']!=state['boot_id'] or after['build_id']!=state['build_id']:raise ValueError('final identity changed')
   result=json.loads((a.evidence/'worker-result.json').read_text())
  except Exception as e:result={'status':'fail','reason':str(e),'no_retry':True}
  (a.evidence/'result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result));return 0 if result['status']=='pass' else 1
if __name__=='__main__':raise SystemExit(main())
