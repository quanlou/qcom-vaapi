#!/usr/bin/env bash
set -uo pipefail

# Real seek-storm probe for the Rust V4L2 VA driver (ROADMAP "Flush, seek,
# and recovery" / "seek storm tests do not deadlock").
#
# VA-API has no flush callback; seek behavior reaches the driver as rapid
# surface sync/drop cycles and continuous re-submission, so the probe drives
# REAL seeks through mpv's JSON IPC socket with --hwdec=vaapi-copy:
#   phase 720p   N seeks on the 720p sample
#   phase mixed  M seeks on a 720x480 + 1280x720 mpegts concat, so seeks
#                cross resolution boundaries (context reconfiguration)
# Each phase is wrapped in tools/capture-iris-kernel-log.sh so any iris
# firmware session/system error raised during the storm is attributed to it,
# and the whole storm is bracketed by single-frame framemd5 sanity decodes
# through the driver: if the storm wedged the node, the post-decode fails.
#
# This probe never retries: a wedged node must be reported, not hammered.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
low_sample="${V4L2_VA_RESOLUTION_LOW_SAMPLE:-/home/mq/tmp/vaatest/seq-480p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_SEEK_DIR:-/tmp/libva-v4l2-seek}"
seeks720="${V4L2_VA_SEEK_COUNT_720:-24}"
seeks_mixed="${V4L2_VA_SEEK_COUNT_MIXED:-12}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

mkdir -p "$work_dir"

if [[ ! -f "$sample" ]]; then
    echo "seek_storm=skip reason=missing_sample=$sample"
    exit 77
fi
for tool in mpv ffmpeg ffprobe python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "seek_storm=skip reason=missing_tool=$tool"
        exit 77
    fi
done
if [[ ! -f "$kernel_tool" ]]; then
    echo "seek_storm=skip reason=missing_kernel_tool=$kernel_tool"
    exit 77
fi
# No grep -q here: under pipefail its early exit SIGPIPEs the large
# --list-options producer and the pipeline wrongly reads as failure.
if ! mpv --list-options 2>/dev/null | grep 'input-ipc-server' >/dev/null; then
    echo "seek_storm=skip reason=mpv-ipc-unavailable"
    exit 77
fi
# Every child below (mpv storm, ffmpeg sanity decodes, mixed-ts build) must
# hit the Rust driver; export once so the exported run_storm() sees it too.
export LIBVA_DRIVERS_PATH="$driver_dir"

mixed_ts="$work_dir/mixed.ts"
build_mixed=0
if [[ -f "$low_sample" ]]; then
    printf "file '%s'\nfile '%s'\n" "$low_sample" "$sample" \
        > "$work_dir/concat.txt"
    set +e
    timeout 60s ffmpeg -nostdin -hide_banner -v error \
        -f concat -safe 0 -i "$work_dir/concat.txt" \
        -c copy -f mpegts "$mixed_ts" \
        > "$work_dir/concat.log" 2>&1
    concat_status=$?
    set -e
    if [[ "$concat_status" -eq 0 && -s "$mixed_ts" ]]; then
        build_mixed=1
    fi
fi

# Pre-storm sanity through the driver; also proves the node is healthy
# before the storm, so a post-storm failure is attributable to the storm.
before_md5="$work_dir/before.md5"
set +e
timeout 60s env LIBVA_DRIVERS_PATH="$driver_dir" \
    ffmpeg -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_device "$drm_device" \
    -i "$sample" -map 0:v:0 -frames:v 1 -f framemd5 "$before_md5" \
    > "$work_dir/before.log" 2>&1
pre_status=$?
set -e
if [[ "$pre_status" -ne 0 || ! -s "$before_md5" ]]; then
    echo "seek_storm=skip reason=node_unhealthy_pre_decode status=$pre_status log=$work_dir/before.log"
    exit 77
fi

