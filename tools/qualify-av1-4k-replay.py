#!/usr/bin/env python3
"""Run one frozen 4K AV1 pixel experiment on a verified clean boot.

This never installs, sleeps, reloads or retries. A cold identity seal is
read-only and does not qualify playback. The run requires that seal, the
shared hardware lease, complete kernel observation and bounded child cleanup.
"""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import struct
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent
BOOT = Path('/proc/sys/kernel/random/boot_id')


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def sha(path):
    with Path(path).open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def require_safe_boot(boot, forbidden):
    if boot in forbidden:
        raise ValueError('faulted boot: no decoder opens; no retry')


def verify_files(packet):
    manifest = json.loads((packet / 'sha256.json').read_text())
    for name, expected in manifest.items():
        path = packet / name
        if Path(name).is_absolute() or '..' in Path(name).parts or not path.resolve().is_relative_to(packet):
            raise ValueError('manifest path escapes packet')
        if sha(path) != expected:
            raise ValueError('frozen file changed: ' + name)


def loaded_id(name):
    raw = (Path('/sys/module') / name / 'notes/.note.gnu.build-id').read_bytes()
    n, d, kind = struct.unpack_from('<III', raw)
    offset = 12 + ((n + 3) & ~3)
    if kind != 3 or raw[12:12+n] != b'GNU\0' or len(raw) < offset+d:
        raise ValueError('invalid loaded build note: ' + name)
    return raw[offset:offset+d].hex()


def identities(packet, cycle):
    # Refuse a known fault before module inspection, leases or any device use.
    boot = BOOT.read_text().strip()
    require_safe_boot(boot, cycle.FAULTED)
    verify_files(packet)
    plan = json.loads((packet / 'plan.json').read_text())
    if os.uname().release != plan['kernel']:
        raise ValueError('kernel changed')
    validate_reference(packet)
    actual = {}
    for name, expected in plan['modules'].items():
        selected = Path(subprocess.check_output(['modinfo', '-n', name], text=True, timeout=5).strip())
        if selected.resolve() != Path(expected['selected_module']).resolve() or sha(selected) != expected['selected_sha256']:
            raise ValueError('selected module changed: ' + name)
        actual[name] = loaded_id(name)
        if actual[name] != expected['loaded_build_id']:
            raise ValueError('actual loaded module changed: ' + name)
    cycle.require_clean_messages(cycle.journal('-o', 'cat'))
    return {'boot_id': boot, 'kernel': plan['kernel'], 'actual_build_ids': actual,
            'packet_manifest_sha256': sha(packet / 'sha256.json')}


def check(packet, cycle, helper, sealed=True):
    actual = identities(packet, cycle)
    state = cycle.inspect(helper)
    if state['boot_id'] != actual['boot_id']:
        raise ValueError('boot changed during preflight')
    if sealed:
        seal = json.loads((packet / 'cold-identity.json').read_text())
        if seal.get('status') != 'pass' or seal.get('action') != 'read_only_cold_verification' or seal.get('identity') != actual:
            raise ValueError('successful current-boot cold identity seal required')
    return actual, state


def checksum_rows(path, reference=False):
    content = path.read_text()
    if reference and ('#dimensions 0: 3840x2160' not in content or '#hash: MD5' not in content):
        raise ValueError('reference must be frozen 4K NV12 MD5')
    hashes = []
    for line in content.splitlines():
        if not line or line.startswith('#'):
            continue
        fields = [x.strip() for x in line.split(',')]
        if len(fields) != 6 or fields[0] != '0' or not re.fullmatch('[0-9a-f]{32}', fields[-1]):
            raise ValueError('invalid checksum record')
        if int(fields[4]) != 3840 * 2160 * 3 // 2:
            raise ValueError('frame is not tightly packed 4K NV12')
        hashes.append(fields[-1])
    if len(hashes) != 375:
        raise ValueError('all 375 coded frames required')
    return hashes


