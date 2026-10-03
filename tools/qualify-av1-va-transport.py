#!/usr/bin/env python3
"""Single frozen paired AV1 VA experiment; never enables installed AV1 support."""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
import re
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


def input_seek_args(plan):
    if not plan or 'input_seek_seconds' not in plan:
        return []
    value = plan['input_seek_seconds']
    if type(value) not in (int, float) or not 0 < value <= 3600:
        raise ValueError('invalid bounded input seek')
    return ['-ss', str(value)]


def va_command(packet, evidence, plan=None):
    if plan and plan.get('mode') == 'va_replay':
        if input_seek_args(plan):
            raise ValueError('replay fixture already defines the exact input boundary')
        return [str(packet/'producer/replay'), '/dev/dri/renderD128',
                str(packet/'original-captures.bin'), str(evidence/'va.md5')]
    return [str(packet/'producer/ffmpeg'), '-nostdin', '-hide_banner', '-v', 'verbose',
            '-nofind_stream_info', '-hwaccel', 'vaapi', '-hwaccel_device', '/dev/dri/renderD128',
            '-hwaccel_output_format', 'vaapi', '-c:v', 'av1'] + input_seek_args(plan) + [
            '-i', str(packet/'sample.ivf'),
            '-map', '0:v:0', '-an', '-vf', 'hwdownload,format=nv12', '-threads:v', '1',
            '-fps_mode', 'passthrough', '-f', 'framemd5', str(evidence/'va.md5')]


def software_reference_command(packet, plan=None):
    return ['ffmpeg', '-nostdin', '-hide_banner', '-v', 'verbose', '-c:v', 'libdav1d',
            *input_seek_args(plan), '-i', str(packet/'sample.ivf'), '-map', '0:v:0', '-an', '-vf', 'format=nv12',
            '-threads:v', '1', '-fps_mode', 'passthrough', '-f', 'framemd5',
            str(packet/'original-software.md5')]


def require_reference_format(packet, plan):
    if (plan.get('reference_format') != 'nv12'
            or not re.search(r'Video: rawvideo[^\n]*\(NV12[^\n]* nv12',
                             (packet/'software-reference.log').read_text())):
        raise ValueError('software reference must explicitly use the hardware NV12 layout')


def verify_pixels(packet, evidence, plan, rows):
    actual = rows(evidence/'va.md5')
    wanted = rows(packet/'original-software.md5')
    if plan.get('mode') == 'va_replay':
        coded = rows(packet/'coded-software.md5')
        indices = json.loads((packet/'display-indices.json').read_text())
        if (actual != coded or len(actual) != plan['decoded_frames']
                or any(type(i) is not int or not 0 <= i < len(actual) for i in indices)):
            raise ValueError('complete-buffer coded pixel/order parity failed')
        actual = [actual[i] for i in indices]
    if actual != wanted or len(actual) != plan['displayed_frames']:
        raise ValueError('original VA display pixel/order parity failed')
    return len(actual)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'run', 'worker'])
    parser.add_argument('packet', type=Path)
    args = parser.parse_args()
    packet = args.packet.resolve()
    evidence = packet/'evidence'
    plan = json.loads((packet/'plan.json').read_text())
    input_seek_args(plan)
    require_reference_format(packet, plan)
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
        env = dict(os.environ, LC_ALL='C', V4L2_VA_DEVICE=node, LIBVA_DRIVER_NAME='msm',
                   LIBVA_DRIVERS_PATH=str(packet/'driver'), V4L2_VA_EXPERIMENTAL_AV1='1',
                   V4L2_VA_AV1_CBS_TRANSPORT='1', V4L2_VA_DEBUG='1')
        if plan.get('mode') == 'va_replay':
            env['V4L2_VA_AV1_COMPLETE_LIBRARY'] = str(packet/'companion/libiris_av1_complete.so')
        command = va_command(packet, evidence, plan)
        (evidence/'command.json').write_text(json.dumps(command)+'\n')
        cursor = json.loads(c.journal('-n', '1', '-o', 'json'))['__CURSOR']
        with (evidence/'va.log').open('x') as log:
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
                raise ValueError('paired VA decode failed; no retry')
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
                raise ValueError('observer/VA/process cleanup failed')
            after = trace.wait_for_idle(c, helper, state, cursor)
            subprocess.run([sys.executable, str(packet/'verify-loaded.py')], check=True)
            (evidence/'after.json').write_text(json.dumps(after, indent=2)+'\n')
            displayed_frames = verify_pixels(packet, evidence, plan, trace.rows)
            log_text = (evidence/'va.log').read_text()
            if (log_text.count('msm_drv_video_rs: EndPicture context=') != plan['decoded_frames']
                    or 'msm_drv_video_rs: decode-order output requested via display-delay controls' not in log_text
                    or 'msm_drv_video_rs: OUTPUT fmt fourcc=0x31305641' not in log_text):
                raise ValueError('actual VA coded-frame/control witness missing')
            c.require_clean_messages(c.journal('--after-cursor='+cursor, '-o', 'cat'))
            result = {'status': 'pass', 'decoded_frames': plan['decoded_frames'], 'displayed_frames': displayed_frames,
                      'pixel_parity': True, 'scope': 'paired producer AV1 VA experiment only; lifecycle/Chromium/production unqualified'}
        except Exception as error:
            result = {'status': 'fail', 'reason': str(error), 'no_retry': True}
        (evidence/'kernel.log').write_text(c.journal('--after-cursor='+cursor, '-o', 'cat'))
        (evidence/'result.json').write_text(json.dumps(result, indent=2)+'\n')
        print(json.dumps(result))
        return 0 if result['status'] == 'pass' else 1


if __name__ == '__main__':
    raise SystemExit(main())
