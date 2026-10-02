"""Production measurement must not confuse a loaded driver with playback."""
import copy
import importlib.util
import itertools
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("playback_checks", ROOT / "tools/check-playback-performance.py")
checks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(checks)
monitor_spec = importlib.util.spec_from_file_location("process_monitor", ROOT / "tools/measure-process-tree.py")
monitor = importlib.util.module_from_spec(monitor_spec)
monitor_spec.loader.exec_module(monitor)


class MeasurementTests(unittest.TestCase):
    def test_nonfinite_and_invalid_measurements_never_pass(self):
        limits = checks.thresholds(20, 100000, 10, True)
        for value in ("NaN", "inf", "-inf", "1e999", "no", -1, 0, True):
            with self.subTest(value=value):
                for field in range(3):
                    args = [600, 20, 20000]
                    args[field] = value
                    with self.assertRaises(ValueError):
                        checks.check_measurement(*args, limits)

    def test_real_positive_thresholds_required_for_strict_deployment(self):
        for values in ((0, 100000, 10), (20, 0, 10), (20, 100000, 0)):
            self.assertFalse(checks.thresholds(*values, False)[3])
            with self.assertRaisesRegex(ValueError, "positive_deployment_thresholds_required"):
                checks.thresholds(*values, True)
        for value in ("nan", "inf", "1e999", "-1", True):
            with self.assertRaises(ValueError):
                checks.thresholds(value, 100000, 10, False)

    def test_bounds_fail_and_diagnostic_is_not_qualified(self):
        result = checks.check_measurement(600, 20, 20000, checks.thresholds(0, 0, 0, False))
        self.assertEqual(result["performance"], "unqualified")
        for limits, reason in (((40, 100000, 10, True), "throughput"),
                               ((20, 19999, 10, True), "memory"),
                               ((20, 100000, 21, True), "too_short")):
            with self.assertRaisesRegex(ValueError, reason):
                checks.check_measurement(600, 20, 20000, limits)


