#!/usr/bin/env python3
"""Record an operator's idle system-sleep cycle; never initiate sleep or unload.

This witness alone does not qualify post-resume decoding or active playback.
"""
import argparse
import fcntl
import json
from pathlib import Path
import re
import struct
import subprocess

BUILD = "231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2"
FAULTED = {"c5ace5e4-12f0-4e9f-a222-639256685bf8", "4492e975-80dc-4be5-97cd-6de4431d6b5e",
           "311d78af-9cfb-44c3-ac47-30ea92056ad5"}
FAULT = re.compile(r"session error received|received system error|video hw is power on|"
                   r"UBSAN:|KASAN:|BUG:|WARNING:|blocked for more than|watchdog:.*lockup")


def snapshot():
    boot = Path("/proc/sys/kernel/random/boot_id").read_text().strip()
    if boot in FAULTED:
        raise ValueError("known faulted boot")
    note = Path("/sys/module/qcom_iris/notes/.note.gnu.build-id").read_bytes()
    namesz, size, kind = struct.unpack_from("<III", note)
    start = 12 + ((namesz + 3) & ~3)
    build = note[start:start + size].hex()
    if kind != 3 or note[12:12 + namesz] != b"GNU\0" or build != BUILD:
        raise ValueError("loaded candidate identity mismatch")
    power = Path("/sys/bus/platform/devices/aa00000.video-codec/power")
    state = {name: (power / name).read_text().strip()
             for name in ("runtime_status", "runtime_usage", "control")}
    if Path("/sys/module/qcom_iris/refcnt").read_text().strip() != "0":
        raise ValueError("decoder clients still active; idle witness refused")
    if state != {"runtime_status": "suspended", "runtime_usage": "0", "control": "auto"}:
        raise ValueError("decoder is not idle with runtime autosuspend")
    return {"boot_id": boot, "build_id": build, "power": state,
            "sleep_success": int(Path("/sys/power/suspend_stats/success").read_text()),
            "pm_test": Path("/sys/power/pm_test").read_text().strip(),
            "sleep_modes": Path("/sys/power/mem_sleep").read_text().strip()}


def verify_witness(before, after, messages):
    if any(state.get("pm_test") is None or "[none]" not in state["pm_test"]
           for state in (before, after)) or "suspend debug: Waiting" in messages:
        raise ValueError("PM test mode is not a genuine system-sleep witness")
    if before["boot_id"] != after["boot_id"] or before["build_id"] != after["build_id"]:
        raise ValueError("boot or module changed during observation")
    if after["sleep_success"] <= before["sleep_success"]:
        raise ValueError("no successful system-sleep cycle recorded")
    if FAULT.search(messages):
        raise ValueError("kernel/firmware warning, fault or hung task; stop hardware")
    entry = re.search(r"PM: suspend entry \(([^)]+)\)", messages)
    if not entry or "PM: suspend exit" not in messages[entry.end():]:
        raise ValueError("missing completed suspend/resume journal window")
    return {"status": "pass", "sleep_mode": entry.group(1),
            "scope": "idle system-sleep witness only; post-resume decode and active playback pending"}


def journal(*args, boot="0"):
    return subprocess.run(["journalctl", "-k", "-b", boot.replace("-", ""), "--no-pager", *args],
                          check=True, capture_output=True, text=True, timeout=10).stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["prepare", "verify"])
    parser.add_argument("evidence", type=Path)
    args = parser.parse_args()
    with Path("/tmp/libva-v4l2-hardware.lock").open("w") as lease:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = snapshot()
        if args.action == "prepare":
            if "[none]" not in state["pm_test"]:
                raise ValueError("PM test mode active; real sleep witness refused")
            args.evidence.mkdir(parents=True, exist_ok=False)
            # Refuse already faulted runtime before asking for an operator cycle.
            messages = journal("-o", "cat")
            if FAULT.search(messages):
                raise ValueError("current boot journal contains a fault; do not proceed")
            last = json.loads(journal("-n", "1", "-o", "json").strip())
            state["journal_cursor"] = last["__CURSOR"]
            (args.evidence / "before.json").write_text(json.dumps(state, indent=2) + "\n")
            print("idle_sleep_observer=prepared; no sleep or module operation requested")
        else:
            before = json.loads((args.evidence / "before.json").read_text())
            messages = journal("--after-cursor=" + before["journal_cursor"], "-o", "cat",
                               boot=before["boot_id"])
            with (args.evidence / "journal-after.log").open("x") as stream:
                stream.write(messages)
            with (args.evidence / "after.json").open("x") as stream:
                json.dump(state, stream, indent=2)
            try:
                result = verify_witness(before, state, messages)
            except ValueError as error:
                result = {"status": "fail", "reason": str(error),
                          "scope": "operator system-sleep witness; no support claim"}
            with (args.evidence / "result.json").open("x") as stream:
                json.dump(result, stream, indent=2)
            print(json.dumps(result))
            return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
