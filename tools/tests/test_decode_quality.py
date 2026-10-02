#!/usr/bin/env python3
"""Host regressions for complete playback and kernel-fault acceptance gates."""
import os
import sys
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('frame_repeats', ROOT / 'tools/compare-frame-repeats.py')
comparator = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparator)


class FrameParityTests(unittest.TestCase):
    def compare(self, reference, actual, frames=2, repeats=2):
        with tempfile.TemporaryDirectory() as directory:
            ref, got = Path(directory) / 'ref', Path(directory) / 'got'
            ref.write_text('#tb 0: 1/30\n' + reference)
            got.write_text('#tb 0: 1/30\n' + actual)
            return comparator.compare(ref, got, frames, repeats)

    reference = '0,0,0,1,384,aaa\n0,1,1,1,384,bbb\n'
    repeated = reference + '0,2,2,1,384,aaa\n0,3,3,1,384,bbb\n'

    def test_complete_repeated_frames_and_timestamps(self):
        self.assertEqual(self.compare(self.reference, self.repeated), 4)

    def test_short_native_reference_fails(self):
        with self.assertRaisesRegex(ValueError, 'short reference'):
            self.compare(self.reference.splitlines()[0] + '\n', self.repeated)

    def test_short_actual_fails_even_when_prefix_matches(self):
        with self.assertRaisesRegex(ValueError, 'actual frame count'):
            self.compare(self.reference, self.reference)

    def test_wrong_pixels_in_second_loop_fail(self):
        with self.assertRaisesRegex(ValueError, 'index=3'):
            self.compare(self.reference, self.repeated.replace('0,3,3,1,384,bbb', '0,3,3,1,384,aaa'))

    def test_timestamp_reset_fails(self):
        with self.assertRaisesRegex(ValueError, 'index=2'):
            self.compare(self.reference, self.reference * 2)

    def test_extra_frame_fails(self):
        with self.assertRaisesRegex(ValueError, 'actual frame count'):
            self.compare(self.reference, self.repeated + '0,4,4,1,384,aaa\n')

    def test_malformed_record_fails(self):
        with self.assertRaisesRegex(ValueError, 'malformed'):
            self.compare(self.reference, '0,0,aaa\n')


class KernelClassificationTests(unittest.TestCase):
    def classify(self, text):
        return subprocess.run(['bash', str(ROOT / 'tools/capture-iris-kernel-log.sh'), '--classify'],
                              input=text, text=True, capture_output=True, timeout=10)

    def test_clean_window_passes(self):
        result = self.classify('qcom-iris: video hw is power on\n')
        self.assertEqual(result.returncode, 0)
        self.assertIn('kernel-bugs=0', result.stdout)

    def test_real_4k_ubsan_signature_fails(self):
        result = self.classify('UBSAN: array-index-out-of-bounds in iris_buffer.c:869:26\n'
                               'UBSAN: array-index-out-of-bounds in iris_buffer.c:870:28\n')
        self.assertEqual(result.returncode, 1)
        self.assertIn('kernel-bugs=2', result.stdout)
        self.assertNotIn('(clean)', result.stdout)

    def test_other_kernel_memory_bugs_fail(self):
        for text in ('BUG: KASAN: use-after-free', 'WARNING: CPU: 0 at iris_buffer.c:123'):
            with self.subTest(text=text):
                self.assertEqual(self.classify(text).returncode, 1)

    def test_firmware_abort_fails(self):
        result = self.classify('qcom-iris: session error received 0x4000003\n')
        self.assertEqual(result.returncode, 1)
        self.assertIn('session-fatal(0x4000003)=1', result.stdout)

    def test_vb2_warning_remains_classified(self):
        result = self.classify('WARNING: CPU: 0 at vb2_start_streaming\n')
        self.assertEqual(result.returncode, 1)
        self.assertIn('vb2-warns=1', result.stdout)


