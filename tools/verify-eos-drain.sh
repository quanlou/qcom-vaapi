#!/usr/bin/env bash
set -uo pipefail

# Dedicated EOS and teardown-drain regression for the Rust V4L2 VA driver
# (ROADMAP: "cover normal EOS and teardown drain behavior with a dedicated
# regression sample" while DECODER_CMD STOP stays limited to explicit
# teardown/recovery drains).
#
#   leg A natural EOS  full-sample framemd5 through the driver under
#                      V4L2_VA_DEBUG=1; the md5 becomes the reference. A
#                      legitimate end-of-stream must NOT arm abort recovery,
#                      so the log must contain none of the recovery-armed
#                      markers ("anomalous EOS without drain", "empty
#                      CAPTURE without drain").
#   leg B mid-cut      mpv --frames=N exits mid-stream with pipelined work;
#                      the teardown flush ("teardown flush done" under
#                      V4L2_VA_DEBUG=1) must bound the drain. Marker count
#                      is reported (mpv may already have synced everything,
#                      in which case flush is a no-op), but the REQUIRED
#                      assertion is the cross-session one: the IMMEDIATELY
#                      following full decode must succeed and match the
#                      reference — that is exactly the next-session CAPTURE
#                      STREAMON EIO wedge the teardown drain was built for.
#   drain-then-resubmit is covered by tools/verify-seek-storm.sh (submit.rs
#   resets `draining` on new submissions); it is referenced, not duplicated.
#
# Each leg runs under tools/capture-iris-kernel-log.sh and the whole probe
# is bracketed by single-frame framemd5 sanity decodes. No retries: a wedged
# node is reported, never hammered.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_EOS_DIR:-/tmp/libva-v4l2-eos-drain}"
cut_frames="${V4L2_VA_EOS_CUT_FRAMES:-45}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

mkdir -p "$work_dir"

if [[ ! -f "$sample" ]]; then
    echo "eos_drain=skip reason=missing_sample=$sample"
    exit 77
fi
for tool in ffmpeg ffprobe mpv; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "eos_drain=skip reason=missing_tool=$tool"
        exit 77
    fi
done
if [[ ! -f "$kernel_tool" ]]; then
    echo "eos_drain=skip reason=missing_kernel_tool=$kernel_tool"
    exit 77
fi
export LIBVA_DRIVERS_PATH="$driver_dir"

# A full independent reference prevents header-only or equally truncated
# decodes from certifying natural EOS and cross-session recovery.
expected_frames="$(ffprobe -v error -select_streams v:0 -count_frames \
    -show_entries stream=nb_read_frames -of csv=p=0 "$sample")" || exit 1
if [[ ! "$expected_frames" =~ ^[1-9][0-9]*$ || ! "$cut_frames" =~ ^[1-9][0-9]*$ ]] ||
   (( expected_frames <= cut_frames )); then
    echo "eos_drain=fail reason=invalid_frame_or_cut_count frames=$expected_frames cut=$cut_frames"
    exit 1
fi
software_md5="$work_dir/software.md5"
if ! timeout -k 5s 120s ffmpeg -y -nostdin -hide_banner -v error \
    -c:v h264 -i "$sample" -map 0:v:0 -vf format=nv12 -threads:v 1 -f framemd5 "$software_md5" \
    > "$work_dir/software.log" 2>&1; then
    echo "eos_drain=fail reason=software_reference_failed"
    exit 1
fi

# Node-health bracket: single-frame decode before and after, byte-compared.
sanity_md5() { # <out.md5> <log>
    timeout -k 5s 60s "$kernel_tool" -- ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
        -i "$sample" -map 0:v:0 -frames:v 1 -vf hwdownload,format=nv12 -f framemd5 "$1" \
        > "$2" 2>&1 || return $?
    python3 "$repo_root/tools/check-playback-performance.py" kernel --log "$2" || return $?
    [[ "$(awk '/^[0-9]+,/ {n++} END {print n+0}' "$1")" == 1 ]]
}

