#!/usr/bin/env python3
"""Fail-closed checks for measured playback, separate from driver-load probes."""
import argparse
import json
import math
from pathlib import Path
import re


def number(value, name, positive=False, integer=False):
    if isinstance(value, bool):
        raise ValueError(f"invalid_{name}")
    try:
        result = float(value)
    except (ValueError, TypeError, OverflowError):
        raise ValueError(f"invalid_{name}") from None
    if not math.isfinite(result) or result < 0 or (positive and result <= 0):
        raise ValueError(f"invalid_{name}")
    if integer and (not result.is_integer() or result > 2**53):
        raise ValueError(f"invalid_{name}")
    return result


def thresholds(minimum_fps, maximum_rss, minimum_seconds, strict):
    fps = number(minimum_fps, "minimum_fps")
    rss = number(maximum_rss, "maximum_rss", integer=True)
    seconds = number(minimum_seconds, "minimum_seconds")
    qualified = fps > 0 and rss > 0 and seconds > 0
    if strict and not qualified:
        raise ValueError("positive_deployment_thresholds_required")
    return fps, rss, seconds, qualified


def check_measurement(frames, elapsed, peak_rss, limits):
    frames = number(frames, "frame_count", positive=True, integer=True)
    elapsed = number(elapsed, "elapsed_time", positive=True)
    peak_rss = number(peak_rss, "peak_rss", positive=True, integer=True)
    minimum_fps, maximum_rss, minimum_seconds, qualified = limits
    fps = frames / elapsed
    if not math.isfinite(fps):
        raise ValueError("invalid_measured_fps")
    if fps < minimum_fps:
        raise ValueError("throughput_below_requirement")
    if maximum_rss and peak_rss > maximum_rss:
        raise ValueError("memory_budget_exceeded")
    if elapsed < minimum_seconds:
        raise ValueError("run_too_short_for_sustained_measurement")
    return {"frames": int(frames), "elapsed_s": elapsed, "fps": fps,
            "peak_rss_kib": int(peak_rss), "performance": "qualified" if qualified else "unqualified"}


def check_kernel_log(log):
    counters = [r"session-fatal\(0x4000003\)", r"system-fatal\(0x5000003\)",
                "power-cycles", "vb2-warns", "other-session", "other-system", "kernel-bugs"]
    pattern = r"^[ \t]*summary: " + r"[ \t]+".join(key + "=0" for key in counters) + r"[ \t]*$"
    summaries = [line for line in log.splitlines() if line.lstrip().startswith("summary:")]
    if not summaries:
        raise ValueError("missing_clean_kernel_evidence")
    if re.search(r"(?:" + "|".join(counters) + r")=[1-9]", log):
        raise ValueError("kernel_or_firmware_fault")
    if any(not re.fullmatch(pattern, line) for line in summaries):
        raise ValueError("incomplete_clean_kernel_evidence")


