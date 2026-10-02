#!/usr/bin/env python3
"""Operator-led PM freezer/devices/platform diagnostic, never genuine sleep qualification.

Check is read-only. Run must be invoked locally by the authorized operator.
No module operation, reboot, boot edit, force action or retry.
"""
import argparse,fcntl,importlib.util,json,os
from pathlib import Path
import re,subprocess,sys
ROOT=Path(__file__).resolve().parent

def load_cycle():
 spec=importlib.util.spec_from_file_location('cycle',ROOT/'qualify-iris-module-cycle.py')
 m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m

def selected(text):
 match=re.search(r'\[([^]]+)\]',text)
 if not match:raise ValueError('missing selected PM test mode')
 return match.group(1)

def require_preceding(stage,state,evidence):
 if stage!='platform':return
 if evidence is None:raise ValueError('platform requires preceding device-stage evidence')
 result=json.loads((evidence/'result.json').read_text())
 after=json.loads((evidence/'after.json').read_text())
 if result.get('status')!='pass' or result.get('stage')!='devices':
  raise ValueError('preceding devices stage did not pass')
 if after.get('boot_id')!=state['boot_id'] or after.get('build_id')!=state['build_id']:
  raise ValueError('preceding device-stage identity mismatch')
 if selected(after['pm_test'])!='none':raise ValueError('preceding test mode unrestored')


def classify(stage,before,after,messages,process):
 if before['boot_id']!=after['boot_id'] or before['build_id']!=after['build_id']:
  raise ValueError('boot/module changed')
 if selected(before['pm_test'])!='none' or selected(after['pm_test'])!='none':
  raise ValueError('PM test mode not restored')
 for key in ('pm_debug_messages','pm_print_times'):
  if stage=='platform' and (key not in before or key not in after):raise ValueError('missing PM control restoration witness')
  if key in before and after.get(key)!=before[key]:raise ValueError('PM diagnostic control not restored: '+key)
 load_cycle().require_clean_messages(messages)
 entry=messages.find('PM: suspend entry')
 if entry<0 or messages.find('PM: suspend exit',entry)<0 or 'suspend debug: Waiting' not in messages:
  raise ValueError('missing completed PM diagnostic journal window')
 if process.get('exit_status')!=0 or process.get('timed_out') is not False or process.get('lingering_descendants') is not False:
  raise ValueError('diagnostic did not exit cleanly')
 return {'status':'pass','stage':stage,'scope':'PM '+stage+' dry run only; real system sleep and post-wake decode unqualified'}

def worker(stage,evidence):
 if os.geteuid()!=0:raise ValueError('operator root invocation required')
 cycle=load_cycle();state=cycle.inspect(cycle.activation_helper())
 before=json.loads((evidence/'before.json').read_text())
 if state['boot_id']!=before['boot_id']:raise ValueError('boot changed')
 paths=[Path('/sys/power/pm_test'),Path('/sys/power/pm_debug_messages'),Path('/sys/power/pm_print_times')]
 old=[selected(paths[0].read_text()),paths[1].read_text().strip(),paths[2].read_text().strip()]
 if old[0]!='none':raise ValueError('another PM test mode active')
 try:
  paths[1].write_text('1');paths[2].write_text('1');paths[0].write_text(stage)
  if selected(paths[0].read_text())!=stage:raise ValueError('test mode selection failed')
  # ONLY the operator's worker reaches this diagnostic transition.
  Path('/sys/power/state').write_text('mem')
 finally:
  failures=[]
  for path,value in zip(paths,old):
   try:path.write_text(value)
   except OSError as error:failures.append(str(error))
  if failures:raise ValueError('PM diagnostic settings restore failed: '+'; '.join(failures))


def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('action',choices=['check','run','worker']);p.add_argument('stage',choices=['freezer','devices','platform']);p.add_argument('evidence',type=Path);p.add_argument('--preceding',type=Path);a=p.parse_args()
 if a.action=='worker':worker(a.stage,a.evidence);return 0
 cycle=load_cycle();helper=cycle.activation_helper()
 with helper.open_lease() as lease:
  fcntl.flock(lease,fcntl.LOCK_EX|fcntl.LOCK_NB)
  state=cycle.inspect(helper);state['pm_test']=Path('/sys/power/pm_test').read_text().strip();state['pm_debug_messages']=Path('/sys/power/pm_debug_messages').read_text().strip();state['pm_print_times']=Path('/sys/power/pm_print_times').read_text().strip()
  if selected(state['pm_test'])!='none':raise ValueError('another PM test active')
  if a.stage not in state['pm_test'].replace('[','').replace(']','').split():raise ValueError('requested PM stage unavailable')
  cycle.require_clean_messages(cycle.journal('-o','cat'))
  require_preceding(a.stage,state,a.preceding)
  if a.action=='check':print(json.dumps({'status':'ready','stage':a.stage,'state':state,'no_transition':True}));return 0
  if os.geteuid()!=0:raise ValueError('operator root invocation required')
  a.evidence.mkdir(parents=True,exist_ok=False)
  (a.evidence/'before.json').write_text(json.dumps(state,indent=2)+'\n')
  cursor=json.loads(cycle.journal('-n','1','-o','json'))['__CURSOR']
  command=[str(ROOT/'capture-iris-kernel-log.sh'),'--',sys.executable,str(ROOT/'measure-process-tree.py'),'--seconds','45','--output',str(a.evidence/'process.json'),'--',sys.executable,str(Path(__file__).resolve()),'worker',a.stage,str(a.evidence)]
  with (a.evidence/'observer.log').open('x') as log:status=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=65).returncode
  messages=cycle.journal('--after-cursor='+cursor,'-o','cat');(a.evidence/'kernel.log').write_text(messages)
  process=json.loads((a.evidence/'process.json').read_text())
  try:
   after=cycle.inspect(helper);after['pm_test']=Path('/sys/power/pm_test').read_text().strip();after['pm_debug_messages']=Path('/sys/power/pm_debug_messages').read_text().strip();after['pm_print_times']=Path('/sys/power/pm_print_times').read_text().strip();(a.evidence/'after.json').write_text(json.dumps(after,indent=2)+'\n')
   if status:raise ValueError('observer or diagnostic failed')
   result=classify(a.stage,state,after,messages,process)
  except ValueError as error:result={'status':'fail','stage':a.stage,'reason':str(error),'no_retry':True,'scope':'PM diagnostic only; stop further tests'}
  (a.evidence/'result.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result));return 0 if result['status']=='pass' else 1

if __name__=='__main__':raise SystemExit(main())
