import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "module_cycle", Path(__file__).resolve().parents[1] / "qualify-iris-module-cycle.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class ModuleCyclePreflightTests(unittest.TestCase):
    def setUp(self):
        self.values = ["clean", module.KERNEL, module.BUILD, 0,
                       {"runtime_status": "suspended", "runtime_usage": "0", "control": "auto"},
                       module.MODULE_SHA]

    def test_exact_idle_candidate_is_accepted(self):
        module.validate_idle(*self.values)

    def test_busy_clients_are_rejected(self):
        self.values[3] = 1
        with self.assertRaises(ValueError):
            module.validate_idle(*self.values)

    def test_bad_identities_are_rejected(self):
        for index in (1, 2, 5):
            with self.subTest(index=index), self.assertRaises(ValueError):
                values = self.values.copy()
                values[index] = "wrong"
                module.validate_idle(*values)

    def test_faulted_boots_are_rejected(self):
        for boot in module.FAULTED:
            with self.subTest(boot=boot), self.assertRaises(ValueError):
                module.validate_idle(boot, *self.values[1:])

    def test_unsettled_power_is_rejected(self):
        for field, value in (("runtime_status", "active"), ("runtime_usage", "1"), ("control", "on")):
            with self.subTest(field=field), self.assertRaises(ValueError):
                values = self.values.copy()
                values[4] = {**values[4], field: value}
                module.validate_idle(*values)

    def test_fault_between_removal_and_reload_stops_reload(self):
        for message in ("session error received 0x4000003", "BUG: use-after-free", "task blocked for more than 120 seconds"):
            with self.subTest(message=message), self.assertRaises(ValueError):
                module.require_clean_messages(message)

    def test_normal_message_does_not_block_reload(self):
        module.require_clean_messages("qcom_iris unloaded\n")