def check_browser(log, events, measurement, limits, maximum_drop_ratio, run_id):
    """One fresh private profile, one video, actual driver frames and a real seek."""
    if not run_id or measurement.get("run_id") != run_id:
        raise ValueError("stale_or_unbound_browser_measurement")
    if (type(measurement.get("exit_status")) is not int or measurement["exit_status"] != 0
            or measurement.get("timed_out") is not False
            or measurement.get("lingering_descendants") is not False):
        raise ValueError("browser_did_not_exit_cleanly")
    if "msm_drv_video_rs" not in log:
        raise ValueError("driver_not_loaded")
    if re.search(r"IsHardwareAccelerated=(?:0|false)|software_decoder|fall(?:ing)? back to software|"
                 r"switching back to SW decode|"
                 r"va(?:BeginPicture|EndPicture|SyncSurface|CreateSurfaces).*?(?:failed|error)|"
                 r"GPU process.*?(?:crash|exiting)|vaSyncSurface timed out|invalid CAPTURE completion", log, re.I):
        raise ValueError("decoder_error_or_software_fallback")
    completions = len(re.findall(r"msm_drv_video_rs: publish surface=\d+ cap_idx=Some\(\d+\)", log))
    if completions < 30:
        raise ValueError("insufficient_hardware_frame_completions")
    check_kernel_log(log)
    positions = {}
    for i, event in enumerate(events):
        if not isinstance(event, dict) or event.get("event") in ("error", "close_error", "close_still_open"):
            raise ValueError("invalid_or_failed_browser_telemetry")
        if event.get("run_id") != run_id:
            raise ValueError("stale_or_unbound_browser_telemetry")
        name = event.get("event")
        if name in positions:
            raise ValueError("duplicate_browser_milestone")
        positions[name] = i
    names = ["playing", "before_seek", "seek_requested", "seeked", "after_seek", "finished"]
    if any(name not in positions for name in names):
        raise ValueError("missing_playback_or_seek_evidence")
    if [positions[name] for name in names] != sorted(positions[name] for name in names):
        raise ValueError("out_of_order_browser_milestones")
    milestones = {name: events[positions[name]] for name in names}
    for event in milestones.values():
        for key in ("time", "elapsed", "total", "dropped"):
            if type(event.get(key)) not in (int, float):
                raise ValueError(f"invalid_telemetry_{key}")
            number(event.get(key), f"telemetry_{key}", integer=key in ("total", "dropped"))
        if type(event.get("ready")) is not int or event["ready"] not in (3, 4):
            raise ValueError("browser_video_not_ready")
    request = milestones["seek_requested"]
    sought = milestones["seeked"]
    target = number(request.get("target"), "seek_target", positive=True)
    if type(request.get("target")) not in (int, float):
        raise ValueError("invalid_seek_target")
    if abs(sought["time"] - target) > 1:
        raise ValueError("seek_did_not_reach_requested_target")
    before, after, done = (milestones[k] for k in ("before_seek", "after_seek", "finished"))
    if before["total"] - milestones["playing"]["total"] < 30 or after["total"] - sought["total"] < 30:
        raise ValueError("insufficient_playback_before_or_after_seek")
    if abs(request["time"] - target) < 1 or after["time"] - sought["time"] < 1:
        raise ValueError("seek_or_post_seek_playback_did_not_advance")
    previous_total = previous_elapsed = previous_dropped = -1
    for name in names:
        event = milestones[name]
        if (event["total"] < previous_total or event["elapsed"] < previous_elapsed
                or event["dropped"] < previous_dropped or event["dropped"] > event["total"]):
            raise ValueError("inconsistent_browser_counters")
        previous_total, previous_elapsed, previous_dropped = event["total"], event["elapsed"], event["dropped"]
    drop_limit = number(maximum_drop_ratio, "maximum_drop_ratio")
    if drop_limit > 1 or done["total"] <= 0 or done["dropped"] / done["total"] > drop_limit:
        raise ValueError("dropped_frame_budget_exceeded")
    if completions < done["total"]:
        raise ValueError("partial_hardware_frame_evidence")
    collector_elapsed = number(measurement.get("elapsed_s"), "collector_elapsed", positive=True)
    if done["elapsed"] > collector_elapsed + 0.5:
        raise ValueError("telemetry_exceeds_observed_runtime")
    result = check_measurement(done["total"] - done["dropped"], done["elapsed"], measurement.get("peak_rss_kib"), limits)
    result.update(hardware_completions=completions, seek="acknowledged_and_playing", teardown="clean_process_exit")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["thresholds", "kernel", "4k", "browser"])
    parser.add_argument("--minimum-fps", default="0")
    parser.add_argument("--maximum-rss-kib", default="0")
    parser.add_argument("--minimum-seconds", default="0")
    parser.add_argument("--strict", action="store_true")
    parser.add_argument("--frames")
    parser.add_argument("--time-file", type=Path)
    parser.add_argument("--log", type=Path)
    parser.add_argument("--events", type=Path)
    parser.add_argument("--measurement", type=Path)
    parser.add_argument("--maximum-drop-ratio", default="0.01")
    parser.add_argument("--run-id")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        limits = thresholds(args.minimum_fps, args.maximum_rss_kib, args.minimum_seconds, args.strict)
        if args.mode == "thresholds":
            result = {"thresholds": "valid", "performance": "qualified" if limits[3] else "unqualified"}
        elif args.mode == "kernel":
            check_kernel_log(args.log.read_text(errors="replace"))
            result = {"kernel_window": "clean"}
        elif args.mode == "4k":
            elapsed, peak_rss = args.time_file.read_text().split()
            result = check_measurement(args.frames, elapsed, peak_rss, limits)
        else:
            events = [json.loads(line) for line in args.events.read_text().splitlines() if line]
            result = check_browser(args.log.read_text(errors="replace"), events,
                                   json.loads(args.measurement.read_text()), limits, args.maximum_drop_ratio, args.run_id)
        text = json.dumps(result, sort_keys=True, allow_nan=False)
        if args.output:
            args.output.write_text(text + "\n")
        print(text)
    except (ValueError, OSError, TypeError, AttributeError) as error:
        print(f"playback_measurement=fail reason={error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
