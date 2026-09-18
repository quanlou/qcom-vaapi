#!/usr/bin/env bash
set -euo pipefail

driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
work_dir="${V4L2_VA_VERIFY_DIR:-/tmp/libva-v4l2-verify}"
num_buffers="${V4L2_VA_GST_EXPORT_BUFFERS:-1}"
hold_ms="${V4L2_VA_GST_EXPORT_HOLD_MS:-0}"
hold_us=$((hold_ms * 1000))
log="$work_dir/gst-glupload-export.log"

mkdir -p "$work_dir"

if [[ ! -f "$sample" ]]; then
    echo "gst_export_probe=skip missing_sample=$sample"
    exit 77
fi

if ! command -v gst-launch-1.0 >/dev/null 2>&1 || ! command -v gst-inspect-1.0 >/dev/null 2>&1; then
    echo "gst_export_probe=skip reason=gstreamer-not-installed"
    exit 77
fi

gst_env=(
    GST_VA_ALL_DRIVERS=1
    GST_VAAPI_ALL_DRIVERS=1
    LIBVA_DRIVERS_PATH="$driver_dir"
)

if ! env "${gst_env[@]}" gst-inspect-1.0 vah264dec >/dev/null 2>&1; then
    echo "gst_export_probe=skip reason=vah264dec-unavailable"
    exit 77
fi
if ! gst-inspect-1.0 glupload >/dev/null 2>&1; then
    echo "gst_export_probe=skip reason=glupload-unavailable"
    exit 77
fi

set +e
pipeline=(
    filesrc location="$sample" ! qtdemux name=d d.video_0 !
    queue ! h264parse ! vah264dec ! glupload
)
if (( hold_us > 0 )); then
    # Keep a bounded set of imported buffers alive on a side branch while the
    # main branch continues feeding the decoder. A leaky queue prevents the
    # lifetime probe from turning downstream sleep into an artificial decoder
    # input starvation test.
    pipeline+=(
        ! tee name=t
        t. ! queue max-size-buffers="$num_buffers" max-size-bytes=0 max-size-time=0 leaky=downstream
        ! identity sleep-time="$hold_us" ! fakesink sync=false
        t. ! fakesink num-buffers="$num_buffers" sync=false
    )
else
    pipeline+=(
        ! queue max-size-buffers="$num_buffers" max-size-bytes=0 max-size-time=0
        ! identity sleep-time="$hold_us"
        ! fakesink num-buffers="$num_buffers" sync=false
    )
fi
timeout --signal=INT --kill-after=5s 30s env "${gst_env[@]}" V4L2_VA_DEBUG=1 \
    gst-launch-1.0 -q "${pipeline[@]}" > "$log" 2>&1
status=$?
set -e

if grep -q 'msm_drv_video_rs: ExportSurfaceHandle succeeded' "$log"; then
    if [[ "$status" -eq 0 ]]; then
        echo "gst_export_probe=reached_driver status=0 buffers=$num_buffers hold_ms=$hold_ms log=$log"
        exit 0
    fi
    echo "gst_export_probe=reached_driver_but_pipeline_failed status=$status log=$log"
    exit "$status"
fi

if grep -q 'msm_drv_video_rs: ExportSurfaceHandle' "$log"; then
    echo "gst_export_probe=callback_reached_but_export_failed status=$status log=$log"
    exit 1
fi

echo "gst_export_probe=no_driver_call status=$status log=$log"
exit 1
