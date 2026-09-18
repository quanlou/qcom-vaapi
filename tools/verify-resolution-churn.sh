#!/usr/bin/env bash
set -euo pipefail

driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
low_sample="${V4L2_VA_RESOLUTION_LOW_SAMPLE:-/home/mq/tmp/vaatest/seq-480p.mp4}"
high_sample="${V4L2_VA_RESOLUTION_HIGH_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
work_dir="${V4L2_VA_RESOLUTION_DIR:-/tmp/libva-v4l2-resolution}"

if ! command -v gst-launch-1.0 >/dev/null 2>&1 ||
    ! command -v gst-inspect-1.0 >/dev/null 2>&1; then
    echo "resolution_probe=skip reason=gstreamer-not-installed"
    exit 77
fi
if [[ ! -f "$low_sample" || ! -f "$high_sample" ]]; then
    echo "resolution_probe=skip missing_samples low=$low_sample high=$high_sample"
    exit 77
fi
if ! command -v ffprobe >/dev/null 2>&1; then
    echo "resolution_probe=skip reason=ffprobe-not-installed"
    exit 77
fi
if ! env GST_VA_ALL_DRIVERS=1 GST_VAAPI_ALL_DRIVERS=1 \
    LIBVA_DRIVERS_PATH="$driver_dir" gst-inspect-1.0 vah264dec >/dev/null 2>&1; then
    echo "resolution_probe=skip reason=vah264dec-unavailable"
    exit 77
fi

mkdir -p "$work_dir"
log="$work_dir/gst-resolution.log"

set +e
timeout 120s env \
    GST_VA_ALL_DRIVERS=1 \
    GST_VAAPI_ALL_DRIVERS=1 \
    LIBVA_DRIVERS_PATH="$driver_dir" \
    V4L2_VA_DEBUG=1 \
    gst-launch-1.0 -q \
    concat name=c ! h264parse ! vah264dec ! fakesink sync=false \
    filesrc location="$low_sample" ! qtdemux name=lo0 \
    lo0.video_0 ! queue ! h264parse ! c.sink_0 \
    filesrc location="$high_sample" ! qtdemux name=hi0 \
    hi0.video_0 ! queue ! h264parse ! c.sink_1 \
    filesrc location="$low_sample" ! qtdemux name=lo1 \
    lo1.video_0 ! queue ! h264parse ! c.sink_2 \
    filesrc location="$high_sample" ! qtdemux name=hi1 \
    hi1.video_0 ! queue ! h264parse ! c.sink_3 \
    > "$log" 2>&1
status=$?
set -e

low_dims="$(ffprobe -v error -select_streams v:0 \
    -show_entries stream=width,height -of csv=p=0 "$low_sample" 2>/dev/null || true)"
high_dims="$(ffprobe -v error -select_streams v:0 \
    -show_entries stream=width,height -of csv=p=0 "$high_sample" 2>/dev/null || true)"
low_output_dims="${low_dims/,/x}"
high_output_dims="${high_dims/,/x}"

changes="$(grep -c 'SOURCE_CHANGE' "$log" || true)"

if [[ "$status" -eq 0 ]] &&
    grep -q "OUTPUT fmt .* ${low_output_dims} " "$log" &&
    grep -q "OUTPUT fmt .* ${high_output_dims} " "$log" &&
    [[ "$changes" -ge 4 ]]; then
    echo "resolution_probe=passed status=0 source_changes=$changes log=$log"
    exit 0
fi

echo "resolution_probe=failed status=$status low_dims=$low_dims high_dims=$high_dims log=$log"
exit 1
