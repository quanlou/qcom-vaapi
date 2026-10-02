import importlib.util
from pathlib import Path
import unittest
spec=importlib.util.spec_from_file_location('pmstage',Path(__file__).resolve().parents[1]/'qualify-iris-pm-stage.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class StageWitnessTests(unittest.TestCase):
 def test_platform_requires_successful_same_identity_devices_stage(self):
  import tempfile,json
  with tempfile.TemporaryDirectory() as folder:
   prior=Path(folder)
   with self.assertRaises(ValueError):m.require_preceding('platform',self.state,None)
   result={'status':'pass','stage':'devices'}
   (prior/'result.json').write_text(json.dumps(result))
   (prior/'after.json').write_text(json.dumps(self.state))
   m.require_preceding('platform',self.state,prior)
   for change in [{'status':'fail'},{'stage':'freezer'}]:
    (prior/'result.json').write_text(json.dumps({**result,**change}))
    with self.assertRaises(ValueError):m.require_preceding('platform',self.state,prior)
   (prior/'result.json').write_text(json.dumps(result))
   for change in [{'boot_id':'new'},{'build_id':'new'},{'pm_test':'none [devices]'}]:
    (prior/'after.json').write_text(json.dumps({**self.state,**change}))
    with self.assertRaises(ValueError):m.require_preceding('platform',self.state,prior)
 def test_platform_restoration_requires_all_control_witnesses(self):
  m.classify('platform',self.state,self.state,self.log,self.process)
  for key in ('pm_debug_messages','pm_print_times'):
   changed={**self.state,key:'1'}
   with self.subTest(key=key),self.assertRaises(ValueError):m.classify('platform',self.state,changed,self.log,self.process)
   missing=dict(self.state);del missing[key]
   with self.subTest(missing=key),self.assertRaises(ValueError):m.classify('platform',self.state,missing,self.log,self.process)
 def setUp(self):
  self.state={'boot_id':'a','build_id':'b','pm_test':'[none] freezer devices platform','pm_debug_messages':'0','pm_print_times':'0'}
  self.log='PM: suspend entry (deep)\nsuspend debug: Waiting for 5 second(s).\nPM: suspend exit\n'
  self.process={'exit_status':0,'timed_out':False,'lingering_descendants':False}
 def test_completed_dry_run_is_never_sleep_support(self):
  r=m.classify('freezer',self.state,self.state,self.log,self.process);self.assertIn('real system sleep',r['scope'])
 def test_fault_or_incomplete_window_rejected(self):
  for log in [self.log+'WARNING: bad','PM: suspend entry (deep)\n','PM: suspend entry (deep)\nPM: suspend exit\n']:
   with self.subTest(log=log),self.assertRaises(ValueError):m.classify('freezer',self.state,self.state,log,self.process)
 def test_changed_boot_or_unrestored_mode_rejected(self):
  for change in [{'boot_id':'new'},{'build_id':'new'},{'pm_test':'none [devices]'}]:
   with self.subTest(change=change),self.assertRaises(ValueError):m.classify('devices',self.state,{**self.state,**change},self.log,self.process)
 def test_timeout_lingering_or_failed_process_rejected(self):
  for change in [{'timed_out':True},{'lingering_descendants':True},{'exit_status':1}]:
   with self.subTest(change=change),self.assertRaises(ValueError):m.classify('devices',self.state,self.state,self.log,{**self.process,**change})
 def test_restore_attempts_all_controls_after_worker_failure(self):
  from unittest.mock import patch
  from types import SimpleNamespace
  import tempfile,json
  realpath=Path
  writes=[]
  modes={'/sys/power/pm_test':'none'}
  class FakePath:
   def __init__(self,name):self.name=name
   def read_text(self):return '['+modes[self.name]+']' if self.name.endswith('pm_test') else '0'
   def write_text(self,value):
    writes.append((self.name,value))
    if self.name.endswith('pm_test'):modes[self.name]=value
    if self.name.endswith('/state'):raise OSError('injected transition failure')
  with tempfile.TemporaryDirectory() as folder:
   evidence=realpath(folder);(evidence/'before.json').write_text(json.dumps({'boot_id':'a'}))
   cycle=SimpleNamespace(activation_helper=lambda:None,inspect=lambda _: {'boot_id':'a'})
   with patch.object(m,'load_cycle',return_value=cycle),patch.object(m.os,'geteuid',return_value=0),patch.object(m,'Path',side_effect=lambda name:FakePath(str(name))):
    for stage in ('freezer','devices','platform'):
     with self.subTest(stage=stage),self.assertRaises(OSError):m.worker(stage,evidence)
     self.assertEqual(writes[-3:],[('/sys/power/pm_test','none'),('/sys/power/pm_debug_messages','0'),('/sys/power/pm_print_times','0')])
  self.assertEqual(writes[-3:],[('/sys/power/pm_test','none'),('/sys/power/pm_debug_messages','0'),('/sys/power/pm_print_times','0')])
 def test_reversed_journal_pair_rejected(self):
  with self.assertRaises(ValueError):m.classify('freezer',self.state,self.state,'PM: suspend exit\nsuspend debug: Waiting\nPM: suspend entry (deep)',self.process)
