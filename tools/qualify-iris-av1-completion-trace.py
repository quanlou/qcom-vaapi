#!/usr/bin/env python3
"""Operator-led read-only BPF AV1 diagnostic; never changes modules or boot files.

The frozen packet supplies DWARF-derived offsets bound to the exact loaded build.
Tracing must attach before decoding. Failed attempts are exclusive and never retried.
This observes firmware metadata, not hidden pixel validity or VAAPI support.
"""
import argparse
import fcntl
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent


def cycle():
    spec = importlib.util.spec_from_file_location('cycle', ROOT / 'qualify-iris-module-cycle.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def rows(path):
    return [line.split(',')[-1].strip() for line in path.read_text().splitlines()
            if line and not line.startswith('#')]


def inspect_trace(text):
    if text.count('AV1_TRACE_READY') != 1 or text.count('AV1_TRACE_STOPPED') != 1:
        raise ValueError('incomplete trace attachment/termination witness')
    events = []
    for line in text.splitlines():
        if not line.startswith('AV1_RAW '):
            continue
        event = dict((key, int(value)) for key, value in
                     re.findall(r'(\w+)=(-?\d+)', line))
        required = {'session', 'index', 'size', 'timestamp', 'flags', 'picture',
                    'no_output', 'corrupt', 'overflow', 'retval'}
        if set(event) != required or event['retval'] != 0:
            raise ValueError('invalid or rejected firmware completion')
        if event['corrupt'] or event['overflow']:
            raise ValueError('firmware corruption/overflow witnessed')
        events.append(event)
    hidden = [event for event in events if event['picture'] & 0x40]
    if not hidden:
        raise ValueError('no hidden firmware completions captured')
    return {'raw_completions': len(events), 'hidden_completions': len(hidden),
            'hidden_nonzero_size': sum(event['size'] > 0 for event in hidden),
            'hidden_nonzero_timestamp': sum(event['timestamp'] > 0 for event in hidden),
            'sessions': sorted({event['session'] for event in events}),
            'events': events,
            'scope': 'firmware metadata observation only; nonzero size does not prove pixels'}


def controls_valid(text):
    instances = {}
    matched = re.findall(
            r'(\[av1_v4l2m2m @ [^\]]+\]) IRIS_AV1_ORDER control=(\d+) value=(\d+) verified', text)
    for instance, control, value in matched:
        instances.setdefault(instance, []).append((int(control), int(value)))
    return (len(matched) == text.count('IRIS_AV1_ORDER control=')
            and bool(instances) and all(values == [(10029965, 0), (10029966, 1)]
                                   for values in instances.values())
            )


def worker(packet, evidence):
    if os.geteuid() != 0:
        raise ValueError('local operator root invocation required')
    plan = json.loads((packet / 'plan.json').read_text())
    c = cycle()
    c.inspect(c.activation_helper())
    c.require_clean_messages(c.journal('-o', 'cat'))
    env = dict(os.environ, LC_ALL='C')
    with (evidence / 'bpf.log').open('x') as output, (evidence / 'bpf-errors.log').open('x') as errors:
        trace = subprocess.Popen(['bpftrace', '-k', '-B', 'line', '-q', str(packet / 'trace.bt')],
                                 stdout=output, stderr=errors, env=env)
        trace_status = None
        try:
            deadline = time.monotonic() + 15
            while 'AV1_TRACE_READY' not in (evidence / 'bpf.log').read_text():
                if trace.poll() is not None or time.monotonic() >= deadline:
                    raise ValueError('tracer unavailable: no decoder opened')
                time.sleep(.1)
            if trace.poll() is not None:
                raise ValueError('tracer exited before decoder open')
            device = subprocess.check_output([sys.executable, str(packet / 'select-iris-device.py')],
                                             text=True).strip()
            env.update(V4L2_VA_DEVICE=device, IRIS_AV1_ORDER_DIAGNOSTIC='1')
            command = [str(packet / 'native/ffmpeg'), '-nostdin', '-hide_banner', '-v', 'verbose',
                       '-c:v', 'av1_v4l2m2m', '-num_capture_buffers', '32', '-num_output_buffers', '4',
                       '-i', plan['sample'], '-map', '0:v:0', '-an', '-threads:v', '1',
                       '-f', 'framemd5', str(evidence / 'native.md5')]
            (evidence / 'native-command.json').write_text(json.dumps(command) + '\n')
            with (evidence / 'native.log').open('x') as log:
                cursor = json.loads(c.journal('-n', '1', '-o', 'json'))['__CURSOR']
                native = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
                # Outer process-tree monitor owns the bounded timeout and cleanup.
                next_kernel_check = time.monotonic()
                while native.poll() is None:
                    if trace.poll() is not None:
                        native.terminate()
                        native.wait(timeout=5)
                        raise ValueError('tracer exited during decode')
                    if time.monotonic() >= next_kernel_check:
                        try:
                            c.require_clean_messages(c.journal('--after-cursor=' + cursor, '-o', 'cat'))
                        except Exception:
                            native.terminate()
                            native.wait(timeout=5)
                            raise
                        next_kernel_check = time.monotonic() + .25
                    time.sleep(.05)
                if native.returncode:
                    raise ValueError('native decode failed')
        finally:
            if trace.poll() is None:
                trace.send_signal(signal.SIGINT)
            try:
                trace_status = trace.wait(timeout=5)
            except subprocess.TimeoutExpired:
                trace.terminate()
                trace.wait(timeout=5)
                raise ValueError('tracer failed clean stop')
            (evidence / 'trace-exit.json').write_text(json.dumps({'exit_status': trace_status}) + '\n')
    if trace_status != 0:
        raise ValueError('tracer failed clean exit')
    errors_text = (evidence / 'bpf-errors.log').read_text()
    if re.search(r'ERROR|WARNING|lost|dropped|failed|fault', errors_text, re.IGNORECASE):
        raise ValueError('trace errors or dropped observations; preserve diagnostics')
    report = inspect_trace((evidence / 'bpf.log').read_text())
    reference = rows(packet / 'software.md5')
    if rows(evidence / 'native.md5') != reference or len(reference) != plan['required_frames']:
        raise ValueError('displayed AV1 pixel/order parity failed')
    if not controls_valid((evidence / 'native.log').read_text()):
        raise ValueError('decode-order controls not verified for every codec instance')
    report.update(status='pass', pixel_parity=True, displayed_frames=len(reference))
    (evidence / 'worker-result.json').write_text(json.dumps(report, indent=2) + '\n')



def wait_for_idle(c, helper, expected, cursor, seconds=15, clock=time.monotonic,
                  sleep=time.sleep):
    """Observe autosuspend settling; never retry decoding or identity/fault errors."""
    deadline = clock() + seconds
    while True:
        c.require_clean_messages(c.journal('--after-cursor=' + cursor, '-o', 'cat'))
        try:
            state = c.inspect(helper)
        except ValueError as error:
            if str(error) != 'decoder is busy or not safely runtime-suspended':
                raise
            if clock() >= deadline:
                raise ValueError('decoder did not settle to safe runtime idle within deadline') from error
            sleep(min(.1, max(0, deadline - clock())))
            continue
        if state['boot_id'] != expected['boot_id'] or state['build_id'] != expected['build_id']:
            raise ValueError('final loaded identity changed')
        return state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'run', 'worker'])
    parser.add_argument('packet', type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve()
    evidence = packet / 'evidence'
    if args.action == 'worker':
        worker(packet, evidence)
        return 0
    plan = json.loads((packet / 'plan.json').read_text())
    c = cycle()
    helper = c.activation_helper()
    with helper.open_lease() as lease:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = c.inspect(helper)
        if state['boot_id'] != plan['boot_id']:
            raise ValueError('frozen boot changed')
        c.require_clean_messages(c.journal('-o', 'cat'))
        if args.action == 'check':
            print(json.dumps({'status': 'prepared', 'state': state, 'no_decoder_open': True,
                              'root_trace_attach_still_unproven': True}))
            return 0
        if os.geteuid() != 0:
            raise ValueError('operator sudo required')
        prior = Path(plan['pending_refusal_packet']) / 'evidence'
        if (json.loads((prior / 'result.json').read_text()).get('status') != 'pass'
                or json.loads((prior / 'after.json').read_text()).get('boot_id') != state['boot_id']):
            raise ValueError('sameboot client-open refusal prerequisite missing or failed')
        evidence.mkdir(exist_ok=False)
        (evidence / 'before.json').write_text(json.dumps(state, indent=2) + '\n')
        cursor = json.loads(c.journal('-n', '1', '-o', 'json'))['__CURSOR']
        command = [str(ROOT / 'capture-iris-kernel-log.sh'), '--', sys.executable,
                   str(ROOT / 'measure-process-tree.py'), '--seconds', '45', '--output',
                   str(evidence / 'process.json'), '--', sys.executable,
                   str(Path(__file__).resolve()), 'worker', str(packet)]
        with (evidence / 'observer.log').open('x') as log:
            status = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT).returncode
        messages = c.journal('--after-cursor=' + cursor, '-o', 'cat')
        (evidence / 'kernel.log').write_text(messages)
        try:
            c.require_clean_messages(messages)
            measure = json.loads((evidence / 'process.json').read_text())
            if (status or measure.get('exit_status') != 0
                    or measure.get('timed_out') is not False
                    or measure.get('lingering_descendants') is not False
                    or measure.get('signal_denied') or measure.get('unresolved_pids')):
                raise ValueError('observer/worker/cleanup failed')
            after = wait_for_idle(c, helper, state, cursor)
            (evidence / 'kernel.log').write_text(c.journal('--after-cursor=' + cursor, '-o', 'cat'))
            if after['boot_id'] != state['boot_id'] or after['build_id'] != state['build_id']:
                raise ValueError('final loaded identity changed')
            (evidence / 'after.json').write_text(json.dumps(after, indent=2) + '\n')
            result = json.loads((evidence / 'worker-result.json').read_text())
        except Exception as error:
            result = {'status': 'fail', 'reason': str(error), 'no_retry': True}
        (evidence / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({key: value for key, value in result.items() if key != 'events'}))
        return 0 if result['status'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