class WrappedKernelTests(unittest.TestCase):
    def run_window(self, message, command_status=0, journal_status=0):
        with tempfile.TemporaryDirectory() as directory:
            fakebin = Path(directory)
            journal = fakebin / 'journalctl'
            journal.write_text('#!' + sys.executable + "\n" +
                               "import os,sys\n" +
                               "if '-n0' in sys.argv:\n" +
                               " if '--show-cursor' in sys.argv: print('-- cursor: test')\n" +
                               "else:\n" +
                               " print(os.environ['JOURNAL_MESSAGE'])\n" +
                               " sys.exit(int(os.environ['JOURNAL_STATUS']))\n")
            journal.chmod(0o755)
            return subprocess.run([
                'bash', str(ROOT / 'tools/capture-iris-kernel-log.sh'), '--',
                sys.executable, '-c', f'import sys; sys.exit({command_status})',
            ], env={**os.environ, 'PATH': str(fakebin) + os.pathsep + os.environ['PATH'],
                    'JOURNAL_MESSAGE': message, 'JOURNAL_STATUS': str(journal_status)},
                capture_output=True, text=True, timeout=10)

    def test_ubsan_fails_a_successful_decode(self):
        result = self.run_window('UBSAN: array-index-out-of-bounds in iris_buffer.c:869:26')
        self.assertEqual(result.returncode, 1)
        self.assertIn('kernel-bugs=1', result.stdout)

    def test_warning_cpu_header_survives_noise_filter(self):
        result = self.run_window('WARNING: CPU: 0 at iris_buffer.c:100')
        self.assertEqual(result.returncode, 1)
        self.assertIn('kernel-bugs=1', result.stdout)

    def test_original_command_failure_is_preserved(self):
        self.assertEqual(self.run_window('', command_status=9).returncode, 9)

    def test_missing_journal_window_cannot_pass(self):
        result = self.run_window('', journal_status=1)
        self.assertEqual(result.returncode, 77)
        self.assertNotIn('(clean)', result.stdout)

    def test_empty_observed_window_passes(self):
        self.assertEqual(self.run_window('').returncode, 0)


class ChurnAcceptanceTests(unittest.TestCase):
    def run_churn(self, reference_frames=3, driver_status=0):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            fakebin = base / 'bin'
            fakebin.mkdir()
            programs = {
                'ffprobe': "print('3')\n",
                'ffmpeg': "import os,pathlib,sys\n" +
                          "hardware = '-hwaccel' in sys.argv\n" +
                          "count = 3 if hardware else int(os.environ['REF_FRAMES'])\n" +
                          "pathlib.Path(sys.argv[-1]).write_text('#tb 0: 1/30\\n' + ''.join(f'0,{i},{i},1,384,aaa\\n' for i in range(count)))\n" +
                          "sys.exit(int(os.environ['DRIVER_STATUS']) if hardware else 0)\n",
                'mpv': "raise SystemExit('unexpected: acceptance should stop before playback')\n",
            }
            for name, body in programs.items():
                executable = fakebin / name
                executable.write_text('#!' + sys.executable + '\n' + body)
                executable.chmod(0o755)
            return subprocess.run(['bash', str(ROOT / 'tools/verify-session-churn.sh'), str(base / 'driver')],
                env={**os.environ, 'PATH': str(fakebin) + os.pathsep + os.environ['PATH'],
                     'REF_FRAMES': str(reference_frames), 'DRIVER_STATUS': str(driver_status),
                     'V4L2_VA_CHURN_DIR': str(base / 'logs'), 'V4L2_VA_CHURN_CUT_FRAMES': '1'},
                text=True, capture_output=True, timeout=10)

    def test_short_reference_fails_before_churn(self):
        result = self.run_churn(reference_frames=1)
        self.assertEqual(result.returncode, 1)
        self.assertIn('short reference', result.stderr)
        self.assertNotIn('unexpected:', result.stderr)

    def test_failed_decode_cannot_pass_with_complete_output(self):
        result = self.run_churn(driver_status=9)
        self.assertEqual(result.returncode, 1)
        self.assertIn('session-churn: fail stopped_before_next_session status=9 expected=0', result.stdout)
        self.assertNotIn('unexpected:', result.stderr)


if __name__ == '__main__':
    unittest.main()
