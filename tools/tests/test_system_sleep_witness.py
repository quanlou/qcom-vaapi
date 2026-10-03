import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "sleep_witness", Path(__file__).resolve().parents[1] / "observe-iris-system-sleep.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class SleepWitnessTests(unittest.TestCase):
    def setUp(self):
        self.before = {"boot_id": "a", "build_id": "b", "sleep_success": 0,
                       "pm_test": "[none] core processors platform devices freezer"}
        self.after = {**self.before, "sleep_success": 1}
        self.log = "PM: suspend entry (deep)\nPM: suspend exit\n"

    def test_complete_cycle_has_limited_scope(self):
        result = module.verify_witness(self.before, self.after, self.log)
        self.assertEqual(result["sleep_mode"], "deep")
        self.assertIn("post-resume decode", result["scope"])

    def test_known_faulted_boot_is_refused_before_module_or_power_access(self):
        for boot in module.FAULTED:
            with self.subTest(boot=boot), patch.object(Path, 'read_text', return_value=boot), \
                 patch.object(Path, 'read_bytes') as note:
                with self.assertRaisesRegex(ValueError, 'known faulted boot'):
                    module.snapshot()
                note.assert_not_called()

    def test_fault_even_after_resume_rejects(self):
        for fault in ("session error received 0x4000003", "WARNING: bad", "task blocked for more than 120 seconds"):
            with self.subTest(fault=fault), self.assertRaises(ValueError):
                module.verify_witness(self.before, self.after, self.log + fault)

    def test_reboot_or_module_change_rejects(self):
        for field in ("boot_id", "build_id"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                module.verify_witness(self.before, {**self.after, field: "changed"}, self.log)

    def test_no_new_cycle_rejects(self):
        with self.assertRaises(ValueError):
            module.verify_witness(self.before, self.before, self.log)

    def test_dry_run_or_missing_mode_cannot_qualify_sleep(self):
        for mode in (None, "none core processors platform [devices] freezer"):
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                module.verify_witness(self.before, {**self.after, "pm_test": mode}, self.log)
        with self.assertRaises(ValueError):
            module.verify_witness(self.before, self.after,
                                 self.log + "suspend debug: Waiting for 5 seconds.\n")

    def test_informational_deprecation_is_not_a_kernel_warning_header(self):
        result = module.verify_witness(self.before, self.after, self.log +
                                      "warning: glances uses wireless extensions\n")
        self.assertEqual(result["status"], "pass")

    def test_missing_or_reversed_journal_pair_rejects(self):
        for log in ("PM: suspend entry (deep)", "PM: suspend exit", "PM: suspend exit\nPM: suspend entry (deep)"):
            with self.subTest(log=log), self.assertRaises(ValueError):
                module.verify_witness(self.before, self.after, log)
