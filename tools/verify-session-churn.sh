#!/usr/bin/env bash
# Regression coverage for the cross-session wedge: sessions that end with
# frames in flight (mid-stream cuts, kills, aborted probes) must not poison
# the NEXT session's CAPTURE bring-up. Every round ends with a full decode
# whose framemd5 must match the reference produced at the start.
#
# All ffmpeg invocations use -nostdin; from automation, ffmpeg otherwise
# parks on a non-EOF stdin under SIGTERM and ignores the timeout.
set -uo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_CHURN_DIR:-/tmp/libva-v4l2-churn}"
cut_frames="${V4L2_VA_CHURN_CUT_FRAMES:-60}"
if [[ ! "$cut_frames" =~ ^[1-9][0-9]*$ ]]; then
    echo "session-churn: fail reason=invalid_cut_frames value=$cut_frames"
    exit 1
fi
gst_env=(GST_VA_ALL_DRIVERS=1 GST_VAAPI_ALL_DRIVERS=1)
reference_md5="$work_dir/reference.md5"
software_md5="$work_dir/software.md5"
expected_frames="$(ffprobe -v error -select_streams v:0 -count_frames \
    -show_entries stream=nb_read_frames -of csv=p=0 "$sample")" || exit 1
if [[ ! "$expected_frames" =~ ^[1-9][0-9]*$ || "$expected_frames" -le "$cut_frames" ]]; then
    echo "session-churn: fail reason=sample_too_short frames=$expected_frames cut=$cut_frames"
    exit 1
fi

mkdir -p "$work_dir"
rm -f "$work_dir"/round-*.md5

pass=0
fail=0
declare -a failed_rounds=()

vaapi_decode() { # <out.md5> [extra ffmpeg args...]
    local out="$1"; shift
    timeout 120s env LIBVA_DRIVERS_PATH="$driver_dir" \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
        -i "$sample" -map 0:v:0 -vf hwdownload,format=nv12 -f framemd5 "$out" "$@" \
        >"$work_dir/last-leg.log" 2>&1
}

full_decode_ok() { # <label> <md5>
    local label="$1" md5="$2"
    if [[ ! -s "$md5" ]]; then
        echo "FAIL [$label]: no output produced"
        return 1
    fi
    python3 "$repo_root/tools/compare-frame-repeats.py" "$software_md5" "$md5" \
        --reference-frames "$expected_frames" || return 1
    return 0
}

note_result() { # <label> <ok>
    if [[ "$2" == ok ]]; then
        pass=$((pass + 1))
        echo "ok   $1"
    else
        fail=$((fail + 1))
        failed_rounds+=("$1")
    fi
}

run_gst_decode() { # <log> [debug]
    local log="$1"
    local debug="${2:-}"
    local env_args=("${gst_env[@]}" "LIBVA_DRIVERS_PATH=$driver_dir")
    if [[ "$debug" == debug ]]; then
        env_args+=("V4L2_VA_DEBUG=1")
    fi
    timeout 60s env "${env_args[@]}" \
        gst-launch-1.0 -q filesrc location="$sample" ! qtdemux name=d d.video_0 ! \
        queue ! h264parse ! vah264dec ! fakesink \
        >"$log" 2>&1
}

# 1. Reference decode (also warms nothing: fresh session).
if ! timeout -k 5s 120s ffmpeg -y -nostdin -hide_banner -v error \
    -c:v h264 -i "$sample" -map 0:v:0 -vf format=nv12 -f framemd5 "$software_md5" \
    > "$work_dir/software.log" 2>&1; then
    echo "FATAL: software reference failed (see $work_dir/software.log)"
    exit 1
fi
if vaapi_decode "$reference_md5" && full_decode_ok "reference" "$reference_md5"; then
    note_result "reference-full-decode" ok
else
    note_result "reference-full-decode" fail
    echo "FATAL: reference decode failed; aborting (see $work_dir/last-leg.log)"
    exit 1
fi

round=0
check() { # <label> — full decode + parity for the round
    local label="$1"
    round=$((round + 1))
    local md5="$work_dir/round-$round.md5"
    if vaapi_decode "$md5" && full_decode_ok "after $label (round $round)" "$md5"; then
        note_result "after-$label" ok
    else
        note_result "after-$label" fail
    fi
}

# 2. mpv mid-stream cut (leaves frames in flight) x3, each followed by GStreamer.
for i in 1 2 3; do
    timeout 90s env LIBVA_DRIVERS_PATH="$driver_dir" \
        mpv --hwdec=vaapi-copy --hwdec-software-fallback=no --vo=null --ao=null --frames="$cut_frames" "$sample" \
        >"$work_dir/mpv-$i.log" 2>&1
    mpv_status=$?
    run_gst_decode "$work_dir/gst-$i.log"
    gst_status=$?
    if [[ $mpv_status == 0 && $gst_status == 0 ]] &&
       rg -q 'Using hardware decoding \(vaapi-copy\)' "$work_dir/mpv-$i.log"; then
        note_result "mpv-cut-followed-by-gst ($i)" ok
    else
        note_result "mpv-cut-followed-by-gst ($i) mpv=$mpv_status gst=$gst_status" fail
    fi
done

# 3. SIGKILL ffmpeg mid-decode (device left with queued work) then recover.
#    The sample decodes in about a second, so 0.5s guarantees a mid-decode kill.
timeout --preserve-status -s KILL 0.5s env LIBVA_DRIVERS_PATH="$driver_dir" \
    ffmpeg -y -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
    -i "$sample" -map 0:v:0 -vf hwdownload,format=nv12 -f framemd5 "$work_dir/killed.md5" \
    >"$work_dir/kill-leg.log" 2>&1
kill_status=$?
if [[ "$kill_status" != 137 ]]; then
    note_result "sigkill-mid-decode not-interrupted status=$kill_status" fail
fi
check "sigkill-mid-decode"

# 4. SIGKILL GStreamer mid-decode then recover.
timeout --preserve-status -s KILL 0.5s env "${gst_env[@]}" LIBVA_DRIVERS_PATH="$driver_dir" \
    gst-launch-1.0 -q filesrc location="$sample" ! qtdemux name=d d.video_0 ! \
    queue ! h264parse ! vah264dec ! fakesink \
    >"$work_dir/gst-kill.log" 2>&1
kill_status=$?
if [[ "$kill_status" != 137 ]]; then
    note_result "sigkill-gst not-interrupted status=$kill_status" fail
fi
check "sigkill-gst"

# 5. SIGTERM GStreamer mid-decode (graceful-ish teardown path) then recover.
timeout -s TERM 0.5s env "${gst_env[@]}" LIBVA_DRIVERS_PATH="$driver_dir" \
    gst-launch-1.0 -q filesrc location="$sample" ! qtdemux name=d d.video_0 ! \
    queue ! h264parse ! vah264dec ! fakesink \
    >"$work_dir/gst-term.log" 2>&1
term_status=$?
if [[ "$term_status" != 124 ]]; then
    note_result "sigterm-gst not-interrupted status=$term_status" fail
fi
check "sigterm-gst"

echo
echo "session-churn: pass=$pass fail=$fail"
if [[ $fail -gt 0 ]]; then
    printf 'failed: %s\n' "${failed_rounds[@]}"
    exit 1
fi
echo "verified: mid-stream cuts, kills, and SIGTERM teardowns do not wedge the next session"
