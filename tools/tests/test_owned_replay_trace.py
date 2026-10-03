import fcntl
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

TOOLS = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('owned_trace', TOOLS / 'trace-iris-owned-replay.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


class OwnedTraceTest(unittest.TestCase):
    def packet(self, root):
        trace = root / 'trace'
        trace.mkdir()
        modules = {'qcom_iris': {'selected_module': '/test/iris.ko',
                                'selected_sha256': 'a'*64, 'loaded_build_id': 'b'*40}}
        converted = {'qcom_iris': {'path': '/test/iris.ko', 'sha256': 'a'*64, 'build_id': 'b'*40}}
        (root / 'plan.json').write_text(json.dumps({'kernel': 'test-kernel', 'modules': modules}))
        (trace / 'plan.json').write_text(json.dumps({'kernel': 'test-kernel', 'modules': converted}))
        (trace / 'trace.bt').write_text('BEGIN { printf("IRIS_MEM_READY\\n"); }\n'
                                      'kprobe:test { printf("%llu", nsecs(monotonic)); }\n'
                                      'END { printf("IRIS_MEM_STOPPED\\n"); }\n')
        (trace / 'sha256.json').write_text('{}')
        receipt = dict(status='pass', exit_status=0, error=None, attachments=0,
                       decoder_opens=0, browser_launches=0, live_traceable_inventory_verified=True,
                       kernel='test-kernel', modules=converted, trace_sha256=m.sha(trace/'trace.bt'),
                       packet_manifest_sha256=m.sha(trace/'sha256.json'),
                       command=['/usr/bin/bpftrace', '--mode', 'codegen', str(trace/'trace.bt')])
        (trace / 'privileged-codegen.json').write_text(json.dumps(receipt))
        return trace, receipt

    def test_exact_successful_compile_receipt_and_root_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trace, receipt = self.packet(root)
            with patch.object(m.os, 'geteuid', return_value=0):
                self.assertEqual(m.require_compilation(root), trace)
                for update in [dict(status='fail'), dict(exit_status=1), dict(error='timeout'),
                               dict(attachments=1), dict(decoder_opens=1), dict(browser_launches=1),
                               dict(live_traceable_inventory_verified=False), dict(kernel='changed'),
                               dict(modules={}), dict(trace_sha256='changed'),
                               dict(packet_manifest_sha256='changed'),
                               dict(command=['/usr/bin/bpftrace', '--dry-run'])]:
                    (trace/'privileged-codegen.json').write_text(json.dumps(dict(receipt, **update)))
                    with self.assertRaisesRegex(ValueError, 'exact-packet compile-only'):
                        m.require_compilation(root)
                (trace/'privileged-codegen.json').write_text(json.dumps(receipt))
            with patch.object(m.os, 'geteuid', return_value=1000):
                with self.assertRaisesRegex(ValueError, 'operator root'):
                    m.require_compilation(root)

    def test_bare_boottime_clock_rejected_even_with_matching_receipt(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            trace, receipt = self.packet(root)
            (trace/'trace.bt').write_text((trace/'trace.bt').read_text().replace('nsecs(monotonic)', 'nsecs'))
            receipt['trace_sha256'] = m.sha(trace/'trace.bt')
            (trace/'privileged-codegen.json').write_text(json.dumps(receipt))
            with patch.object(m.os, 'geteuid', return_value=0):
                with self.assertRaisesRegex(ValueError, 'journal-compatible'):
                    m.require_compilation(root)

    def run_fake(self, root, fd, mode, watchdog):
        # Exercise actual readiness, signals, child lease inheritance and files
        # using a userspace process, with no BPF or device access.
        evidence = root/'evidence'
        evidence.mkdir()
        real_popen = subprocess.Popen
        code = '''import fcntl, os, signal, sys, time
fd=int(sys.argv[1]); fcntl.flock(fd,fcntl.LOCK_EX|fcntl.LOCK_NB)
mode=sys.argv[2]
def stop(sig,frame):
 print('IRIS_MEM_STOPPED',flush=True); sys.exit(0)
signal.signal(signal.SIGINT,stop)
if mode=='no-ready': sys.exit(1)
if mode=='error': print('attach diagnostic',file=sys.stderr,flush=True)
os.write(1,b'IRIS_MEM_READY\\nLost 3 events\\n' if mode=='lost' else b'IRIS_MEM_READY\\n')
while True: time.sleep(.01)
'''
        compile(code, 'userspace-tracer-fixture', 'exec')
        def start(command, **kwargs):
            self.assertEqual(command[:5], ['/usr/bin/bpftrace', '-k', '-q', '-B', 'line'])
            return real_popen([sys.executable, '-c', code, str(fd), mode], **kwargs)
        with patch.object(m, 'require_compilation', return_value=root/'trace'):
            return m.Tracer(root, evidence, fd, watchdog), start

    def test_real_process_ready_lease_and_clean_stop(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); trace,_=self.packet(root)
            lease=root/'lease'
            with lease.open('w+') as held, lease.open('r+') as competing:
                fcntl.flock(held, fcntl.LOCK_EX|fcntl.LOCK_NB)
                tracer, start=self.run_fake(root, held.fileno(), 'normal', lambda:None)
                with patch.object(m, 'require_compilation', return_value=trace), patch.object(m.subprocess,'Popen',side_effect=start):
                    with tracer:
                        self.assertTrue(tracer.health())
                        with self.assertRaises(BlockingIOError):
                            fcntl.flock(competing,fcntl.LOCK_EX|fcntl.LOCK_NB)
                    self.assertEqual(tracer.process.returncode,0)
                    self.assertTrue(tracer.output.closed)
                    self.assertTrue(tracer.errors.closed)
                    self.assertEqual(tracer.output_path.read_text(),'IRIS_MEM_READY\nIRIS_MEM_STOPPED\n')

    def test_loss_or_diagnostics_stop_before_decode(self):
        for mode in ['lost','error','no-ready']:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as tmp:
                root=Path(tmp); trace,_=self.packet(root)
                with (root/'lease').open('w+') as lease:
                    tracer,start=self.run_fake(root,lease.fileno(),mode,lambda:None)
                    with patch.object(m,'require_compilation',return_value=trace), patch.object(m.subprocess,'Popen',side_effect=start):
                        with self.assertRaises(ValueError):
                            with tracer:
                                self.fail('decode body must not be reached')
                    self.assertIsNotNone(tracer.process.poll())
                    self.assertTrue(tracer.output.closed)
                    self.assertTrue(tracer.errors.closed)

    def test_kernel_fault_during_replay_stops_tracer_preserves_primary(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); trace,_=self.packet(root)
            with (root/'lease').open('w+') as lease:
                tracer,start=self.run_fake(root,lease.fileno(),'normal',lambda:None)
                with patch.object(m,'require_compilation',return_value=trace), patch.object(m.subprocess,'Popen',side_effect=start):
                    with self.assertRaisesRegex(ValueError,'new kernel fault'):
                        with tracer:
                            def fault(): raise ValueError('new kernel fault')
                            tracer.kernel_watchdog=fault
                            tracer.health()
                self.assertEqual(tracer.process.returncode,0)

    def test_existing_evidence_never_overwritten_and_handles_close(self):
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp); trace,_=self.packet(root)
            evidence=root/'evidence';evidence.mkdir()
            (evidence/'trace-errors.log').write_text('old failure')
            with patch.object(m,'require_compilation',return_value=trace), patch.object(m.subprocess,'Popen') as child:
                tracer=m.Tracer(root,evidence,5,lambda:None)
                with self.assertRaises(FileExistsError): tracer.__enter__()
                self.assertTrue(tracer.output.closed)
                child.assert_not_called()
                self.assertEqual((evidence/'trace-errors.log').read_text(),'old failure')

    def test_full_observation_and_single_session_required(self):
        rows=''.join(f'IRIS_MEM event={kind} ns=1 cpu=0 tid=2 session=3 type=1\n'*375
                     for kind in ['queue','hfi_encoded','output_response'])
        self.assertEqual(m.validate_observations(rows)['queued_inputs'],375)
        for bad in [rows.replace('event=output_response','event=other'),
                    rows.replace('session=3','session=4',1), rows.replace('session=3','',1),
                    rows.replace('type=1','type=2')]:
            with self.assertRaises(ValueError): m.validate_observations(bad)

    def test_actual_replay_worker_stops_owned_child_on_watchdog_fault(self):
        replay_spec=importlib.util.spec_from_file_location('trace_child_replay',TOOLS/'qualify-av1-4k-replay.py')
        replay=importlib.util.module_from_spec(replay_spec)
        replay_spec.loader.exec_module(replay)
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);(root/'evidence').mkdir()
            identity={'boot_id':'host-fixture','actual_build_ids':{'qcom_iris':'reviewed'}}
            (root/'evidence/before.json').write_text(json.dumps(identity))
            boot=root/'boot';boot.write_text('host-fixture')
            lease_path=root/'lease'
            helper=types.SimpleNamespace(LOCK=str(lease_path))
            cycle=types.SimpleNamespace(journal=lambda *a:'{"__CURSOR":"host"}',require_clean_messages=lambda s:None)
            real_popen=subprocess.Popen
            children=[]
            count=0
            def launch(command,**kwargs):
                code='import fcntl,sys,time; fcntl.flock(int(sys.argv[1]),fcntl.LOCK_EX|fcntl.LOCK_NB); print("CLIENT_LEASE_HELD",flush=True); time.sleep(60)'
                child=real_popen([sys.executable,'-c',code,str(held.fileno())],**kwargs)
                children.append(child)
                return child
            def fault():
                nonlocal count
                count+=1
                if count>=3: raise ValueError('new kernel fault; stop client')
            with lease_path.open('w+') as held:
                fcntl.flock(held,fcntl.LOCK_EX|fcntl.LOCK_NB)
                with patch.object(replay,'BOOT',boot),patch.object(replay,'check',return_value=(identity,{})),patch.object(replay.subprocess,'Popen',side_effect=launch):
                    with self.assertRaisesRegex(ValueError,'new kernel fault'):
                        replay.worker(root,cycle,helper,held.fileno(),watchdog=fault)
                self.assertEqual(len(children),1)
                self.assertIsNotNone(children[0].poll())
                self.assertIn('CLIENT_LEASE_HELD',(root/'evidence/decode.log').read_text())


if __name__ == '__main__':
    unittest.main()
