#!/usr/bin/env python3
"""One sealed, GPU-only byte-parity run; never opens a decoder or installs files."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import shutil
import struct
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
TEST = "gpu_copy::tests::gpu_dma_buf_byte_parity"
FAULTS = re.compile(
    r"session error received|received system error|video hw is power on|"
    r"Unhandled context fault|UBSAN:|KASAN:|BUG:|WARNING:|"
    r"Internal error:|Oops:|Kernel panic|blocked for more than|watchdog:.*lockup|GPU fault|GPU HANG|gpu.*hangcheck|"
    r"arm-smmu.*fault|Bad page|Corrupted page table|Bad swap file entry"
)
SESSION_ERROR = re.compile(r"^qcom-iris \S+: session error received 0x4000003: fatal error$")


def build_id(name):
    data = Path(f"/sys/module/{name}/notes/.note.gnu.build-id").read_bytes()
    namesz, descsz, kind = struct.unpack_from("=III", data)
    offset = 12 + (namesz + 3) // 4 * 4
    if kind != 3 or data[12:12 + namesz].rstrip(b"\0") != b"GNU":
        raise RuntimeError(f"invalid loaded build ID: {name}")
    value = data[offset:offset + descsz].hex()
    if len(value) != descsz * 2:
        raise RuntimeError(f"truncated loaded build ID: {name}")
    return value


def check_journal(journal, boot, kernel, actual, acknowledged=None):
    if not journal.strip():
        raise RuntimeError("unverifiable boot; no GPU/decoder opens")
    faults = [line for line in journal.splitlines() if FAULTS.search(line)]
    if acknowledged is None:
        if faults:
            raise RuntimeError("faulted boot; no GPU/decoder opens; no retry")
        return
    if (acknowledged.get("boot_id") != boot or acknowledged.get("kernel") != kernel
            or acknowledged.get("loaded_build_ids") != actual):
        raise RuntimeError("acknowledged session baseline identities changed")
    prior = acknowledged.get("session_errors")
    if not isinstance(prior, list) or not prior or any(
            not isinstance(line, str) or not SESSION_ERROR.fullmatch(line) for line in prior):
        raise RuntimeError("baseline must contain only explicitly acknowledged Iris session errors")
    if faults != prior:
        raise RuntimeError("new session or system/memory/GPU fault; stop without retry")


def preflight(expected, kernel, acknowledged=None):
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    if os.uname().release != kernel:
        raise RuntimeError("kernel identity mismatch")
    actual = {name: build_id(name) for name in expected}
    if actual != expected:
        raise RuntimeError(f"loaded module identities mismatch: {actual}")
    modules = {line.split()[0]: line.split() for line in Path("/proc/modules").read_text().splitlines()}
    if any(modules.get(name, [None] * 5)[4] != "Live" for name in expected):
        raise RuntimeError("a required module is not Live")
    if int(modules["qcom_iris"][2]) != 0:
        raise RuntimeError("decoder already open; exclusive idle hardware required")
    journal = subprocess.run(["journalctl", "-k", "-b", "--no-pager", "-o", "cat"],
                             capture_output=True, text=True, check=True).stdout
    check_journal(journal, boot, kernel, actual, acknowledged)
    return boot, actual, journal


def run(args):
    packet = Path(args.output).resolve()
    try:
        packet.mkdir(mode=0o700)  # Existing/sealed attempts must never be reused.
    except OSError as error:
        print(json.dumps({"status": "stopped", "reason": f"cannot create fresh packet: {error}",
                          "gpu_opens": 0, "decoder_opens": 0, "no_retry": True}))
        return 1
    result = {"status": "stopped", "hardware_qualification": False,
              "decoder_opens": 0, "gpu_opens": 0, "no_retry": True,
              "scope": "GPU transfer parity only; no browser or decoder qualification"}
    try:
        expected = {"qcom_iris": args.expect_iris, "qrtr": args.expect_qrtr,
                    "qrtr_mhi": args.expect_qrtr_mhi}
        if any(not re.fullmatch(r"[0-9a-f]{40}", value) for value in expected.values()):
            raise RuntimeError("expected build IDs must be 40 lowercase hex characters")
        acknowledged = None
        if args.acknowledged_session_baseline:
            data = Path(args.acknowledged_session_baseline).read_bytes()
            acknowledged = json.loads(data)
            (packet / "acknowledged-session-baseline.json").write_bytes(data)
            result["acknowledged_session_baseline_sha256"] = hashlib.sha256(data).hexdigest()
            result["prior_session_errors_acknowledged"] = len(acknowledged.get("session_errors", []))
        # Fail the faulted boot before even building a hardware packet.
        preflight(expected, args.kernel, acknowledged)
        built = subprocess.run(["cargo", "test", "--manifest-path", str(ROOT / "rust/Cargo.toml"),
                                "--locked", "--features", "gpu-copy,system-av1", "--lib", "--no-run",
                                "--message-format=json"], capture_output=True, text=True)
        (packet / "build.log").write_text(built.stderr + built.stdout)
        if built.returncode:
            raise RuntimeError("hardware parity test compilation failed")
        binaries = []
        for line in built.stdout.splitlines():
            record = json.loads(line)
            if record.get("reason") == "compiler-artifact" and record.get("executable"):
                binaries.append(record["executable"])
        if len(binaries) != 1:
            raise RuntimeError("could not identify exactly one private test executable")
        binary = packet / "gpu-parity-test"
        shutil.copyfile(binaries[0], binary)
        binary.chmod(0o500)
        result["test_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        with open("/tmp/libva-v4l2-hardware.lock", "r+") as lease:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            boot, actual, journal = preflight(expected, args.kernel, acknowledged)
            (packet / "kernel-before.log").write_text(journal)
            result.update(boot_id=boot, kernel=args.kernel, loaded_build_ids=actual)
            # The child inherits only these two fds. Hardware tests cannot be
            # invoked accidentally through an ordinary cargo test run.
            with open(args.drm, "r+b", buffering=0) as drm:
                result["gpu_opens"] = 1
                env = os.environ.copy()
                env.update(V4L2_VA_GPU_LEASE_FD=str(lease.fileno()),
                           V4L2_VA_GPU_DRM_FD=str(drm.fileno()), V4L2_VA_GPU_BOOT_ID=boot,
                           V4L2_VA_DEBUG="1", EGL_LOG_LEVEL="debug", MESA_DEBUG="1")
                with (packet / "parity.log").open("w") as log:
                    child = subprocess.Popen([str(binary), "--exact", TEST, "--ignored", "--nocapture",
                                               "--test-threads=1"], env=env, stdout=log,
                                             stderr=subprocess.STDOUT, start_new_session=True,
                                             pass_fds=(lease.fileno(), drm.fileno()))
                    try:
                        deadline = time.monotonic() + 90
                        while child.poll() is None:
                            if time.monotonic() >= deadline:
                                raise RuntimeError("GPU parity exceeded bounded window; preserve packet, no retry")
                            try:
                                child.wait(timeout=0.5)
                            except subprocess.TimeoutExpired:
                                preflight(expected, args.kernel, acknowledged)
                        status = child.returncode
                    finally:
                        if child.poll() is None:
                            os.killpg(child.pid, signal.SIGKILL)
                            child.wait(timeout=5)
            _, _, after = preflight(expected, args.kernel, acknowledged)
            (packet / "kernel-after.log").write_text(after)
            parity = (packet / "parity.log").read_text()
            if status or "1 passed; 0 failed; 0 ignored" not in parity or parity.count("gpu_parity ") != 8:
                raise RuntimeError("GPU parity failed or fell back; preserve packet, no retry")
            result.update(status="pass", hardware_qualification=True, transfers=48,
                          formats=["NV12", "P010"], maximum_dimensions="3840x2160")
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        result["reason"] = str(error)
        # Preserve the whole-boot log on any failure, without another hardware open.
        observed = subprocess.run(["journalctl", "-k", "-b", "--no-pager", "-o", "cat"],
                                  capture_output=True, text=True)
        (packet / "kernel-after.log").write_text(observed.stdout + observed.stderr)
    (packet / "result.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    return 0 if result["status"] == "pass" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, help="new evidence directory; an existing directory is refused")
    parser.add_argument("--kernel", required=True)
    parser.add_argument("--expect-iris", required=True)
    parser.add_argument("--expect-qrtr", required=True)
    parser.add_argument("--expect-qrtr-mhi", required=True)
    parser.add_argument("--drm", default="/dev/dri/renderD128")
    parser.add_argument("--acknowledged-session-baseline", help="explicitly authorized exact prior session-only errors; any new fault stops the run")
    return run(parser.parse_args())


if __name__ == "__main__":
    sys.exit(main())
