import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('replay4k', TOOLS / 'qualify-av1-4k-replay.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class ReplayTest(unittest.TestCase):
    def test_known_fault_refused_before_inspection_or_lease(self):
        cycle = m.load('cycle_test', 'qualify-iris-module-cycle.py')
        with tempfile.TemporaryDirectory() as tmp:
            boot = Path(tmp) / 'boot'
            for forbidden in cycle.FAULTED:
                boot.write_text(forbidden)
                with patch.object(m, 'BOOT', boot), patch.object(m, 'verify_files') as files:
                    with self.assertRaisesRegex(ValueError, 'faulted boot'):
                        m.identities(Path(tmp), cycle)
                    files.assert_not_called()

    def test_cleanup_rejects_timeout_linger_denial_missing_and_memory(self):
        clean = dict(exit_status=0, timed_out=False, lingering_descendants=False,
                     signal_denied=[], unresolved_pids=[], interrupted=False, peak_rss_kib=512)
        m.validate_cleanup(clean, 0, 512)
        for update in [dict(exit_status=1), dict(timed_out=True), dict(lingering_descendants=True),
                       dict(signal_denied=[7]), dict(unresolved_pids=[7]), dict(interrupted=True),
                       dict(peak_rss_kib=513), dict(peak_rss_kib=None)]:
            with self.assertRaises(ValueError):
                m.validate_cleanup(dict(clean, **update), 0, 512)
        with self.assertRaises(ValueError):
            m.validate_cleanup(clean, 1, 512)

    def test_all_coded_frames_and_exact_format_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = Path(tmp) / 'md5'
            header = '#dimensions 0: 3840x2160\n#hash: MD5\n'
            row = '0, 0, 0, 1, 12441600, ' + 'a' * 32 + '\n'
            p.write_text(header + row * 375)
            self.assertEqual(len(m.checksum_rows(p, True)), 375)
            for content in [header + row * 374, (header + row * 375).replace('12441600', '1'),
                            (header + row * 375).replace('3840x2160', '1920x1080'),
                            header + row * 376]:
                p.write_text(content)
                with self.assertRaises(ValueError):
                    m.checksum_rows(p, True)

    def test_changed_file_and_escaping_manifest_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = Path(tmp); data = r / 'file'; data.write_text('original')
            manifest = r / 'sha256.json'
            manifest.write_text(json.dumps({'file': hashlib.sha256(data.read_bytes()).hexdigest()}))
            m.verify_files(r)
            data.write_text('changed')
            with self.assertRaisesRegex(ValueError, 'changed'):
                m.verify_files(r)
            manifest.write_text(json.dumps({'../file': 'a' * 64}))
            with self.assertRaisesRegex(ValueError, 'escapes'):
                m.verify_files(r)

    def test_wrong_or_missing_worker_lease_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            lease = Path(tmp) / 'lease'; lease.touch()
            helper = types.SimpleNamespace(LOCK=str(lease))
            with self.assertRaises(ValueError):
                m.require_inherited_lease(None, helper)
            with (Path(tmp) / 'wrong').open('w+') as wrong:
                with self.assertRaises(ValueError):
                    m.require_inherited_lease(wrong.fileno(), helper)
            with lease.open('r+') as held, lease.open('r+') as competitor:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                m.require_inherited_lease(held.fileno(), helper)
                with self.assertRaises(BlockingIOError):
                    m.require_inherited_lease(competitor.fileno(), helper)

    def test_stale_or_missing_cold_identity_never_runs(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = Path(tmp)
            identity = {'boot_id': 'new-boot', 'actual_build_ids': {'qcom_iris': 'reviewed'}}
            cycle = types.SimpleNamespace(inspect=lambda helper: {'boot_id': 'new-boot'})
            with patch.object(m, 'identities', return_value=identity):
                with self.assertRaises(FileNotFoundError):
                    m.check(r, cycle, None)
                seal = {'status': 'pass', 'action': 'read_only_cold_verification', 'identity': identity}
                (r / 'cold-identity.json').write_text(json.dumps(seal))
                self.assertEqual(m.check(r, cycle, None)[0], identity)
                seal['identity'] = dict(identity, boot_id='previous-boot')
                (r / 'cold-identity.json').write_text(json.dumps(seal))
                with self.assertRaisesRegex(ValueError, 'current-boot'):
                    m.check(r, cycle, None)

    def test_monitor_passes_reviewed_lease_to_child_and_blocks_competitor(self):
        with tempfile.TemporaryDirectory() as tmp:
            r = Path(tmp); lease = r / 'lease'; lease.touch()
            code = ('import fcntl,os,sys; fd=int(sys.argv[1]); '
                    'fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB); '
                    'other=os.open(sys.argv[2],os.O_RDWR); '
                    '\ntry: fcntl.flock(other,fcntl.LOCK_EX|fcntl.LOCK_NB)'
                    '\nexcept BlockingIOError: sys.exit(0)'
                    '\nelse: sys.exit(9)')
            with lease.open('r+') as held:
                fcntl.flock(held, fcntl.LOCK_EX | fcntl.LOCK_NB)
                # Exercise descriptor propagation through the real shell
                # observer as well, using a host-only fake empty journal.
                journal = r / 'journalctl'
                journal.write_text("#!/bin/sh\nprintf '%s\\n' '-- cursor: host-fixture'\nexit 0\n")
                journal.chmod(0o755)
                command = [str(TOOLS / 'capture-iris-kernel-log.sh'), '--',
                           sys.executable, str(TOOLS / 'measure-process-tree.py'), '--seconds', '5',
                           '--output', str(r / 'result.json'), '--inherit-fd', str(held.fileno()), '--',
                           sys.executable, '-c', code, str(held.fileno()), str(lease)]
                env = dict(os.environ, PATH=str(r) + ':' + os.environ['PATH'])
                result = subprocess.run(command, pass_fds=(held.fileno(),), capture_output=True, timeout=15, env=env)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(b'verdict: no iris firmware errors in this window (clean).', result.stdout)
                m.validate_cleanup(json.loads((r / 'result.json').read_text()), 0, 512 * 1024)


if __name__ == '__main__':
    unittest.main()
