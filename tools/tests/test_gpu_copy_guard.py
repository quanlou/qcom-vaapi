import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location("gpu_guard", Path(__file__).resolve().parents[1] / "qualify-gpu-copy.py")
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class SessionBaseline(unittest.TestCase):
    def setUp(self):
        self.error = "qcom-iris aa00000.video-codec: session error received 0x4000003: fatal error"
        self.ids = {"qcom_iris": "a" * 40, "qrtr": "b" * 40, "qrtr_mhi": "c" * 40}
        self.baseline = {"boot_id": "boot", "kernel": "kernel", "loaded_build_ids": self.ids,
                         "session_errors": [self.error] * 5}
        self.journal = "\n".join(["normal startup", *self.baseline["session_errors"]])

    def check(self, journal, baseline=None):
        guard.check_journal(journal, "boot", "kernel", self.ids, baseline)

    def test_default_accepts_clean_boot_and_rejects_prior_errors(self):
        self.check("normal startup")
        with self.assertRaises(RuntimeError):
            self.check(self.journal)

    def test_exact_explicit_baseline_can_proceed(self):
        self.check(self.journal, self.baseline)

    def test_an_additional_session_error_always_stops(self):
        with self.assertRaises(RuntimeError):
            self.check(self.journal + "\n" + self.error, self.baseline)

    def test_system_memory_and_gpu_faults_never_get_acknowledged(self):
        for fault in ["received system error", "Unhandled context fault", "GPU HANG",
                      "Bad page", "Corrupted page table", "WARNING: damaged state", "KASAN: bounds"]:
            with self.subTest(fault=fault), self.assertRaises(RuntimeError):
                self.check(self.journal + "\n" + fault, self.baseline)
            with self.subTest(baseline=fault), self.assertRaises(RuntimeError):
                self.check(self.journal + "\n" + fault,
                           {**self.baseline, "session_errors": [*self.baseline["session_errors"], fault]})

    def test_fpac_oops_and_panic_stop_clean_and_acknowledged_boots(self):
        import subprocess
        classifier = Path(__file__).resolve().parents[1] / "capture-iris-kernel-log.sh"
        for fault in ["Internal error: Oops - FPAC: 0000000072000000 [#1] SMP",
                      "Oops: fatal fault", "Kernel panic - not syncing: fatal exception"]:
            with self.subTest(fault=fault):
                with self.assertRaises(RuntimeError):
                    self.check("normal startup\n" + fault)
                with self.assertRaises(RuntimeError):
                    self.check(self.journal + "\n" + fault, self.baseline)
                result = subprocess.run([str(classifier), "--classify"], input=fault+"\n",
                                        text=True, capture_output=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("kernel-bugs=1", result.stdout)

    def test_changed_identity_or_baseline_cannot_proceed(self):
        for changed in [{"boot_id": "another"}, {"kernel": "another"}, {"loaded_build_ids": {}},
                        {"session_errors": []}, {"session_errors": [self.error.replace("4000003", "4000004")]},
                        {"session_errors": [self.error] * 4}]:
            with self.subTest(changed=changed), self.assertRaises(RuntimeError):
                self.check(self.journal, {**self.baseline, **changed})

    def test_missing_journal_cannot_proceed(self):
        with self.assertRaises(RuntimeError):
            self.check("", self.baseline)


if __name__ == "__main__":
    unittest.main()
