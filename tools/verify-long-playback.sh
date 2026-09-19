#!/usr/bin/env bash
set -euo pipefail

# Sustained CPU-copy playback gate for Phase 4. The generated MPEG-TS playlist
# concatenates many complete clips in one FFmpeg run. Mixed-resolution changes
# are covered independently by verify-resolution-churn.sh, so a firmware DRC
# defect cannot mask basic long-playback lifetime regressions here.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
high_sample="${V4L2_VA_RESOLUTION_HIGH_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
segments="${V4L2_VA_LONG_SEGMENTS:-12}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_LONG_DIR:-/tmp/libva-v4l2-long-playback}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

if ! [[ "$segments" =~ ^[0-9]+$ ]] || (( segments < 4 )); then
    echo "long_playback=skip reason=invalid_segment_count value=$segments"
    exit 77
fi
for tool in ffmpeg ffprobe; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "long_playback=skip reason=missing_tool=$tool"
        exit 77
    fi
done
if [[ ! -f "$high_sample" ]]; then
    echo "long_playback=skip reason=missing_sample high=$high_sample"
    exit 77
fi
if [[ ! -x "$kernel_tool" ]]; then
    echo "long_playback=skip reason=missing_kernel_tool"
    exit 77
fi

mkdir -p "$work_dir"

decode_one() { # <md5> <log>
    timeout 60s env LIBVA_DRIVERS_PATH="$driver_dir" \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_device "$drm_device" \
        -i "$high_sample" -map 0:v:0 -frames:v 1 -f framemd5 "$1" \
        > "$2" 2>&1
}

before_md5="$work_dir/before.md5"
after_md5="$work_dir/after.md5"
if ! decode_one "$before_md5" "$work_dir/before.log" || ! [[ -s "$before_md5" ]]; then
    echo "long_playback=skip reason=node_unhealthy_pre_decode log=$work_dir/before.log"
    exit 77
fi

concat_file="$work_dir/concat.txt"
: > "$concat_file"
expected_frames=0
for ((i = 0; i < segments; i++)); do
    sample="$high_sample"
    printf "file '%s'\n" "$sample" >> "$concat_file"
    frames="$(ffprobe -v error -select_streams v:0 -count_frames \
        -show_entries stream=nb_read_frames -of csv=p=0 "$sample" 2>/dev/null || true)"
    if [[ "$frames" =~ ^[0-9]+$ ]]; then
        expected_frames=$((expected_frames + frames))
    fi
done

playlist="$work_dir/playlist.ts"
if ! timeout 120s ffmpeg -y -nostdin -hide_banner -v error \
    -f concat -safe 0 -i "$concat_file" -map 0:v:0 -c copy -f mpegts "$playlist" \
    > "$work_dir/playlist-build.log" 2>&1; then
    echo "long_playback=skip reason=playlist_build_failed log=$work_dir/playlist-build.log"
    exit 77
fi

playlist_log="$work_dir/playlist.log"
playlist_md5="$work_dir/playlist.md5"
set +e
timeout 300s "$kernel_tool" -- timeout 270s env \
    LIBVA_DRIVERS_PATH="$driver_dir" V4L2_VA_DEBUG=1 \
    ffmpeg -y -nostdin -hide_banner -v error \
    -hwaccel vaapi -hwaccel_device "$drm_device" \
    -i "$playlist" -map 0:v:0 -f framemd5 "$playlist_md5" \
    > "$playlist_log" 2>&1
playlist_status=$?
set -e

decoded="$(awk '!/^#/ && NF {count++} END {print count + 0}' "$playlist_md5" 2>/dev/null || true)"
source_changes="$(grep -c 'SOURCE_CHANGE' "$playlist_log" || true)"
kernel_line="$(grep -m1 'summary:' "$playlist_log" || true)"
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
if (( playlist_status != 0 )); then
    result=fail
    reason="playlist_status_$playlist_status"
elif (( expected_frames > 0 && decoded != expected_frames )); then
    result=fail
    reason="decoded_${decoded}_expected_${expected_frames}"
elif (( source_changes < 1 )); then
    result=fail
    reason="source_change_missing"
elif [[ "$system_fatal" != NA ]] && (( system_fatal > 0 )); then
    result=fail
    reason=firmware_system_fatal
elif [[ "$session_fatal" != NA ]] && (( session_fatal > 0 )); then
    result=fail
    reason=firmware_session_fatal
elif [[ "$sanity" != pass ]]; then
    result=fail
    reason=post_decode_failed
fi

echo "long_playback=$result reason=$reason segments=$segments decoded=$decoded expected=$expected_frames source_changes=$source_changes sanity=$sanity kernel(session=$session_fatal,system=$system_fatal) log=$playlist_log"
[[ "$result" == pass ]]
