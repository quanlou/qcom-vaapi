#!/usr/bin/env python3
"""Check or temporarily load an ABI-reviewed Iris module; leave installed files intact."""
import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import stat
import struct
import subprocess
import tempfile

MODULE = Path('/sys/module/qcom_iris')
LOCK = Path('/tmp/libva-v4l2-hardware.lock')
# These binaries share the teardown path that deadlocked on 2026-10-01.
# A subprocess timeout cannot recover a module-removal syscall in D state.
UNSAFE_UNLOAD_BUILD_IDS = {
    'd32b38c1a71add26795cf6719c71268188da8a6b',  # installed distro module
    'c034f2232b2e6cf509c1eec1e9762e00edaf7ef4',  # bounds-only candidate
}


def open_lease():
    # Opening an existing user-owned /tmp file with O_CREAT can be rejected
    # even for sudo by fs.protected_regular. Do not truncate or follow links.
    flags = os.O_RDWR | os.O_CLOEXEC | os.O_NOFOLLOW
    try:
        fd = os.open(LOCK, flags)
    except FileNotFoundError:
        if os.geteuid() == 0:
            raise ValueError('run_check_only_as_decoder_user_to_create_shared_lease')
        try:
            fd = os.open(LOCK, flags | os.O_CREAT | os.O_EXCL, 0o600)
        except FileExistsError:
            fd = os.open(LOCK, flags)
    if not stat.S_ISREG(os.fstat(fd).st_mode):
        os.close(fd)
        raise ValueError('hardware_lease_is_not_a_regular_file')
    return os.fdopen(fd, 'r+')


class KernelCommandUnfinished(subprocess.SubprocessError):
    def __init__(self, command, pid):
        self.command, self.pid = command, pid
        super().__init__(f'kernel_command_unfinished pid={pid} command={command!r}; do_not_retry')


def run(command):
    # subprocess.run waits without a bound after killing a timed-out D-state
    # child. Retain its PID and stop instead of starting a concurrent rollback.
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        stdout, stderr = child.communicate(timeout=30)
    except subprocess.TimeoutExpired:
        child.kill()
        try:
            child.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            raise KernelCommandUnfinished(command, child.pid) from None
        raise
    if child.returncode:
        raise subprocess.CalledProcessError(child.returncode, command, stdout, stderr)
    return stdout.strip()


class EvidenceJournal:
    """Publish each phase before mutation, with atomic updates and durable bytes."""
    def __init__(self, path, **values):
        self.path, self.values = path, dict(values, status='pending', phases=[])
        self.fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o644)
        os.fchmod(self.fd, 0o644)
        with os.fdopen(self.fd, 'w') as out:
            json.dump(self.values, out, indent=2)
            out.write('\n')
            out.flush()
            os.fsync(out.fileno())

    def update(self, phase, **values):
        self.values.update(values)
        self.values['phase'] = phase
        self.values['phases'].append(dict(phase=phase, time=datetime.now(timezone.utc).isoformat()))
        fd, name = tempfile.mkstemp(prefix='.' + self.path.name + '.', dir=self.path.parent)
        try:
            os.fchmod(fd, 0o644)
            with os.fdopen(fd, 'w') as out:
                json.dump(self.values, out, indent=2)
                out.write('\n')
                out.flush()
                os.fsync(out.fileno())
            os.replace(name, self.path)
            parent = os.open(self.path.parent, os.O_RDONLY | os.O_DIRECTORY)
            try:
                os.fsync(parent)
            finally:
                os.close(parent)
        finally:
            if os.path.exists(name):
                os.unlink(name)


def loaded_build_id():
    if not MODULE.exists():
        return None
    note = (MODULE / 'notes/.note.gnu.build-id').read_bytes()
    namesz, descsz, kind = struct.unpack_from('<III', note)
    offset = 12 + ((namesz + 3) & ~3)
    if kind != 3 or note[12:12 + namesz] != b'GNU\0' or len(note) < offset + descsz:
        raise ValueError('invalid_loaded_build_id_note')
    return note[offset:offset + descsz].hex()


