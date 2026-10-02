#!/usr/bin/env python3
"""Operator-led idle Iris removal/reload; check never changes the live module.

Run only after explicit operator authorization. No force removal, dependency
removal, sleep, reboot, installation or boot-file writes. A failure never retries.
"""
import argparse
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
BUILD = "231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2"
MODULE_SHA = "a604eda3918b0f8f8d3422d8a5dd53268792a93b59bb5ef9c9bcd9cf228230aa"
KERNEL = "7.3.0-15-qcom-x1e"
FAULTED = {"c5ace5e4-12f0-4e9f-a222-639256685bf8", "4492e975-80dc-4be5-97cd-6de4431d6b5e",
           "311d78af-9cfb-44c3-ac47-30ea92056ad5",
           "b86c3104-05ca-4e40-a913-9226ea801cfb",
           "4f13b3c1-dddf-4b86-9667-b104b7fad629"}
FAULT = re.compile(r"session error received|received system error|video hw is power on|"
                   r"UBSAN:|KASAN:|BUG:|WARNING:|blocked for more than|watchdog:.*lockup")


def require_clean_messages(messages):
    if FAULT.search(messages):
        raise ValueError("kernel/firmware fault: no removal, reload or further hardware")


def journal(*args):
    return subprocess.run(["journalctl", "-k", "-b", "--no-pager", *args],
                          check=True, capture_output=True, text=True, timeout=10).stdout


def activation_helper():
    spec = importlib.util.spec_from_file_location("activation", ROOT / "activate-iris-candidate.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def validate_idle(boot, kernel, build, refcount, power, selected_sha):
    if boot in FAULTED or kernel != KERNEL or build != BUILD or selected_sha != MODULE_SHA:
        raise ValueError("boot/kernel/loaded/selected identity refused")
    if refcount != 0 or power != {"runtime_status": "suspended", "runtime_usage": "0", "control": "auto"}:
        raise ValueError("decoder is busy or not safely runtime-suspended")


def inspect(helper):
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    selected = Path(subprocess.run(["modinfo", "-n", "qcom_iris"], check=True,
                                  capture_output=True, text=True, timeout=5).stdout.strip())
    sha = hashlib.sha256(selected.read_bytes()).hexdigest()
    power = Path("/sys/bus/platform/devices/aa00000.video-codec/power")
    values = {name: (power / name).read_text().strip()
              for name in ("runtime_status", "runtime_usage", "control")}
    build = helper.loaded_build_id()
    refs = int(Path("/sys/module/qcom_iris/refcnt").read_text())
    validate_idle(boot, os.uname().release, build, refs, values, sha)
    return {"boot_id": boot, "build_id": build, "selected_module": str(selected),
            "selected_sha256": sha, "refcount": refs, "power": values}


def command_worker(evidence):
    # Only reached by the operator's root-run process monitor, never by check.
    if os.geteuid() != 0:
        raise SystemExit("root operator required")
    helper = activation_helper()
    before = json.loads((evidence / "before.json").read_text())
    now = inspect(helper)
    if now["boot_id"] != before["boot_id"]:
        raise ValueError("boot changed after preflight")
    require_clean_messages(journal("-o", "cat"))
    cursor = json.loads(journal("-n", "1", "-o", "json").strip())["__CURSOR"]
    subprocess.run(["rmmod", "qcom_iris"], check=True)
    if Path("/sys/module/qcom_iris").exists():
        raise ValueError("module not absent after removal")
    (evidence / "removed.json").write_text(json.dumps({"status": "pass", "boot_id": now["boot_id"]}) + "\n")
    messages = journal("--after-cursor=" + cursor, "-o", "cat")
    (evidence / "after-removal-kernel.log").write_text(messages)
    require_clean_messages(messages)
    subprocess.run(["modprobe", "qcom_iris"], check=True)
    if helper.loaded_build_id() != BUILD:
        raise ValueError("reloaded module identity mismatch")
    (evidence / "reloaded.json").write_text(json.dumps({"status": "pass", "boot_id": now["boot_id"], "build_id": BUILD}) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["check", "run", "worker"])
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    if args.action == "worker":
        command_worker(args.evidence)
        return 0
    helper = activation_helper()
    with helper.open_lease() as lease:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = inspect(helper)
        require_clean_messages(journal("-o", "cat"))
        if args.action == "check":
            print(json.dumps({"status": "ready", "action": "read_only", **state}))
            return 0
        if os.geteuid() != 0:
            raise SystemExit("Run the prepared command in an operator terminal with sudo.")
        args.evidence.mkdir(parents=True, exist_ok=False)
        (args.evidence / "before.json").write_text(json.dumps(state, indent=2) + "\n")
        command = [str(ROOT / "capture-iris-kernel-log.sh"), "--", sys.executable,
                   str(ROOT / "measure-process-tree.py"), "--seconds", "45", "--output",
                   str(args.evidence / "process.json"), "--", sys.executable,
                   str(Path(__file__).resolve()), "worker", str(args.evidence)]
        # The observer retains the inherited lease through the bounded worker.
        with (args.evidence / "kernel-observer.log").open("x") as log:
            status = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT,
                                    pass_fds=(lease.fileno(),)).returncode
        process = json.loads((args.evidence / "process.json").read_text())
        clean = subprocess.run([sys.executable, str(ROOT / "check-playback-performance.py"),
                                "kernel", "--log", str(args.evidence / "kernel-observer.log")]).returncode == 0
        valid = (status == 0 and clean and process.get("exit_status") == 0 and
                 process.get("timed_out") is False and process.get("lingering_descendants") is False and
                 not process.get("unresolved_pids") and not process.get("signal_denied") and
                 (args.evidence / "removed.json").exists() and (args.evidence / "reloaded.json").exists() and
                 helper.loaded_build_id() == BUILD)
        result = {"status": "pass" if valid else "fail", "boot_id": state["boot_id"],
                  "scope": "idle normal module removal/reload only; post-reload decode pending",
                  "no_forced_removal": True, "no_retry": True, "observer_exit_status": status}
        (args.evidence / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result))
        return 0 if valid else 1


if __name__ == "__main__":
    raise SystemExit(main())
