#!/usr/bin/env python3
"""Private 4K replay tracing lifecycle; never starts unowned browser playback.

Called only by the sealed replay runner inside its kernel observer and process
tree monitor. This retains the shared lease in the tracer and owns its cleanup.
Compile-only and readiness witnesses must precede every decoder open.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def require_compilation(packet):
    if os.geteuid() != 0:
        raise ValueError('operator root invocation required for tracing; no decoder opened')
    plan = json.loads((packet / 'plan.json').read_text())
    trace = packet / 'trace'
    # The enclosing replay manifest binds all these immutable files. The
    # operator-produced receipt is deliberately separate from that manifest.
    expected = json.loads((trace / 'plan.json').read_text())
    receipt = json.loads((trace / 'privileged-codegen.json').read_text())
    wanted_modules = {name: {'path': value['selected_module'], 'sha256': value['selected_sha256'],
                             'build_id': value['loaded_build_id']}
                      for name, value in plan['modules'].items()}
    if (expected['kernel'] != plan['kernel'] or expected['modules'] != wanted_modules or
            receipt.get('status') != 'pass' or receipt.get('exit_status') != 0 or
            receipt.get('error') is not None or receipt.get('attachments') != 0 or
            receipt.get('decoder_opens') != 0 or receipt.get('browser_launches') != 0 or
            receipt.get('live_traceable_inventory_verified') is not True or
            receipt.get('kernel') != plan['kernel'] or receipt.get('modules') != wanted_modules or
            receipt.get('trace_sha256') != sha(trace / 'trace.bt') or
            receipt.get('packet_manifest_sha256') != sha(trace / 'sha256.json') or
            receipt.get('command') != ['/usr/bin/bpftrace', '--mode', 'codegen', str(trace / 'trace.bt')]):
        raise ValueError('successful exact-packet compile-only receipt required; no decoder opened')
    code = (trace / 'trace.bt').read_text()
    if (re.search(r'\bnsecs\b(?!\s*\(monotonic\))', code) or
            code.count('IRIS_MEM_READY') != 1 or code.count('IRIS_MEM_STOPPED') != 1):
        raise ValueError('journal-compatible exact trace program required')
    return trace


class Tracer:
    def __init__(self, packet, evidence, lease_fd, kernel_watchdog):
        self.root = require_compilation(packet)
        self.output_path = evidence / 'trace.log'
        self.error_path = evidence / 'trace-errors.log'
        self.lease_fd = lease_fd
        self.kernel_watchdog = kernel_watchdog
        self.process = None
        self.output = None
        self.errors = None

    def health(self, ready=True):
        self.kernel_watchdog()
        if self.output_path.stat().st_size > 8*1024*1024 or self.error_path.stat().st_size > 1024*1024:
            raise ValueError('trace output budget exceeded; stop owned replay')
        output, errors = self.output_path.read_text(), self.error_path.read_text()
        if errors.strip() or re.search(r'\b(lost|dropped)\b', output, re.I):
            raise ValueError('tracer diagnostics or lost records; stop owned replay')
        if self.process.poll() is not None:
            raise ValueError('tracer exited before owned replay finished')
        if output.count('IRIS_MEM_READY') > 1 or 'IRIS_MEM_STOPPED' in output:
            raise ValueError('tracer readiness/termination changed during replay')
        attached = 'IRIS_MEM_READY\n' in output
        if ready and not attached:
            raise ValueError('trace READY missing; no decoder open')
        return attached

    def __enter__(self):
        try:
            self.output = self.output_path.open('x')
            self.errors = self.error_path.open('x')
            env = dict(os.environ, BPFTRACE_MAX_MAP_KEYS='8192', BPFTRACE_PERF_RB_PAGES='64')
            self.process = subprocess.Popen(['/usr/bin/bpftrace', '-k', '-q', '-B', 'line',
                                             str(self.root / 'trace.bt')],
                                            stdout=self.output, stderr=self.errors, env=env,
                                            pass_fds=(self.lease_fd,))
            deadline = time.monotonic()+15
            while not self.health(ready=False):
                if time.monotonic() >= deadline:
                    raise ValueError('trace attachment deadline; no decoder open')
                time.sleep(.05)
            return self
        except BaseException as error:
            try:
                self.stop()
            except Exception as cleanup:
                error.add_note('trace cleanup also failed: '+str(cleanup))
            raise

    def stop(self):
        try:
            if self.process is not None and self.process.poll() is None:
                self.process.send_signal(signal.SIGINT)
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.terminate()
                    self.process.wait(timeout=3)
                    raise ValueError('tracer required TERM cleanup; no clean qualification')
            if self.process is not None:
                if (self.output_path.stat().st_size > 8*1024*1024 or
                        self.error_path.stat().st_size > 1024*1024):
                    raise ValueError('trace output budget exceeded during cleanup')
                output, errors = self.output_path.read_text(), self.error_path.read_text()
                if (self.process.returncode != 0 or errors.strip() or
                        output.count('IRIS_MEM_READY') != 1 or output.count('IRIS_MEM_STOPPED') != 1 or
                        re.search(r'\b(lost|dropped)\b', output, re.I)):
                    raise ValueError('tracer cleanup/complete observation failed')
        finally:
            if self.output is not None:
                self.output.close()
            if self.errors is not None:
                self.errors.close()

    def __exit__(self, kind, error, traceback):
        try:
            self.stop()
        except Exception as cleanup:
            if error is None:
                raise
            error.add_note('trace cleanup also failed: '+str(cleanup))


def validate_observations(text):
    # READY alone is not proof that the required probes observed this run.
    # Pixel/publication checks remain independent in the original worker.
    rows = text.splitlines()
    queued_inputs = [row for row in rows if row.startswith('IRIS_MEM event=queue ') and
                     re.search(r'\btype=1\b', row)]
    encoded = [row for row in rows if row.startswith('IRIS_MEM event=hfi_encoded ')]
    outputs = [row for row in rows if row.startswith('IRIS_MEM event=output_response ')]
    if min(len(queued_inputs), len(encoded), len(outputs)) < 375:
        raise ValueError('actual full coded-frame trace observations missing')
    sessions = {re.search(r'\bsession=(\d+)\b', row).group(1)
                for row in queued_inputs + encoded + outputs
                if re.search(r'\bsession=(\d+)\b', row)}
    if (len(sessions) != 1 or
            any(not re.search(r'\bsession=\d+\b', row) for row in queued_inputs + encoded + outputs)):
        raise ValueError('owned replay trace session missing or competing session observed')
    return {'queued_inputs': len(queued_inputs), 'encoded': len(encoded),
            'outputs': len(outputs), 'session': next(iter(sessions))}


def worker(packet, cycle, helper, lease_fd, replay_worker):
    # The original worker rechecks the inherited lease and current seal before
    # launch. Do so before trace attachment as well, with no device use here.
    from importlib.util import module_from_spec, spec_from_file_location
    spec = spec_from_file_location('owned_trace_replay', Path(__file__).with_name('qualify-av1-4k-replay.py'))
    replay = module_from_spec(spec)
    spec.loader.exec_module(replay)
    replay.require_safe_boot(replay.BOOT.read_text().strip(), cycle.FAULTED)
    replay.require_inherited_lease(lease_fd, helper)
    before, _ = replay.check(packet, cycle, helper)
    expected = json.loads((packet / 'evidence/before.json').read_text())
    if before != expected:
        raise ValueError('identity changed before tracing')
    cursor = json.loads(cycle.journal('-n', '1', '-o', 'json'))['__CURSOR']

    def kernel_watchdog():
        cycle.require_clean_messages(cycle.journal('--after-cursor='+cursor, '-o', 'cat'))
        if replay.BOOT.read_text().strip() != before['boot_id']:
            raise ValueError('boot changed during tracing')

    with Tracer(packet, packet / 'evidence', lease_fd, kernel_watchdog) as tracer:
        # Exact seal/idle/whole-boot checks are repeated after attach, immediately
        # before the client opens. Loss of tracing fails the client wait loop.
        replay_worker(packet, cycle, helper, lease_fd, watchdog=tracer.health)
        tracer.health()
    observations = validate_observations(tracer.output_path.read_text())
    (packet / 'evidence/trace-observations.json').write_text(json.dumps(observations, indent=2)+'\n')