def validate_reference(packet):
    witness = (packet / 'fixtures/software-reference.log').read_text()
    if not re.search(r'Video: rawvideo[^\n]*\(NV12[^\n]* nv12', witness) or '375 frames decoded; 0 decode errors' not in witness:
        raise ValueError('explicit NV12 software generation witness missing')
    return checksum_rows(packet / 'fixtures/coded-software.md5', True)


def validate_result(packet, evidence):
    if checksum_rows(evidence / 'actual.md5') != validate_reference(packet):
        raise ValueError('coded pixel/order parity failed; no retry')
    log = (evidence / 'decode.log').read_text()
    if (log.count('msm_drv_video_rs: EndPicture context=') != 375 or
            log.count('msm_drv_video_rs: publish surface=') != 375 or
            'OUTPUT fmt fourcc=0x31305641' not in log or 'decode-order output requested' not in log or
            'terminal decoder poll' in log):
        raise ValueError('actual AV1 queue/publication witness failed')


def validate_cleanup(measurement, observer_status, memory_limit):
    if (observer_status or measurement.get('exit_status') != 0 or
            measurement.get('timed_out') is not False or measurement.get('lingering_descendants') is not False or
            measurement.get('signal_denied') or measurement.get('unresolved_pids') or
            measurement.get('interrupted')):
        raise ValueError('observer/child cleanup failed; no retry')
    peak = measurement.get('peak_rss_kib')
    if type(peak) is not int or peak < 0 or peak > memory_limit:
        raise ValueError('headless memory budget failed')


def require_inherited_lease(fd, helper):
    if fd is None or fd < 3:
        raise ValueError('worker requires inherited shared lease')
    actual = os.fstat(fd)
    expected = os.stat(helper.LOCK, follow_symlinks=False)
    if not stat.S_ISREG(actual.st_mode) or (actual.st_dev, actual.st_ino) != (expected.st_dev, expected.st_ino):
        raise ValueError('inherited descriptor is not the shared lease')
    fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)