def file_build_id(path):
    result = re.findall(r'Build ID: ([0-9a-f]+)', run(['readelf', '-n', str(path)]))
    if len(result) != 1:
        raise ValueError('missing_or_ambiguous_module_build_id')
    return result[0]


def review_abi(candidate, expected, installed, symvers=None):
    actual = hashlib.sha256(candidate.read_bytes()).hexdigest()
    if actual != expected:
        raise ValueError('candidate_hash_changed')
    release = platform.release()
    candidate_vermagic = run(['modinfo', '-F', 'vermagic', str(candidate)])
    installed_vermagic = run(['modinfo', '-F', 'vermagic', str(installed)])
    if candidate_vermagic != installed_vermagic or candidate_vermagic.split()[0] != release:
        raise ValueError('kernel_abi_identity_mismatch')
    # Keep the installed module's CRC as authority for shared imports. Added
    # imports must match the exact running release's complete kernel table.
    def versions(path):
        return dict((name, crc) for crc, name in
                    (line.split() for line in run(['modprobe', '--show-modversions', str(path)]).splitlines()))
    original, proposed = versions(installed), versions(candidate)
    symvers = symvers or Path('/lib/modules') / release / 'build/Module.symvers'
    table = dict((fields[1], fields[0]) for line in symvers.read_text().splitlines()
                 if len(fields := line.split()) >= 2) if symvers.exists() else {}
    if not proposed or any((original.get(name, table.get(name)) != crc or
                           (name in table and table[name] != crc))
                          for name, crc in proposed.items()):
        raise ValueError('imported_symbol_crc_mismatch')
    candidate_id, original_id = file_build_id(candidate), file_build_id(installed)
    dependencies = list(filter(None, run(['modinfo', '-F', 'depends', str(candidate)]).split(',')))
    if any(not re.fullmatch(r'[A-Za-z0-9_-]+', name) or name.replace('-', '_') == 'qcom_iris'
           for name in dependencies):
        raise ValueError('invalid_candidate_dependencies')
    return dict(kernel=release, candidate_sha256=actual, candidate_build_id=candidate_id,
                original_build_id=original_id, imported_symbol_crcs=len(proposed),
                added_imports=sorted(proposed.keys() - original.keys()), dependencies=dependencies,
                symvers_sha256=hashlib.sha256(symvers.read_bytes()).hexdigest() if table else None,
                candidate=str(candidate), installed=str(installed))


def preflight(candidate, expected, installed, symvers=None):
    evidence = review_abi(candidate, expected, installed, symvers)
    current_id = loaded_build_id()
    if current_id not in (None, evidence['candidate_build_id'], evidence['original_build_id']):
        raise ValueError('loaded_module_is_neither_original_nor_candidate')
    if current_id is not None:
        state = MODULE / 'initstate'
        if state.exists() and state.read_text().strip() != 'live':
            raise ValueError('iris_module_transition')
        if int((MODULE / 'refcnt').read_text()) != 0 or list((MODULE / 'holders').iterdir()):
            raise ValueError('iris_module_in_use')
    return dict(evidence, boot_id=Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
                loaded_build_id=current_id)