before_md5="$work_dir/before.md5"
set +e
sanity_md5 "$before_md5" "$work_dir/before.log"
pre_status=$?
set -e
if [[ "$pre_status" -ne 0 || ! -s "$before_md5" ]]; then
    echo "eos_drain=skip reason=node_unhealthy_pre_decode status=$pre_status log=$work_dir/before.log"
    exit 77
fi

# Kernel attribution for one wrapped leg; prints the phase verdict line.
kernel_counts() { # <log>
    local kernel_line session_fatal system_fatal
    kernel_line="$(grep -m1 'summary:' "$1" || true)"
    session_fatal="$(sed -n 's/.*session-fatal(0x4000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    system_fatal="$(sed -n 's/.*system-fatal(0x5000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    [[ -n "$session_fatal" ]] || session_fatal=NA
    [[ -n "$system_fatal" ]] || system_fatal=NA
    echo "$session_fatal $system_fatal"
}

overall=0

# Never open another decoder after a failed or unobserved hardware leg.
# Command success alone does not establish that the firmware stayed healthy.
require_clean_leg() { # <phase> <command-status> <kernel-log>
    if [[ "$2" -ne 0 ]] || ! python3 "$repo_root/tools/check-playback-performance.py" kernel --log "$3"; then
        echo "eos_drain=fail stopped_after=$1 reason=command_or_kernel_evidence status=$2 log=$3"
        exit 1
    fi
}

# ---------- leg A: natural end-of-stream ----------
leg_a_log="$work_dir/eos-full.log"
ref_md5="$work_dir/reference.md5"
set +e
timeout -k 5s 150s "$kernel_tool" -- \
    env V4L2_VA_DEBUG=1 ffmpeg -y -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
    -i "$sample" -map 0:v:0 -vf hwdownload,format=nv12 -f framemd5 "$ref_md5" \
    > "$leg_a_log" 2>&1
eos_status=$?
set -e
require_clean_leg natural_eos "$eos_status" "$leg_a_log"
read -r a_session a_system <<< "$(kernel_counts "$leg_a_log")"
recovery_armed=0
if grep -q 'recovery armed\|anomalous EOS without drain\|empty CAPTURE without drain' "$leg_a_log"; then
    recovery_armed=1
fi

eos_verdict="ok"
if [[ "$eos_status" -ne 0 || ! -s "$ref_md5" ]]; then
    eos_verdict="fail reason=eos_decode_failed status=$eos_status"
    overall=1
elif ! python3 "$repo_root/tools/compare-frame-repeats.py" "$software_md5" "$ref_md5" --reference-frames "$expected_frames"; then
    eos_verdict="fail reason=incomplete_or_incorrect_eos_frames"
    overall=1
elif [[ "$recovery_armed" -ne 0 ]]; then
    eos_verdict="fail reason=eos_armed_abort_recovery"
    overall=1
elif [[ "$a_system" != NA && "$a_system" -gt 0 ]]; then
    eos_verdict="fail reason=firmware_system_fatal status=$eos_status"
    overall=1
elif [[ "$a_session" != NA && "$a_session" -gt 0 ]]; then
    eos_verdict="degraded reason=session_abort_rescued status=$eos_status"
fi
echo "eos_leg $eos_verdict kernel(session=$a_session,system=$a_system) log=$leg_a_log"
if (( overall != 0 )); then
    echo "eos_drain=fail stopped_after=natural_eos reason=invalid_eos_output"
    exit 1
fi

# ---------- leg B: mid-stream cut, then immediate next-session decode ----------
cut_log="$work_dir/mpv-cut.log"
set +e
timeout -k 5s 120s "$kernel_tool" -- \
    env V4L2_VA_DEBUG=1 mpv --no-config --hwdec=vaapi-copy --hwdec-software-fallback=no --vo=null --ao=null \
    --frames="$cut_frames" --log-file="$work_dir/mpv-cut-driver.log" --no-terminal "$sample" \
    > "$cut_log" 2>&1
cut_status=$?
set -e
require_clean_leg mid_cut "$cut_status" "$cut_log"
read -r b_session b_system <<< "$(kernel_counts "$cut_log")"
flush_markers="$(grep -c 'teardown flush done pending=' "$cut_log" || true)"
stop_markers="$(grep -c 'DECODER_CMD STOP drain started' "$cut_log" || true)"

cut_verdict="ok"
if [[ "$cut_status" -ne 0 ]] || ! grep -q 'Using hardware decoding (vaapi-copy)' "$work_dir/mpv-cut-driver.log"; then
    cut_verdict="fail reason=mpv_cut_failed status=$cut_status"
    overall=1
elif [[ "$b_system" != NA && "$b_system" -gt 0 ]]; then
    cut_verdict="fail reason=firmware_system_fatal status=$cut_status"
    overall=1
elif [[ "$b_session" != NA && "$b_session" -gt 0 ]]; then
    cut_verdict="degraded reason=session_abort_rescued status=$cut_status"
fi
echo "cut_leg $cut_verdict kernel(session=$b_session,system=$b_system) teardown_flush=$flush_markers stop_drain=$stop_markers frames=$cut_frames log=$cut_log"
if (( overall != 0 )); then
    echo "eos_drain=fail stopped_after=mid_cut reason=invalid_cut_output"
    exit 1
fi

# The required cross-session assertion: the immediately following full
# decode must succeed and match the leg-A reference byte-for-byte.
post_md5="$work_dir/post-cut.md5"
set +e
timeout -k 5s 150s "$kernel_tool" -- \
    env V4L2_VA_DEBUG=1 ffmpeg -y -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
    -i "$sample" -map 0:v:0 -vf hwdownload,format=nv12 -f framemd5 "$post_md5" \
    > "$work_dir/post-cut.log" 2>&1
post_status=$?
set -e
require_clean_leg post_cut "$post_status" "$work_dir/post-cut.log"
read -r c_session c_system <<< "$(kernel_counts "$work_dir/post-cut.log")"

parity_verdict="ok"
if [[ "$post_status" -ne 0 || ! -s "$post_md5" ]]; then
    parity_verdict="fail reason=post_cut_decode_failed status=$post_status"
    overall=1
elif ! python3 "$repo_root/tools/compare-frame-repeats.py" "$software_md5" "$post_md5" --reference-frames "$expected_frames"; then
    parity_verdict="fail reason=post_cut_parity_mismatch"
    overall=1
elif [[ "$c_system" != NA && "$c_system" -gt 0 ]]; then
    parity_verdict="fail reason=firmware_system_fatal status=$post_status"
    overall=1
elif [[ "$c_session" != NA && "$c_session" -gt 0 ]]; then
    parity_verdict="degraded reason=session_abort_rescued status=$post_status"
fi
echo "post_cut_leg $parity_verdict kernel(session=$c_session,system=$c_system) log=$work_dir/post-cut.log"
if (( overall != 0 )); then
    echo "eos_drain=fail stopped_after=post_cut reason=invalid_post_cut_output"
    exit 1
fi

# ---------- closing sanity bracket ----------
after_md5="$work_dir/after.md5"
sanity="ok"
set +e
sanity_md5 "$after_md5" "$work_dir/after.log"
post_sanity_status=$?
set -e
if [[ "$post_sanity_status" -ne 0 || ! -s "$after_md5" ]] || ! cmp -s "$after_md5" "$before_md5"; then
    sanity="fail"
    overall=1
fi

if (( overall == 0 )); then
    echo "eos_drain=pass sanity=$sanity"
    exit 0
fi
echo "eos_drain=fail sanity=$sanity"
exit 1