def worker(packet, cycle, helper, lease_fd, watchdog=None):
    require_inherited_lease(lease_fd, helper)
    expected = json.loads((packet / 'evidence/before.json').read_text())
    now, _ = check(packet, cycle, helper)
    if now != expected:
        raise ValueError('worker identity changed')
    evidence = packet / 'evidence'
    cursor = json.loads(cycle.journal('-n', '1', '-o', 'json'))['__CURSOR']
    env = dict(os.environ, LC_ALL='C', LIBVA_DRIVER_NAME='msm', LIBVA_DRIVERS_PATH=str(packet / 'driver'),
               V4L2_VA_DEBUG='1', V4L2_VA_EXPERIMENTAL_AV1='1', V4L2_VA_AV1_CBS_TRANSPORT='1',
               V4L2_VA_AV1_COMPLETE_LIBRARY=str(packet / 'lib/libiris_av1_complete.so'))
    env.pop('V4L2_VA_DEVICE', None)
    # The outer process-tree monitor owns cleanup even if this worker fails.
    with (evidence / 'decode.log').open('x') as log:
        if watchdog is not None:
            watchdog()
        child = subprocess.Popen([str(packet / 'replay'), '/dev/dri/renderD128',
                                  str(packet / 'fixtures/captures.bin'), str(evidence / 'actual.md5')],
                                 stdout=log, stderr=subprocess.STDOUT, env=env,
                                 pass_fds=(lease_fd,))
        try:
            while child.poll() is None:
                if watchdog is not None:
                    watchdog()
                cycle.require_clean_messages(cycle.journal('--after-cursor=' + cursor, '-o', 'cat'))
                if BOOT.read_text().strip() != expected['boot_id']:
                    raise ValueError('boot changed during decode')
                time.sleep(.25)
            if child.returncode:
                raise ValueError('AV1 replay failed; no retry')
            if watchdog is not None:
                watchdog()
        finally:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    # The monitor records and bounds any remaining child.
                    raise ValueError('decoder child did not exit after stop; no retry') from None
    validate_result(packet, evidence)
    trace = load('replay_idle', 'qualify-iris-av1-completion-trace.py')
    trace.wait_for_idle(cycle, helper, {'boot_id': expected['boot_id'], 'build_id': expected['actual_build_ids']['qcom_iris']}, cursor)
    check(packet, cycle, helper)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['check', 'seal', 'run', 'worker'])
    parser.add_argument('packet', type=Path)
    parser.add_argument('--held-lease-fd', type=int)
    args = parser.parse_args()
    packet = args.packet.resolve()
    cycle = load('replay_cycle', 'qualify-iris-module-cycle.py')
    # Check before opening even the lease. Worker is private to the monitor.
    require_safe_boot(BOOT.read_text().strip(), cycle.FAULTED)
    helper = cycle.activation_helper()
    trace_config = json.loads((packet / 'plan.json').read_text()).get('trace')
    if args.action == 'worker':
        if trace_config:
            trace = load('owned_replay_trace', 'trace-iris-owned-replay.py')
            trace.worker(packet, cycle, helper, args.held_lease_fd, worker)
        else:
            worker(packet, cycle, helper, args.held_lease_fd)
        return 0
    with helper.open_lease() as lease:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        identity, state = check(packet, cycle, helper, args.action in ['run'])
        if args.action == 'seal':
            with (packet / 'cold-identity.json').open('x') as output:
                json.dump({'status': 'pass', 'action': 'read_only_cold_verification', 'identity': identity, 'idle': state}, output, indent=2)
            print(json.dumps({'status': 'pass', 'scope': 'read-only cold identities; no decoder open', **identity}))
            return 0
        if args.action == 'check':
            print(json.dumps({'status': 'ready', 'scope': 'preflight only; no decoder open', **identity}))
            return 0
        if trace_config:
            trace = load('owned_replay_trace', 'trace-iris-owned-replay.py')
            trace.require_compilation(packet)
        evidence = packet / 'evidence'
        evidence.mkdir(exist_ok=False)
        (evidence / 'before.json').write_text(json.dumps(identity, indent=2) + '\n')
        (evidence / 'before-idle.json').write_text(json.dumps(state, indent=2) + '\n')
        cursor = json.loads(cycle.journal('-n', '1', '-o', 'json'))['__CURSOR']
        plan = json.loads((packet / 'plan.json').read_text())
        command = [str(ROOT / 'capture-iris-kernel-log.sh'), '--', sys.executable,
                   str(ROOT / 'measure-process-tree.py'), '--seconds', str(plan['timeout_seconds']),
                   '--output', str(evidence / 'process.json'), '--inherit-fd', str(lease.fileno()),
                   '--', sys.executable, str(Path(__file__).resolve()), 'worker', str(packet),
                   '--held-lease-fd', str(lease.fileno())]
        with (evidence / 'observer.log').open('x') as log:
            status = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, pass_fds=(lease.fileno(),)).returncode
        try:
            messages = cycle.journal('--after-cursor=' + cursor, '-o', 'cat')
            (evidence / 'kernel.log').write_text(messages)
            cycle.require_clean_messages(messages)
            if 'verdict: no iris firmware errors in this window (clean).' not in (evidence / 'observer.log').read_text():
                raise ValueError('complete clean kernel window missing')
            process = json.loads((evidence / 'process.json').read_text())
            validate_cleanup(process, status, plan['max_rss_kib'])
            validate_result(packet, evidence)
            after, idle = check(packet, cycle, helper)
            if after != identity:
                raise ValueError('final loaded/frozen identities changed')
            (evidence / 'after-idle.json').write_text(json.dumps(idle, indent=2) + '\n')
            result = {'status': 'pass', 'coded_frames': 375, 'pixel_format': 'NV12', 'kernel_window': 'clean',
                      'boot_id': identity['boot_id'], 'scope': '4K AV1 coded pixels/order and bounded teardown only; browser/sustained performance unqualified'}
        except Exception as error:
            result = {'status': 'fail', 'reason': str(error), 'no_retry': True, 'boot_id': identity['boot_id']}
        (evidence / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result))
        return 0 if result['status'] == 'pass' else 1


if __name__ == '__main__':
    try:
        raise SystemExit(main())
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        print(json.dumps({'status': 'fail', 'reason': str(error), 'no_retry': True}), file=sys.stderr)
        raise SystemExit(1)
