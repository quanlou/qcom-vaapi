#!/usr/bin/env python3
"""Run bounded 4K/browser checks on a successfully qualified immutable packet."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone

FAULT = re.compile(r"(?:session-fatal\(0x4000003\)|system-fatal\(0x5000003\)|power-cycles|"
                   r"vb2-warns|other-session|other-system|kernel-bugs)=[1-9]")


def digest(path):
    with path.open("rb") as source:
        result = hashlib.file_digest(source, "sha256")
    return result.hexdigest()


def source_files(root):
    files = [root / "rust/Cargo.toml", root / "rust/Cargo.lock"]
    for name in ("rust/src", "tools", "kernel"):
        files.extend(path for path in (root / name).rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts)
    return {str(path.relative_to(root)): digest(path) for path in sorted(files)}


def validate_packet(packet):
    identity = json.loads((packet / "identity.json").read_text())
    source = json.loads((packet / "source-sha256.json").read_text())
    harness = json.loads((packet / "harness-sha256.json").read_text())
    if identity.get("source_files") != source:
        raise ValueError("source_identity_manifest_mismatch")
    if source_files(packet / "source") != source or source_files(packet / "harness") != harness:
        raise ValueError("packet_source_or_harness_changed")
    if source_files(Path(identity["source_root"])) != source:
        raise ValueError("live_source_changed_since_packet_capture")
    if digest(packet / "driver/msm_drv_video.so") != identity["driver_sha256"]:
        raise ValueError("packet_driver_changed")
    if "tools/run-real-use-qualification.py" not in source:
        raise ValueError("fresh_packet_with_updated_real_use_runner_required")
    return identity


def validate_gate(packet, gate, driver_sha):
    if not gate.is_relative_to(packet):
        raise ValueError("gate_results_must_belong_to_packet")
    log = (gate / "qualification.log").read_text(errors="replace")
    if not re.search(r"^production=pass scope=", log, re.M) or FAULT.search(log):
        raise ValueError("clean_successful_packet_gate_required")
    if digest(gate / "driver/msm_drv_video.so") != driver_sha:
        raise ValueError("gate_driver_identity_mismatch")
    provenance = json.loads((gate / "results/provenance.json").read_text())
    if provenance.get("driver", {}).get("sha256") != driver_sha:
        raise ValueError("gate_driver_provenance_mismatch")


def sustained_loops(measurement, source_frames):
    fps = measurement.get("fps")
    if type(fps) not in (int, float) or not math.isfinite(fps) or fps <= 0:
        raise ValueError("invalid_calibration_fps")
    if type(source_frames) is not int or source_frames < 30:
        raise ValueError("invalid_fixture_frame_count")
    loops = math.ceil(fps * 70 / source_frames)
    if loops > 100:
        raise ValueError("sustained_duration_exceeds_fixture_loop_cap")
    return max(1, loops)


def stage_browser_driver(packet, destination, expected):
    destination.mkdir(parents=True, exist_ok=True)
    staged = destination / "msm_drv_video.so"
    shutil.copy2(packet / "driver/msm_drv_video.so", staged)
    if digest(staged) != expected:
        raise ValueError("snap_staged_driver_identity_mismatch")


def kernel_evidence(harness, paths):
    if not paths:
        raise ValueError("missing_probe_kernel_logs")
    for path in paths:
        if FAULT.search(path.read_text(errors="replace")):
            raise ValueError("kernel_fault_stop:" + str(path))
        subprocess.run([sys.executable, str(harness / "tools/check-playback-performance.py"),
                        "kernel", "--log", str(path)], check=True, timeout=10)


def run_phase(packet, run, phase, script, driver, env, bound):
    log_path = run / (phase + ".log")
    observer = packet / "harness/tools/measure-process-tree.py"
    command = [sys.executable, str(observer), "--seconds", str(bound),
               "--output", str(run / (phase + "-process.json")), "--", "bash", str(script), str(driver)]
    with log_path.open("w") as log:
        result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT)
    (run / (phase + ".status")).write_text(str(result.returncode) + "\n")
    print(f"real_use_phase={phase} status={result.returncode} log={log_path}", flush=True)
    return result.returncode


def browser_scope(scope):
    if scope == "chromium-headless":
        return {"scope": scope, "browsers": ["chromium"],
                "deferred_unsupported": ["firefox"], "codecs": ["h264", "hevc", "vp9"]}
    if scope == "all":
        return {"scope": scope, "browsers": ["chromium", "firefox"],
                "deferred_unsupported": [], "codecs": ["h264", "hevc", "vp9"]}
    raise ValueError("unknown_qualification_scope")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("packet", type=Path)
    parser.add_argument("--gate-results", type=Path, required=True)
    parser.add_argument("--sample-4k", type=Path, default=Path("/home/mq/tmp/vaatest/quality4k/h264-2160p.mp4"))
    parser.add_argument("--sample-browser", type=Path, default=Path("/home/mq/tmp/vaatest/test_720p.mp4"))
    parser.add_argument("--output-root", type=Path, default=Path.home() / ".cache/libva-v4l2-qualification/real-use")
    parser.add_argument("--scope", choices=("all", "chromium-headless"), default="all",
                        help="Explicit supported browser scope; preserves all 4K and kernel gates")
    args = parser.parse_args()
    packet = args.packet.resolve()
    run = None
    try:
        identity = validate_packet(packet)
        validate_gate(packet, args.gate_results.resolve(), identity["driver_sha256"])
        fixtures = {str(path.resolve()): digest(path) for path in (args.sample_4k, args.sample_browser)}
        args.output_root.mkdir(parents=True, exist_ok=True)
        run = Path(tempfile.mkdtemp(prefix="run.", dir=args.output_root.resolve()))
        (run / "scratch").mkdir()
        scope = browser_scope(args.scope)
        (run / "qualification-scope.json").write_text(json.dumps(scope, indent=2) + "\n")
        for name in ("identity.json", "source-sha256.json", "harness-sha256.json"):
            shutil.copy2(packet / name, run / name)
        (run / "fixture-sha256.json").write_text(json.dumps(fixtures, indent=2) + "\n")
        print("real_use_results=" + str(run), flush=True)
        env = {**os.environ, "TMPDIR": str(run / "scratch"), "LIBVA_DRIVER_NAME": "msm"}
        harness = packet / "harness"

        def check_identity(label="between_phases"):
            if validate_packet(packet) != identity:
                raise ValueError("packet_identity_changed_during_run")
            if any(digest(Path(path)) != sha for path, sha in fixtures.items()):
                raise ValueError("fixture_changed_during_run")
            validate_gate(packet, args.gate_results.resolve(), identity["driver_sha256"])
            with (run / "identity-checks.jsonl").open("a") as checks:
                checks.write(json.dumps({"phase": label, "time_utc": datetime.now(timezone.utc).isoformat(),
                                         "driver_sha256": digest(packet / "driver/msm_drv_video.so"),
                                         "source_manifest_sha256": digest(packet / "source-sha256.json"),
                                         "harness_manifest_sha256": digest(packet / "harness-sha256.json"),
                                         "live_source": "matches_packet", "fixtures": "unchanged"}) + "\n")

        def four_k(phase, loops, strict, seconds):
            check_identity(phase + "_before")
            result = run_phase(packet, run, phase, harness / "tools/verify-4k-decode.sh", packet / "driver", {
                **env, "V4L2_VA_4K_CODEC": "h264", "V4L2_VA_4K_SAMPLE": str(args.sample_4k.resolve()),
                "V4L2_VA_4K_LOOPS": str(loops), "V4L2_VA_4K_STRICT": str(strict),
                "V4L2_VA_4K_MIN_FPS": "30", "V4L2_VA_4K_MAX_RSS_KIB": "524288",
                "V4L2_VA_4K_MIN_SECONDS": str(seconds), "V4L2_VA_4K_LOG_DIR": str(run / phase),
            }, 400)
            check_identity(phase + "_after")
            logs = [run / phase / f"{kind}-{leg}.log" for leg in ("1", "30", "full") for kind in ("native", "driver")]
            kernel_evidence(harness, logs)  # Missing observation stops subsequent hardware phases.
            return result

        overall = 0
        if four_k("h264-calibration", 5, 0, 0) != 0:
            overall = 1  # Budget failure does not authorize a sustained run.
            print("real_use_sustained=not_run reason=calibration_failed", flush=True)
        else:
            probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-count_frames",
                                    "-show_entries", "stream=nb_read_frames", "-of", "csv=p=0",
                                    str(args.sample_4k.resolve())], env=env, capture_output=True, text=True,
                                   timeout=120, check=True)
            measurement = json.loads((run / "h264-calibration/performance.json").read_text())
            loops = sustained_loops(measurement, int(probe.stdout.strip()))
            if four_k("h264-sustained", loops, 1, 60) != 0:
                overall = 1

        # These tools acquire the common hardware lease per invocation; do not
        # hold an outer flock and deadlock their independently guarded probes.
        for browser in scope["browsers"]:
            check_identity(browser + "_before")
            common = Path.home() / "snap" / browser / "common/libva-v4l2-real-use"
            common.mkdir(parents=True, exist_ok=True)
            private = Path(tempfile.mkdtemp(prefix="run.", dir=common))
            driver = private / "driver"
            stage_browser_driver(packet, driver, identity["driver_sha256"])
            (run / (browser + "-artifact-path.txt")).write_text(str(private) + "\n")
            result = run_phase(packet, run, browser, harness / "tools/verify-browser-vaapi.sh", driver, {
                **env, "V4L2_VA_BROWSER": browser, "V4L2_VA_BROWSER_KIND": browser,
                "V4L2_VA_BROWSER_CHROMIUM_MODE": "native", "V4L2_VA_BROWSER_WORK_DIR": str(private),
                "V4L2_VA_SAMPLE": str(args.sample_browser.resolve()), "V4L2_VA_BROWSER_STRICT": "1",
                "V4L2_VA_BROWSER_MIN_FPS": "27", "V4L2_VA_BROWSER_MAX_RSS_KIB": "2097152",
                "V4L2_VA_BROWSER_MIN_SECONDS": "20", "V4L2_VA_BROWSER_SECONDS": "30",
                "V4L2_VA_BROWSER_MAX_DROP_RATIO": "0.01",
            }, 50)
            check_identity(browser + "_after")
            if digest(driver / "msm_drv_video.so") != identity["driver_sha256"]:
                raise ValueError("browser_driver_changed_during_run")
            kernel_evidence(harness, list(private.glob(f"run.*/{browser}.log")) + list(private.glob(f"run-*/{browser}.log")))
            if result:
                overall = 1
        check_identity("completed")
        print(f"real_use={'pass' if overall == 0 else 'fail'} scope={args.scope} driver_sha256={identity['driver_sha256']} results={run}")
        return overall
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        print(f"real_use=stop reason={error} results={run}", flush=True)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
