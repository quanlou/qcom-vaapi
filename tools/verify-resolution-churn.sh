#!/usr/bin/env bash
set -euo pipefail

# CPU-copy resolution-change gate for Phase 4. Keep this separate from the
# GStreamer dmabuf test: export lifetime belongs to Phase 3, while this probe
# verifies context replacement, teardown draining, and detached CPU snapshots.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
high_sample="${V4L2_VA_RESOLUTION_HIGH_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
low_sample="${V4L2_VA_RESOLUTION_LOW_SAMPLE:-}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_RESOLUTION_DIR:-/tmp/libva-v4l2-resolution}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

for tool in ffmpeg ffprobe; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "resolution_probe=skip reason=missing_tool=$tool"
        exit 77
    fi
done
if [[ ! -f "$high_sample" ]]; then
    echo "resolution_probe=skip reason=missing_sample high=$high_sample"
    exit 77
fi
if [[ ! -x "$kernel_tool" ]]; then
    echo "resolution_probe=skip reason=missing_kernel_tool=$kernel_tool"
    exit 77
fi

mkdir -p "$work_dir"

# The old 720x480 fixture independently triggers a known Iris firmware abort.
# Generate a stable, short alternate-resolution clip unless the caller supplies
# an explicit low sample.
if [[ -z "$low_sample" ]]; then
    low_sample="$work_dir/low-960x640.mp4"
    if ! timeout 90s ffmpeg -y -nostdin -hide_banner -v error \
        -i "$high_sample" -map 0:v:0 -t 3 \
        -vf scale=960:640 -an -c:v libx264 -pix_fmt yuv420p \
        -profile:v high -level:v 4.1 -g 30 -keyint_min 30 -sc_threshold 0 -bf 0 \
        "$low_sample" > "$work_dir/low-build.log" 2>&1; then
        echo "resolution_probe=skip reason=low_sample_build_failed log=$work_dir/low-build.log"
        exit 77
    fi
elif [[ ! -f "$low_sample" ]]; then
    echo "resolution_probe=skip reason=missing_sample low=$low_sample"
    exit 77
fi

frame_count() {
    ffprobe -v error -select_streams v:0 -count_frames \
        -show_entries stream=nb_read_frames -of csv=p=0 "$1" 2>/dev/null
}

low_frames="$(frame_count "$low_sample" || true)"
high_frames="$(frame_count "$high_sample" || true)"
if ! [[ "$low_frames" =~ ^[0-9]+$ && "$high_frames" =~ ^[0-9]+$ ]]; then
    echo "resolution_probe=skip reason=frame_count_failed low=$low_frames high=$high_frames"
    exit 77
fi
expected_frames=$((2 * low_frames + 2 * high_frames))

decode_one() { # <md5> <log>
    timeout 60s env LIBVA_DRIVERS_PATH="$driver_dir" \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_device "$drm_device" \
        -i "$high_sample" -map 0:v:0 -frames:v 1 -f framemd5 "$1" \
        > "$2" 2>&1
}

before_md5="$work_dir/before.md5"
after_md5="$work_dir/after.md5"
if ! decode_one "$before_md5" "$work_dir/before.log" || [[ ! -s "$before_md5" ]]; then
    echo "resolution_probe=skip reason=node_unhealthy_pre_decode log=$work_dir/before.log"
    exit 77
fi

concat_file="$work_dir/concat.txt"
printf "file '%s'\nfile '%s'\nfile '%s'\nfile '%s'\n" \
    "$low_sample" "$high_sample" "$low_sample" "$high_sample" > "$concat_file"
playlist="$work_dir/mixed.ts"
if ! timeout 90s ffmpeg -y -nostdin -hide_banner -v error \
    -f concat -safe 0 -i "$concat_file" -map 0:v:0 -c copy -f mpegts "$playlist" \
    > "$work_dir/playlist-build.log" 2>&1; then
    echo "resolution_probe=skip reason=playlist_build_failed log=$work_dir/playlist-build.log"
    exit 77
fi

log="$work_dir/ffmpeg-resolution.log"
frames_md5="$work_dir/frames.md5"
set +e
timeout 240s "$kernel_tool" -- timeout 210s env \
    LIBVA_DRIVERS_PATH="$driver_dir" V4L2_VA_DEBUG=1 \
    ffmpeg -y -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_device "$drm_device" \
    -i "$playlist" -map 0:v:0 -f framemd5 "$frames_md5" \
    > "$log" 2>&1
status=$?
set -e

decoded="$(awk '!/^#/ && NF {count++} END {print count + 0}' "$frames_md5" 2>/dev/null || true)"
source_changes="$(grep -c 'SOURCE_CHANGE' "$log" || true)"
kernel_line="$(grep -m1 'summary:' "$log" || true)"
session_fatal="$(sed -n 's/.*session-fatal(0x4000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
system_fatal="$(sed -n 's/.*system-fatal(0x5000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
session_fatal="${session_fatal:-NA}"
system_fatal="${system_fatal:-NA}"

post_status=0
decode_one "$after_md5" "$work_dir/after.log" || post_status=$?
sanity=pass
if (( post_status != 0 )) || ! cmp -s "$before_md5" "$after_md5"; then
    sanity=fail
fi

result=pass
reason=completed
if (( status != 0 )); then
    result=fail
    reason="decode_status_$status"
elif (( decoded != expected_frames )); then
    result=fail
    reason="decoded_${decoded}_expected_${expected_frames}"
elif (( source_changes < 4 )); then
    result=fail
    reason="source_changes_${source_changes}_expected_at_least_4"
elif [[ "$session_fatal" != NA ]] && (( session_fatal > 0 )); then
    result=fail
    reason=firmware_session_fatal
elif [[ "$system_fatal" != NA ]] && (( system_fatal > 0 )); then
    result=fail
    reason=firmware_system_fatal
elif [[ "$sanity" != pass ]]; then
    result=fail
    reason=post_decode_failed
fi

low_dims="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$low_sample" 2>/dev/null | tr ',' 'x')"
high_dims="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 "$high_sample" 2>/dev/null | tr ',' 'x')"
echo "resolution_probe=$result reason=$reason dimensions=${low_dims}/${high_dims} decoded=$decoded expected=$expected_frames source_changes=$source_changes sanity=$sanity kernel(session=$session_fatal,system=$system_fatal) log=$log"
[[ "$result" == pass ]]
