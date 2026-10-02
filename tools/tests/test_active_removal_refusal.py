import importlib.util,unittest
from pathlib import Path
p=Path(__file__).resolve().parents[1]/'qualify-iris-active-removal-refusal.py';s=importlib.util.spec_from_file_location('refusal',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
class Refusal(unittest.TestCase):
 def test_specific_busy_and_pinned_unchanged(self):
  before={'boot_id':'b','build_id':'x'};held={**before,'refcount':1}
  self.assertEqual(m.classify(before,held,held,1,'Module qcom_iris is in use')['status'],'pass')
 def test_permission_wrong_identity_missing_pin_or_success_fail(self):
  b={'boot_id':'b','build_id':'x'};h={**b,'refcount':1}
  for held,after,rc,error in [(h,h,0,'in use'),(h,h,1,'Permission denied'),({**h,'refcount':0},h,1,'in use'),(h,{**h,'refcount':0},1,'in use'),(h,{**h,'build_id':'other'},1,'in use'),({**h,'boot_id':'other'},h,1,'in use')]:
   with self.subTest(held=held,after=after,rc=rc,error=error),self.assertRaises(ValueError):m.classify(b,held,after,rc,error)
if __name__=='__main__':unittest.main()