# Runs inside the kernel-log wrapper via bash -c; returns mpv's status, or
# 124 when mpv had to be killed for not exiting after quit.
run_storm() { # <file> <count> <sock> <log>
    local file="$1" count="$2" sock="$3" log="$4"
    local duration drive_rc i
    duration="$(ffprobe -v error -show_entries format=duration \
        -of csv=p=0 "$file" 2>/dev/null || echo 0)"
    rm -f "$sock"
    mpv --no-config --hwdec=vaapi-copy --vo=null --ao=null --loop=inf \
        --input-ipc-server="$sock" --no-terminal \
        "$file" > "$log" 2>&1 &
    local pid=$!
    python3 "$STORM_DRIVER" "$sock" "$count" "$duration" >> "$log" 2>&1
    drive_rc=$?
    for i in $(seq 1 30); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 1
    done
    if kill -0 "$pid" 2>/dev/null; then
        kill -TERM "$pid" 2>/dev/null
        sleep 3
        kill -KILL "$pid" 2>/dev/null
        wait "$pid" 2>/dev/null
        return 124
    fi
    wait "$pid"
    return "$?"
}
export -f run_storm
export STORM_DRIVER="$repo_root/tools/mpv_seek_drive.py"

sock="$work_dir/mpv-ipc.sock"
declare -a phase_results=()
overall=0

storm_phase() { # <name> <file> <count>
    local name="$1" file="$2" count="$3"
    local phase_log="$work_dir/$name.log"
    local mpv_log="$work_dir/$name-mpv.log"
    local status kernel_line session_fatal system_fatal verdict

    set +e
    timeout 180s "$kernel_tool" -- \
        bash -c 'run_storm "$@"' _ "$file" "$count" "$sock" "$mpv_log" \
        > "$phase_log" 2>&1
    status=$?
    set -e

    kernel_line="$(grep -m1 'summary:' "$phase_log" || true)"
    session_fatal="$(sed -n 's/.*session-fatal(0x4000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    system_fatal="$(sed -n 's/.*system-fatal(0x5000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    [[ -n "$session_fatal" ]] || session_fatal=NA
    [[ -n "$system_fatal" ]] || system_fatal=NA

    if [[ "$status" -eq 124 || "$status" -eq 137 ]]; then
        verdict="fail reason=mpv_hung status=$status"
    elif [[ "$system_fatal" != NA && "$system_fatal" -gt 0 ]]; then
        verdict="fail reason=firmware_system_fatal status=$status"
    elif [[ "$status" -ne 0 ]]; then
        verdict="fail reason=mpv_failed status=$status"
    elif [[ "$session_fatal" != NA && "$session_fatal" -gt 0 ]]; then
        verdict="degraded reason=session_abort_rescued status=0"
    else
        verdict="ok status=0"
    fi
    phase_results+=("$name=$verdict kernel(session=$session_fatal,system=$system_fatal) seeks=$count log=$phase_log")
    if [[ "$verdict" == fail* ]]; then
        overall=1
        return 1
    fi
    return 0
}

storm_phase 720p "$sample" "$seeks720" || true
if (( overall == 0 )) && (( build_mixed == 1 )); then
    storm_phase mixed "$mixed_ts" "$seeks_mixed" || true
elif (( build_mixed == 0 )); then
    phase_results+=("mixed=skipped reason=mixed_ts_unavailable")
fi

# Post-storm sanity: the same single-frame decode must still pass and match.
after_md5="$work_dir/after.md5"
sanity="ok"
set +e
timeout 60s env LIBVA_DRIVERS_PATH="$driver_dir" \
    ffmpeg -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_device "$drm_device" \
    -i "$sample" -map 0:v:0 -frames:v 1 -f framemd5 "$after_md5" \
    > "$work_dir/after.log" 2>&1
post_status=$?
set -e
if [[ "$post_status" -ne 0 || ! -s "$after_md5" ]] || ! cmp -s "$after_md5" "$before_md5"; then
    sanity="fail"
    overall=1
fi

for r in "${phase_results[@]}"; do
    echo "seek_phase $r"
done
if (( overall == 0 )); then
    echo "seek_storm=pass sanity=$sanity"
    exit 0
fi
echo "seek_storm=fail sanity=$sanity"
exit 1