class BrowserTests(unittest.TestCase):
    def setUp(self):
        self.run_id = "fresh-private-run"
        self.measurement = {"run_id": self.run_id, "exit_status": 0, "timed_out": False,
                            "lingering_descendants": False, "peak_rss_kib": 200000, "elapsed_s": 26}
        self.log = ("msm_drv_video_rs: publish surface=2 cap_idx=Some(1)\n" * 750 +
                    "summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0 "
                    "power-cycles=0 vb2-warns=0 other-session=0 other-system=0 kernel-bugs=0\n")
        names = ["playing", "before_seek", "seek_requested", "seeked", "after_seek", "finished"]
        self.events = [dict(event=name, run_id=self.run_id, time=t, elapsed=e,
                            total=n, dropped=0, ready=4)
                       for name, t, e, n in zip(names, (0, 5, 5, 10, 12, 20),
                                               (0, 5, 5, 5.1, 7.1, 25), (0, 150, 150, 150, 210, 750))]
        self.events[2]["target"] = 10
        self.limits = checks.thresholds(25, 300000, 20, True)

    def check(self):
        return checks.check_browser(self.log, self.events, self.measurement, self.limits, 0.01, self.run_id)

    def test_complete_hardware_playback_real_seek_and_clean_shutdown_pass(self):
        self.assertEqual(self.check()["performance"], "qualified")

    def test_driver_version_only_cannot_pass(self):
        self.log = "msm_drv_video_rs 0.1 loaded\n"
        with self.assertRaisesRegex(ValueError, "insufficient_hardware"):
            self.check()

    def test_software_fallback_and_driver_error_fail_despite_loaded_driver(self):
        original = self.log
        for line in ("IsHardwareAccelerated=0", "IsHardwareAccelerated=false",
                     "HW decoding is slow, switching back to SW decode", "Falling back to software",
                     "vaSyncSurface: internal decoding error", "vaEndPicture failed",
                     "msm_drv_video_rs: vaSyncSurface timed out", "GPU process exiting"):
            self.log = original + line
            with self.subTest(line=line), self.assertRaisesRegex(ValueError, "decoder_error_or_software"):
                self.check()

    def test_killed_browser_and_lingering_children_cannot_prove_teardown(self):
        for key, value in (("exit_status", 124), ("timed_out", True), ("lingering_descendants", True)):
            original = self.measurement.copy()
            self.measurement[key] = value
            with self.assertRaisesRegex(ValueError, "did_not_exit_cleanly"):
                self.check()
            self.measurement = original

    def test_stale_telemetry_and_measurements_fail(self):
        self.events[0]["run_id"] = "last-week"
        with self.assertRaisesRegex(ValueError, "stale_or_unbound_browser_telemetry"):
            self.check()
        self.events[0]["run_id"] = self.run_id
        self.measurement["run_id"] = "last-week"
        with self.assertRaisesRegex(ValueError, "stale_or_unbound_browser_measurement"):
            self.check()

    def test_incomplete_playback_and_failed_seek_fail(self):
        original = copy.deepcopy(self.events)
        for field, value, reason in (("time", 5, "seek_did_not_reach"),
                                     ("ready", 2, "not_ready")):
            self.events[3][field] = value
            with self.assertRaisesRegex(ValueError, reason):
                self.check()
            self.events = copy.deepcopy(original)
        self.events[4]["total"] = 155
        with self.assertRaisesRegex(ValueError, "insufficient_playback"):
            self.check()
        self.events = original[:-1]
        with self.assertRaisesRegex(ValueError, "missing_playback"):
            self.check()

    def test_partial_driver_frames_cannot_certify_whole_playback(self):
        self.log = self.log.replace("publish surface=2", "publish surface=none", 700)
        with self.assertRaisesRegex(ValueError, "partial_hardware"):
            self.check()

    def test_counter_types_nonfinite_and_decreasing_drops_fail(self):
        for value in ("750", True, float("nan"), float("inf")):
            self.events[-1]["total"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                self.check()
        self.events[-1]["total"] = 750
        self.events[1]["dropped"] = 1
        with self.assertRaisesRegex(ValueError, "inconsistent_browser_counters"):
            self.check()

    def test_kernel_fault_and_frame_drops_fail(self):
        self.log += "power-cycles=1"
        with self.assertRaisesRegex(ValueError, "kernel_or_firmware"):
            self.check()
        self.log = self.log.removesuffix("power-cycles=1")
        self.events[-1]["dropped"] = 20
        with self.assertRaisesRegex(ValueError, "dropped_frame_budget"):
            self.check()

    def test_incomplete_kernel_observation_and_impossible_elapsed_fail(self):
        original = self.log
        self.log = self.log.split("power-cycles")[0]
        with self.assertRaisesRegex(ValueError, "incomplete_clean_kernel"):
            self.check()
        self.log = original
        self.measurement["elapsed_s"] = 2
        with self.assertRaisesRegex(ValueError, "telemetry_exceeds_observed"):
            self.check()

    def test_throughput_counts_presented_frames_not_dropped_frames(self):
        self.events[-1]["dropped"] = 7
        self.limits = checks.thresholds(30, 300000, 20, True)
        with self.assertRaisesRegex(ValueError, "throughput_below_requirement"):
            self.check()


class HarnessTests(unittest.TestCase):
    def test_signal_permission_denial_preserves_bounded_failure_evidence(self):
        with tempfile.TemporaryDirectory() as tmp:
            measurement = Path(tmp) / "measurement.json"
            process = mock.Mock(pid=12345)
            process.poll.return_value = None
            with mock.patch.object(sys, "argv", ["monitor", "--seconds", "1", "--output",
                                                 str(measurement), "--", "private-browser"]), \
                 mock.patch.object(monitor.subprocess, "Popen", return_value=process), \
                 mock.patch.object(monitor, "session_rss", return_value=(1000, [12345])), \
                 mock.patch.object(monitor.os, "kill", side_effect=PermissionError(1, "denied")), \
                 mock.patch.object(monitor.time, "monotonic", side_effect=itertools.count()), \
                 mock.patch.object(monitor.time, "sleep"), \
                 mock.patch.object(monitor.signal, "signal"):
                self.assertEqual(monitor.main(), 124)
            process.wait.assert_not_called()
            report = json.loads(measurement.read_text())
            self.assertTrue(report["timed_out"])
            self.assertTrue(report["lingering_descendants"])
            self.assertEqual(report["unresolved_pids"], [12345])
            self.assertEqual([item["signal"] for item in report["signal_denied"]], [15, 9])
            self.assertLess(report["elapsed_s"], 20)

    def test_process_monitor_measures_descendant_memory_and_cleans_timeout(self):
        with tempfile.TemporaryDirectory() as tmp:
            measurement = Path(tmp) / "measurement.json"
            command = [sys.executable, str(ROOT / "tools/measure-process-tree.py"),
                       "--seconds", "1", "--output", str(measurement), "--run-id", "private-run", "--",
                       sys.executable, "-c", "import subprocess,time; subprocess.Popen(['sleep','20']); time.sleep(20)"]
            result = subprocess.run(command, capture_output=True, text=True, timeout=12)
            self.assertEqual(result.returncode, 124, result.stderr)
            report = json.loads(measurement.read_text())
            self.assertGreater(report["peak_rss_kib"], 0)
            self.assertTrue(report["timed_out"])
            self.assertEqual(report["run_id"], "private-run")

    def test_process_monitor_detects_child_that_changes_session_and_outlives_parent(self):
        with tempfile.TemporaryDirectory() as tmp:
            measurement = Path(tmp) / "measurement.json"
            result = subprocess.run([
                sys.executable, str(ROOT / "tools/measure-process-tree.py"), "--seconds", "10",
                "--output", str(measurement), "--", sys.executable, "-c",
                "import subprocess,time; subprocess.Popen(['sleep','20'], start_new_session=True); time.sleep(0.6)",
            ], capture_output=True, text=True, timeout=12)
            self.assertEqual(result.returncode, 1, result.stderr)
            report = json.loads(measurement.read_text())
            self.assertTrue(report["lingering_descendants"])
            self.assertGreater(report["peak_rss_kib"], 0)

    def test_strict_shell_requires_real_budgets_before_launch(self):
        for script in ("verify-4k-decode.sh", "verify-browser-vaapi.sh"):
            result = subprocess.run(["bash", str(ROOT / "tools" / script)],
                                    env={**os.environ, "V4L2_VA_STRICT": "1"},
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertIn("positive_deployment_thresholds_required", result.stdout)

    def test_wrong_driver_fails_before_launch(self):
        for script in ("verify-4k-decode.sh", "verify-browser-vaapi.sh"):
            result = subprocess.run(["bash", str(ROOT / "tools" / script)],
                                    env={**os.environ, "LIBVA_DRIVER_NAME": "other"},
                                    capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertIn("incorrect_driver", result.stdout)


if __name__ == "__main__":
    unittest.main()
