"""Activation failures must preserve the installed-module rollback path."""
import importlib.util
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('iris_activation', ROOT / 'tools/activate-iris-candidate.py')
IRIS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(IRIS)


class IrisActivationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.candidate = self.base / 'candidate.ko'
        self.evidence = {'original_build_id': 'old', 'candidate_build_id': 'new'}

    def test_sysfs_note_is_decoded_and_malformed_notes_fail(self):
        notes = self.base / 'notes'
        notes.mkdir()
        note = notes / '.note.gnu.build-id'
        note.write_bytes(struct.pack('<III', 4, 3, 3) + b'GNU\0' + bytes.fromhex('123456'))
        with patch.object(IRIS, 'MODULE', self.base):
            self.assertEqual(IRIS.loaded_build_id(), '123456')
            note.write_bytes(struct.pack('<III', 4, 20, 3) + b'GNU\0')
            with self.assertRaisesRegex(ValueError, 'invalid_loaded'):
                IRIS.loaded_build_id()

    def test_successful_activation_checks_the_loaded_build_id(self):
        with patch.object(IRIS, 'run') as run, patch.object(IRIS, 'loaded_build_id', side_effect=['old', 'new']):
            IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual([call.args[0] for call in run.call_args_list],
                         [['modprobe', '-r', 'qcom_iris'], ['insmod', str(self.candidate)]])

    def test_already_loaded_target_does_not_unload(self):
        with patch.object(IRIS, 'run') as run, patch.object(IRIS, 'loaded_build_id', return_value='new'):
            IRIS.replace_module(self.candidate, self.evidence)
        run.assert_not_called()

    def test_known_deadlocking_builds_never_reach_unload(self):
        for build_id in IRIS.UNSAFE_UNLOAD_BUILD_IDS:
            with self.subTest(build_id=build_id), patch.object(IRIS, 'run') as run, \
                 patch.object(IRIS, 'loaded_build_id', return_value=build_id):
                with self.assertRaisesRegex(ValueError, 'known_teardown_deadlock'):
                    IRIS.replace_module(self.candidate, self.evidence)
                run.assert_not_called()

    def test_busy_unload_does_not_insert_or_attempt_force(self):
        with patch.object(IRIS, 'run', side_effect=OSError('busy')) as run, \
             patch.object(IRIS, 'loaded_build_id', return_value='old'):
            with self.assertRaisesRegex(OSError, 'busy'):
                IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual(run.call_count, 1)

    def test_failed_insertion_restores_and_verifies_original(self):
        with patch.object(IRIS, 'run', side_effect=[None, OSError('load failed'), None]) as run, \
             patch.object(IRIS, 'MODULE') as module, \
             patch.object(IRIS, 'loaded_build_id', side_effect=['old', 'old']):
            module.exists.return_value = False
            with self.assertRaisesRegex(OSError, 'load failed'):
                IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual(run.call_args_list[-1].args[0], ['modprobe', 'qcom_iris'])

    def test_wrong_loaded_identity_is_removed_and_original_restored(self):
        with patch.object(IRIS, 'run') as run, patch.object(IRIS, 'MODULE') as module, \
             patch.object(IRIS, 'loaded_build_id', side_effect=['old', 'wrong', 'wrong', 'old']):
            module.exists.return_value = True
            with self.assertRaisesRegex(ValueError, 'does_not_match_target'):
                IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual([call.args[0] for call in run.call_args_list][-2:],
                         [['modprobe', '-r', 'qcom_iris'], ['modprobe', 'qcom_iris']])

    def test_explicit_rollback_loads_installed_module(self):
        with patch.object(IRIS, 'run') as run, patch.object(IRIS, 'loaded_build_id', side_effect=['new', 'old']):
            IRIS.replace_module(self.candidate, self.evidence, rollback=True)
        self.assertEqual([call.args[0] for call in run.call_args_list],
                         [['modprobe', '-r', 'qcom_iris'], ['modprobe', 'qcom_iris']])

    def test_cold_boot_absent_module_is_inserted_without_unload(self):
        with patch.object(IRIS, 'run') as run, patch.object(IRIS, 'loaded_build_id', side_effect=[None, 'new']):
            IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual([call.args[0] for call in run.call_args_list], [['insmod', str(self.candidate)]])

    def test_cold_boot_requirement_rejects_even_an_already_loaded_candidate(self):
        for build_id in ('old', 'new'):
            with self.subTest(build_id=build_id), patch.object(IRIS, 'run') as run, \
                 patch.object(IRIS, 'loaded_build_id', return_value=build_id):
                with self.assertRaisesRegex(ValueError, 'cold_boot_requires_iris_absent'):
                    IRIS.replace_module(self.candidate, self.evidence, require_absent=True)
                run.assert_not_called()

    def test_cold_boot_loads_dependencies_then_rechecks_absence(self):
        evidence = dict(self.evidence, dependencies=['videodev', 'v4l2-mem2mem'])
        with patch.object(IRIS, 'run') as run, \
             patch.object(IRIS, 'loaded_build_id', side_effect=[None, None, 'new']):
            IRIS.replace_module(self.candidate, evidence, require_absent=True)
        self.assertEqual([call.args[0] for call in run.call_args_list],
                         [['modprobe', '--all', 'videodev', 'v4l2-mem2mem'], ['insmod', str(self.candidate)]])
        with patch.object(IRIS, 'run') as run, \
             patch.object(IRIS, 'loaded_build_id', side_effect=[None, 'old']):
            with self.assertRaisesRegex(ValueError, 'cold_boot_requires_iris_absent'):
                IRIS.replace_module(self.candidate, evidence, require_absent=True)
        self.assertEqual(run.call_count, 1)

    def test_failed_dependency_load_does_not_insert_or_restore_original(self):
        evidence = dict(self.evidence, dependencies=['videodev'])
        with patch.object(IRIS, 'run', side_effect=OSError('missing dependency')) as run, \
             patch.object(IRIS, 'loaded_build_id', return_value=None):
            with self.assertRaisesRegex(OSError, 'missing dependency'):
                IRIS.replace_module(self.candidate, evidence, require_absent=True)
        self.assertEqual([call.args[0] for call in run.call_args_list], [['modprobe', '--all', 'videodev']])

    def test_unfinished_insert_never_attempts_rollback(self):
        error = IRIS.KernelCommandUnfinished(['insmod', 'candidate'], 123)
        with patch.object(IRIS, 'run', side_effect=error) as run, \
             patch.object(IRIS, 'loaded_build_id', return_value=None):
            with self.assertRaises(IRIS.KernelCommandUnfinished):
                IRIS.replace_module(self.candidate, self.evidence)
        self.assertEqual(run.call_count, 1)

    def test_d_state_timeout_returns_pid_without_unbounded_wait(self):
        child = Mock(pid=123)
        child.communicate.side_effect = [subprocess.TimeoutExpired(['modprobe'], 30),
                                         subprocess.TimeoutExpired(['modprobe'], 2)]
        with patch.object(IRIS.subprocess, 'Popen', return_value=child):
            with self.assertRaises(IRIS.KernelCommandUnfinished) as error:
                IRIS.run(['modprobe', '-r', 'qcom_iris'])
        self.assertEqual(error.exception.pid, 123)
        child.kill.assert_called_once()
        child.wait.assert_not_called()
        self.assertEqual([call.kwargs['timeout'] for call in child.communicate.call_args_list], [30, 2])

    def test_ordinary_timeout_is_reaped_and_propagated(self):
        child = Mock()
        child.communicate.side_effect = [subprocess.TimeoutExpired(['test'], 30), ('', '')]
        with patch.object(IRIS.subprocess, 'Popen', return_value=child):
            with self.assertRaises(subprocess.TimeoutExpired):
                IRIS.run(['test'])
        child.kill.assert_called_once()

    def test_evidence_is_visible_before_mutation_and_cannot_be_reused(self):
        path = self.base / 'evidence.json'
        journal = IRIS.EvidenceJournal(path, action='activate')
        journal.update('unload_started')
        self.assertEqual(json.loads(path.read_text())['phase'], 'unload_started')
        with self.assertRaises(FileExistsError):
            IRIS.EvidenceJournal(path, action='activate')
        journal.update('failure', status='unresolved', unfinished_pid=123)
        record = json.loads(path.read_text())
        self.assertEqual(record['status'], 'unresolved')
        self.assertEqual(record['unfinished_pid'], 123)
        self.assertEqual([step['phase'] for step in record['phases']], ['unload_started', 'failure'])
        self.assertEqual(path.stat().st_mode & 0o777, 0o644)

    def test_faulted_boot_is_journaled_before_any_module_command(self):
        path = self.base / 'evidence.json'
        boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
        argv = ['activate', str(self.candidate), '--sha256', 'unused', '--evidence', str(path),
                '--exclude-boot-id', boot, '--exclude-boot-id', 'another-older-faulted-boot']
        with patch.object(sys, 'argv', argv), patch.object(IRIS, 'run') as run:
            self.assertEqual(IRIS.main(), 1)
        run.assert_not_called()
        record = json.loads(path.read_text())
        self.assertEqual(record['status'], 'failed')
        self.assertEqual(record['error'], 'faulted_boot_requires_restart')

    def test_unfinished_kernel_operation_is_recorded_as_unresolved(self):
        self.candidate.write_bytes(b'candidate')
        expected = hashlib.sha256(self.candidate.read_bytes()).hexdigest()
        path = self.base / 'evidence.json'
        lease = self.base / 'lease'
        lease.touch()
        evidence = dict(self.evidence, loaded_build_id=None)
        def stuck(candidate, reviewed, **kwargs):
            kwargs['progress']('insertion_started')
            record = json.loads(path.read_text())
            self.assertEqual(record['phase'], 'insertion_started')
            self.assertEqual(record['status'], 'pending')
            raise IRIS.KernelCommandUnfinished(['insmod', str(candidate)], 123)
        argv = ['activate', str(self.candidate), '--sha256', expected, '--evidence', str(path), '--activate']
        with patch.object(sys, 'argv', argv), patch.object(IRIS.os, 'geteuid', return_value=0), \
             patch.object(IRIS, 'LOCK', lease), patch.object(IRIS, 'run', return_value=str(self.candidate)), \
             patch.object(IRIS, 'preflight', return_value=evidence), patch.object(IRIS, 'replace_module', side_effect=stuck):
            self.assertEqual(IRIS.main(), 1)
        record = json.loads(path.read_text())
        self.assertEqual(record['status'], 'unresolved')
        self.assertEqual(record['unfinished_pid'], 123)
        self.assertEqual([step['phase'] for step in record['phases']],
                         ['preflight_started', 'preflight_verified', 'insertion_started', 'failure'])

    def test_added_imports_require_matching_complete_kernel_table(self):
        self.candidate.write_bytes(b'candidate')
        installed = self.base / 'original.ko'
        symvers = self.base / 'Module.symvers'
        expected = hashlib.sha256(self.candidate.read_bytes()).hexdigest()
        def commands(command):
            if command[0] == 'modinfo':
                return '' if command[2] == 'depends' else 'test-kernel SMP modversions aarch64'
            return ('0x123 shared' if command[-1] == str(installed) else '0x123 shared\n0x456 added')
        with patch.object(IRIS.platform, 'release', return_value='test-kernel'), \
             patch.object(IRIS, 'run', side_effect=commands), \
             patch.object(IRIS, 'file_build_id', side_effect=lambda path: 'new' if path == self.candidate else 'old'):
            for table in ('', '0x999 added vmlinux EXPORT_SYMBOL_GPL\n',
                          '0x999 shared vmlinux EXPORT_SYMBOL_GPL\n0x456 added vmlinux EXPORT_SYMBOL_GPL\n'):
                symvers.write_text(table)
                with self.subTest(table=table), self.assertRaisesRegex(ValueError, 'imported_symbol_crc_mismatch'):
                    IRIS.review_abi(self.candidate, expected, installed, symvers)
            symvers.write_text('0x123 shared vmlinux EXPORT_SYMBOL_GPL\n0x456 added vmlinux EXPORT_SYMBOL_GPL\n')
            result = IRIS.review_abi(self.candidate, expected, installed, symvers)
            self.assertEqual(result['added_imports'], ['added'])
            self.assertEqual(result['imported_symbol_crcs'], 2)

    def test_changed_candidate_fails_before_module_commands(self):
        self.candidate.write_bytes(b'changed module')
        with patch.object(IRIS, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'candidate_hash_changed'):
                IRIS.preflight(self.candidate, 'old-hash', self.candidate)
        run.assert_not_called()

    def test_existing_lease_is_preserved_without_create_or_truncate(self):
        lease = self.base / 'lease'
        lease.write_text('preserved')
        with patch.object(IRIS, 'LOCK', lease), patch.object(IRIS.os, 'open', wraps=IRIS.os.open) as opening:
            with IRIS.open_lease():
                pass
        self.assertEqual(lease.read_text(), 'preserved')
        self.assertFalse(opening.call_args.args[1] & (IRIS.os.O_CREAT | IRIS.os.O_TRUNC))

    def test_root_does_not_create_a_lease_the_decoder_user_cannot_open(self):
        with patch.object(IRIS, 'LOCK', self.base / 'missing'), patch.object(IRIS.os, 'geteuid', return_value=0):
            with self.assertRaisesRegex(ValueError, 'run_check_only_as_decoder_user'):
                IRIS.open_lease()

    def test_symlink_lease_is_rejected(self):
        target = self.base / 'target'
        target.write_text('preserved')
        lease = self.base / 'lease'
        lease.symlink_to(target)
        with patch.object(IRIS, 'LOCK', lease):
            with self.assertRaises(OSError):
                IRIS.open_lease()
        self.assertEqual(target.read_text(), 'preserved')

    def test_imported_crc_mismatch_and_busy_modules_fail_preflight(self):
        self.candidate.write_bytes(b'candidate')
        installed = self.base / 'original.ko'
        (self.base / 'holders').mkdir()
        (self.base / 'refcnt').write_text('0')
        expected = hashlib.sha256(self.candidate.read_bytes()).hexdigest()
        def commands(command):
            if command[0] == 'modinfo':
                return '' if command[2] == 'depends' else 'test-kernel SMP modversions aarch64'
            if command[0] == 'modprobe':
                return ('0x123 symbol' if command[-1] == str(installed) else '0x456 symbol')
            raise AssertionError(command)
        with patch.object(IRIS, 'MODULE', self.base), patch.object(IRIS.platform, 'release', return_value='test-kernel'), \
             patch.object(IRIS, 'run', side_effect=commands):
            with self.assertRaisesRegex(ValueError, 'imported_symbol_crc_mismatch'):
                IRIS.preflight(self.candidate, expected, installed)
        (self.base / 'refcnt').write_text('1')
        with patch.object(IRIS, 'MODULE', self.base), patch.object(IRIS.platform, 'release', return_value='test-kernel'), \
             patch.object(IRIS, 'run', side_effect=lambda command: ('' if command[2] == 'depends' else
                          'test-kernel SMP modversions aarch64') if command[0] == 'modinfo' else '0x123 symbol'), \
             patch.object(IRIS, 'file_build_id', side_effect=['new', 'old']), \
             patch.object(IRIS, 'loaded_build_id', return_value='old'):
            with self.assertRaisesRegex(ValueError, 'iris_module_in_use'):
                IRIS.preflight(self.candidate, expected, installed)


if __name__ == '__main__':
    unittest.main()
