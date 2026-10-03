"""Kernel faults and incomplete windows cannot be followed by a new session."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
COUNTERS = ['session-fatal(0x4000003)', 'system-fatal(0x5000003)', 'power-cycles',
            'vb2-warns', 'other-session', 'other-system', 'kernel-bugs']
CLEAN = 'summary: ' + '  '.join(name + '=0' for name in COUNTERS)


class HardwareSessionTests(unittest.TestCase):
    def checked(self, summary, status=0, conditional=False, module_state='Live',
                boot_kernel='Linux mock clean boot', journal_status=0):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            tools = root / 'tools'
            tools.mkdir()
            for name in ('hardware-session.sh', 'check-playback-performance.py'):
                shutil.copy2(ROOT / 'tools' / name, tools / name)
            modules = root / 'modules'
            modules.write_text('qcom_iris 237568 0 - ' + module_state + ' 0x0\n' if module_state else '')
            helper = tools / 'hardware-session.sh'
            helper.write_text(helper.read_text().replace('/proc/modules', str(modules)))
            with helper.open('a') as stream:
                stream.write('\njournalctl() { printf "%s\\n" "$BOOT_KERNEL"; return "$JOURNAL_STATUS"; }\n')
            wrapper = tools / 'capture-iris-kernel-log.sh'
            wrapper.write_text('#!' + sys.executable + '\nimport os,sys\n'
                               "print('hardware_command_started')\n"
                               "print(os.environ['SUMMARY'])\nraise SystemExit(int(os.environ['STATUS']))\n")
            wrapper.chmod(0o755)
            call = 'run_kernel_checked "$repo_root/session.log" unused'
            # Optional diagnostics and errexit-disabled functions must still
            # terminate the parent shell after unsafe kernel evidence.
            if conditional:
                call = 'if ' + call + '; then :; else :; fi'
            return subprocess.run(['bash', '-c', 'set +e\nrepo_root="$1"\n'
                                   'source "$repo_root/tools/hardware-session.sh"\n' + call +
                                   '\nstatus=$?\necho next_session\nexit "$status"', '_', str(root)],
                                  env={**os.environ, 'SUMMARY': summary, 'STATUS': str(status),
                                       'BOOT_KERNEL': boot_kernel, 'JOURNAL_STATUS': str(journal_status)},
                                  capture_output=True, text=True, timeout=10)

    def test_clean_windows_preserve_command_status(self):
        for status in (0, 9, 77, 124, 137, 218):
            result = self.checked(CLEAN, status)
            self.assertEqual(result.returncode, status, result.stdout + result.stderr)
            self.assertIn('next_session', result.stdout)

    def test_every_fault_stops_even_in_an_optional_call(self):
        for counter in COUNTERS:
            for conditional in (False, True):
                with self.subTest(counter=counter, conditional=conditional):
                    result = self.checked(CLEAN.replace(counter + '=0', counter + '=1'),
                                          status=124, conditional=conditional)
                    self.assertEqual(result.returncode, 1)
                    self.assertNotIn('next_session', result.stdout)

    def test_missing_partial_or_malformed_evidence_stops(self):
        for summary in ('', 'summary: session-fatal(0x4000003)=0',
                        CLEAN.replace('kernel-bugs=0', 'kernel-bugs=unknown')):
            result = self.checked(summary, conditional=True)
            self.assertEqual(result.returncode, 1)
            self.assertNotIn('next_session', result.stdout)

    def test_loading_or_unloading_module_prevents_the_first_hardware_command(self):
        for state in ('Loading', 'Unloading'):
            result = self.checked(CLEAN, module_state=state, conditional=True)
            self.assertEqual(result.returncode, 1)
            self.assertIn('iris_module_transition', result.stdout)
            self.assertNotIn('next_session', result.stdout)
            self.assertNotIn('kernel_window', result.stdout)

    def test_hosts_without_an_iris_module_can_exercise_the_mock(self):
        self.assertEqual(self.checked(CLEAN, module_state='').returncode, 0)

    def test_fault_before_a_fresh_clean_window_prevents_decoder_open(self):
        for fault in ('session error received 0x4000003',
                      'received system error of type 0x5000002',
                      'video hw is power on',
                      'arm-smmu: Unhandled context fault',
                      'UBSAN: array-index-out-of-bounds', 'WARNING: vb2'):
            for conditional in (False, True):
                with self.subTest(fault=fault, conditional=conditional):
                    result = self.checked(CLEAN, boot_kernel=fault, conditional=conditional)
                    self.assertEqual(result.returncode, 1)
                    self.assertIn('prior_boot_kernel_or_firmware_fault', result.stdout)
                    self.assertNotIn('hardware_command_started', result.stdout)
                    self.assertNotIn('next_session', result.stdout)

    def test_missing_boot_journal_cannot_start_decoder(self):
        for kernel, status in (('', 0), ('Linux mock clean boot', 1)):
            result = self.checked(CLEAN, boot_kernel=kernel, journal_status=status,
                                  conditional=True)
            self.assertEqual(result.returncode, 1)
            self.assertNotIn('hardware_command_started', result.stdout)
            self.assertNotIn('next_session', result.stdout)


class LongFrameCountTests(unittest.TestCase):
    def count(self, counts):
        text = (ROOT / 'tools/verify-long-playback.sh').read_text()
        statement = 'expected_frames="$(ffprobe' + text.split('expected_frames="$(ffprobe', 1)[1].split('\nif [[', 1)[0]
        with tempfile.TemporaryDirectory() as tmp:
            probe = Path(tmp) / 'ffprobe'
            probe.write_text('#!' + sys.executable + '\nimport os\n'
                             "for i in range(100000): print(os.environ['COUNTS'].split(',')[i % len(os.environ['COUNTS'].split(','))])\n")
            probe.chmod(0o755)
            return subprocess.run(['bash', '-c', 'set -euo pipefail\nplaylist=unused\n' + statement +
                                   '\necho "count=$expected_frames"'],
                                  env={**os.environ, 'PATH': tmp + os.pathsep + os.environ['PATH'],
                                       'COUNTS': counts}, capture_output=True, text=True, timeout=10)

    def test_frame_count_consumes_duplicate_program_output_without_sigpipe(self):
        result = self.count('3600')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('count=3600', result.stdout)

    def test_conflicting_program_counts_cannot_certify_the_playlist(self):
        result = self.count('3600,30')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), 'count=')


if __name__ == '__main__':
    unittest.main()