def replace_module(candidate, evidence, rollback=False, progress=None, require_absent=False):
    progress = progress or (lambda *args, **kwargs: None)
    target = evidence['original_build_id' if rollback else 'candidate_build_id']
    current = loaded_build_id()
    if require_absent and current is not None:
        raise ValueError('cold_boot_requires_iris_absent; do_not_unload')
    if current == target:
        return
    if current in UNSAFE_UNLOAD_BUILD_IDS:
        raise ValueError('iris_live_unload_disabled_for_known_teardown_deadlock')
    # No forced unload. Idle refcounts do not prove teardown is safe.
    if current is not None:
        progress('unload_started')
        run(['modprobe', '-r', 'qcom_iris'])
        progress('unloaded')
    if not rollback and evidence.get('dependencies'):
        progress('dependencies_load_started')
        run(['modprobe', '--all', *evidence['dependencies']])
        progress('dependencies_loaded')
    if require_absent and loaded_build_id() is not None:
        raise ValueError('cold_boot_requires_iris_absent; do_not_unload')
    try:
        progress('insertion_started')
        run(['modprobe', 'qcom_iris'] if rollback else ['insmod', str(candidate)])
        if loaded_build_id() != target:
            raise ValueError('loaded_build_id_does_not_match_target')
        progress('loaded_identity_verified')
    except KernelCommandUnfinished:
        # An unfinished insertion is not a failed insertion; no further syscall
        # can establish safe rollback while that kernel operation is still live.
        raise
    except Exception:
        # Restore the installed module after a failed candidate load/verification.
        # If the wrong module did load, refuse a forced unload here too.
        if MODULE.exists():
            if loaded_build_id() in UNSAFE_UNLOAD_BUILD_IDS:
                raise ValueError('rollback_refused_known_teardown_deadlock')
            progress('restore_unload_started')
            run(['modprobe', '-r', 'qcom_iris'])
        progress('restore_original_started')
        run(['modprobe', 'qcom_iris'])
        if loaded_build_id() != evidence['original_build_id']:
            raise ValueError('rollback_build_id_mismatch')
        progress('original_restored')
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('candidate', type=Path)
    parser.add_argument('--sha256', required=True)
    parser.add_argument('--evidence', type=Path, required=True)
    parser.add_argument('--require-absent', action='store_true',
                        help='cold-boot insertion only; reject any loaded Iris module')
    parser.add_argument('--exclude-boot-id', action='append', default=[],
                        help='refuse a known faulted boot; repeat for earlier faults')
    action = parser.add_mutually_exclusive_group()
    action.add_argument('--activate', action='store_true')
    action.add_argument('--rollback', action='store_true')
    args = parser.parse_args()
    journal = None
    try:
        if (args.activate or args.rollback) and os.geteuid() != 0:
            raise ValueError('interactive_sudo_required_for_activation')
        if args.evidence.exists():
            raise ValueError('evidence_path_already_exists')
        candidate = args.candidate.resolve()
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        journal = EvidenceJournal(args.evidence, candidate=str(candidate), candidate_sha256=args.sha256,
                                  action='rollback' if args.rollback else 'activate' if args.activate else 'check_only')
        journal.update('preflight_started')
        if Path('/proc/sys/kernel/random/boot_id').read_text().strip() in args.exclude_boot_id:
            raise ValueError('faulted_boot_requires_restart')
        with open_lease() as lease, tempfile.TemporaryDirectory(prefix='iris-activation-') as tmp:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            installed_path = Path(run(['modinfo', '-F', 'filename', 'qcom_iris']))
            installed = installed_path
            if installed_path.suffix == '.zst':
                installed = Path(tmp) / 'original.ko'
                with installed.open('wb') as out:
                    subprocess.run(['zstd', '-dc', str(installed_path)], stdout=out, check=True, timeout=30)
            evidence = preflight(candidate, args.sha256, installed)
            if args.require_absent and evidence['loaded_build_id'] is not None:
                raise ValueError('cold_boot_requires_iris_absent; do_not_unload')
            evidence['installed'] = str(installed_path)
            evidence['installed_sha256'] = hashlib.sha256(installed_path.read_bytes()).hexdigest()
            journal.update('preflight_verified', **evidence)
            # Pin the checked bytes across module removal and insertion.
            pinned = Path(tmp) / 'candidate.ko'
            pinned.write_bytes(candidate.read_bytes())
            if hashlib.sha256(pinned.read_bytes()).hexdigest() != args.sha256:
                raise ValueError('candidate_changed_during_capture')
            if args.activate or args.rollback:
                replace_module(pinned, evidence, rollback=args.rollback, progress=journal.update,
                               require_absent=args.require_absent)
            evidence['loaded_build_id_after'] = loaded_build_id()
            evidence['action'] = 'rollback' if args.rollback else 'activate' if args.activate else 'check_only'
            journal.update('complete', status='pass', **evidence)
            print('iris_candidate=pass action=' + evidence['action'] + ' evidence=' + str(args.evidence))
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        if journal:
            values = dict(status='failed', error=str(error))
            if isinstance(error, KernelCommandUnfinished):
                values.update(status='unresolved', unfinished_pid=error.pid, unfinished_command=error.command)
            journal.update('failure', **values)
        print('iris_candidate=fail reason=' + str(error))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
