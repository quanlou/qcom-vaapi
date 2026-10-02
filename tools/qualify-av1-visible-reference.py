#!/usr/bin/env python3
"""Single frozen native AV1 visibility experiment; never enables VAAPI support."""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def native_command(packet, evidence):
    return [str(packet/'native/ffmpeg'), '-nostdin', '-hide_banner', '-v', 'verbose',
            '-nofind_stream_info', '-c:v', 'av1_v4l2m2m', '-num_capture_buffers', '32',
            '-num_output_buffers', '4', '-i', str(packet/'coded-frames.ivf'),
            '-map', '0:v:0', '-an', '-threads:v', '1', '-fps_mode', 'passthrough',
            '-f', 'framemd5', str(evidence/'native.md5')]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'run', 'worker'])
    parser.add_argument('packet', type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve()
    evidence = packet/'evidence'
    plan = json.loads((packet/'plan.json').read_text())
    trace = load('trace', ROOT/'qualify-iris-av1-completion-trace.py')
    c = trace.cycle()
    helper = c.activation_helper()
    for name, want in json.loads((packet/'sha256.json').read_text()).items():
        if hashlib.sha256((packet/name).read_bytes()).hexdigest() != want:
            raise ValueError('frozen identity mismatch: '+name)
    gate = Path(plan['preceding_gate'])
    log = (gate/'full-gate.log').read_text()
    if not plan.get('baseline_deferred_by_user', False) and ('production=pass scope=provided_media_and_headless_lifecycle' not in log
            or not log.rstrip().endswith('final_identity=pass driver_sha256='
                                        'a1cbbb6b41019104d5f5057e05cb4d569dd7984c278cdc5804ad8ea033ee50ff')):
        raise ValueError('fresh sameboot supported baseline not complete')
    preceding = json.loads((gate/'kernel-activation.json').read_text())
    if preceding['boot_id'] != plan['boot_id'] or preceding['status'] != 'pass':
        raise ValueError('preceding supported baseline boot mismatch')
    if args.action == 'worker':
        subprocess.run([sys.executable, str(packet/'verify-loaded.py')], check=True)
        state = c.inspect(helper)
        before = json.loads((evidence/'before.json').read_text())
        if state['boot_id'] != before['boot_id'] or state['boot_id'] != plan['boot_id']:
            raise ValueError('worker boot changed')
        c.require_clean_messages(c.journal('-o', 'cat'))
        (evidence/'worker-started.json').open('x').write(json.dumps(state)+'\n')
        node = subprocess.check_output([sys.executable, str(packet/'select-iris-device.py')], text=True).strip()
        env = dict(os.environ, LC_ALL='C', V4L2_VA_DEVICE=node, IRIS_AV1_ORDER_DIAGNOSTIC='1')
        command = native_command(packet, evidence)
        (evidence/'command.json').write_text(json.dumps(command)+'\n')
        cursor = json.loads(c.journal('-n', '1', '-o', 'json'))['__CURSOR']
        with (evidence/'native.log').open('x') as log:
            process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, env=env)
            while process.poll() is None:
                try:
                    c.require_clean_messages(c.journal('--after-cursor='+cursor, '-o', 'cat'))
                except Exception:
                    process.terminate()
                    process.wait(timeout=5)
                    raise
                time.sleep(.25)
            if process.returncode:
                raise ValueError('native decode failed; no retry')
        return 0
    with helper.open_lease() as lease:
        fcntl.flock(lease, fcntl.LOCK_EX|fcntl.LOCK_NB)
        subprocess.run([sys.executable, str(packet/'verify-loaded.py')], check=True)
        state = c.inspect(helper)
        if state['boot_id'] != plan['boot_id']:
            raise ValueError('frozen boot changed')
        activation = json.loads((packet/'kernel-activation.json').read_text())
        if (activation['status'] != 'pass' or activation['boot_id'] != state['boot_id']
                or activation['candidate_build_id'] != state['build_id']):
            raise ValueError('current activation mismatch')
        c.require_clean_messages(c.journal('-o', 'cat'))
        if args.action == 'check':
            print(json.dumps({'status': 'prepared', 'no_decoder_open': True, 'state': state}))
            return 0
        evidence.mkdir(exist_ok=False)
        (evidence/'before.json').write_text(json.dumps(state, indent=2)+'\n')
        cursor = json.loads(c.journal('-n', '1', '-o', 'json'))['__CURSOR']
        command = [str(ROOT/'capture-iris-kernel-log.sh'), '--', sys.executable,
                   str(ROOT/'measure-process-tree.py'), '--seconds', '45', '--output',
                   str(evidence/'process.json'), '--', sys.executable, str(Path(__file__).resolve()),
                   'worker', str(packet)]
        with (evidence/'observer.log').open('x') as log:
            status = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT).returncode
        try:
            c.require_clean_messages(c.journal('--after-cursor='+cursor, '-o', 'cat'))
            measure = json.loads((evidence/'process.json').read_text())
            if (status or measure['exit_status'] or measure['timed_out'] or measure['lingering_descendants']
                    or measure.get('signal_denied') or measure.get('unresolved_pids')):
                raise ValueError('observer/native/process cleanup failed')
            after = trace.wait_for_idle(c, helper, state, cursor)
            subprocess.run([sys.executable, str(packet/'verify-loaded.py')], check=True)
            (evidence/'after.json').write_text(json.dumps(after, indent=2)+'\n')
            actual, wanted = trace.rows(evidence/'native.md5'), trace.rows(packet/'normalized-software.md5')
            if actual != wanted or len(actual) != plan['decoded_frames']:
                raise ValueError('normalized coded-frame pixel/order parity failed')
            projection = load('projection', ROOT/'verify-av1-visible-reference-host.py')
            displayed = projection.project(projection.records(packet/'original-index.jsonl'), actual)
            if displayed != trace.rows(packet/'original-software.md5') or len(displayed) != plan['displayed_frames']:
                raise ValueError('original display reference projection failed')
            if not trace.controls_valid((evidence/'native.log').read_text()):
                raise ValueError('codec controls not verified')
            c.require_clean_messages(c.journal('--after-cursor='+cursor, '-o', 'cat'))
            result = {'status': 'pass', 'decoded_frames': len(actual), 'displayed_frames': len(displayed),
                      'pixel_parity': True, 'scope': 'native normalized AV1/reference projection only; VAAPI/Chromium unqualified'}
        except Exception as error:
            result = {'status': 'fail', 'reason': str(error), 'no_retry': True}
        (evidence/'kernel.log').write_text(c.journal('--after-cursor='+cursor, '-o', 'cat'))
        (evidence/'result.json').write_text(json.dumps(result, indent=2)+'\n')
        print(json.dumps(result))
        return 0 if result['status'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
