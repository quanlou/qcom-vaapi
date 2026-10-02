#!/usr/bin/env bash
set -euo pipefail

# GL-importer NV12 layout validation for the Rust V4L2 VA driver.
#
# Decodes the sample twice over the Rust driver and compares the pixels:
#   reference: ffmpeg vaapi decode + CPU copy, dumped as tightly packed
#              I420 (the same copy path the required framemd5 matrix
#              already proved byte-equal to the native decoder)
#   GL path:   gst-launch vah264dec ! glupload ! gldownload !
#              videoconvert ! video/x-raw,format=I420 ! filesink, i.e.
#              pixels sampled through the driver's exported dma-buf
#              descriptor after EGL import
#
# tools/gst_gl_roundtrip.py hashes both dumps stride-aware and fails on any
# per-frame mismatch, which would indicate wrong plane offsets, strides, or
# sizes in vaExportSurfaceHandle's descriptor.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/tools/hardware-session.sh"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
work_dir="${V4L2_VA_GL_RT_DIR:-/tmp/libva-v4l2-gl-roundtrip}"
frames="${V4L2_VA_GL_RT_FRAMES:-30}"
reference_limit=(-frames:v "$frames")
comparison_args=()
if [[ "${V4L2_VA_STRICT:-0}" == 1 ]]; then
    # The GL pipeline always runs to EOS. Its strict reference must do the
    # same; a 30-frame prefix cannot certify a complete 300-frame stream.
    reference_limit=()
    frames=0
    comparison_args+=(--ordered)
fi

mkdir -p "$work_dir"

if [[ ! -f "$sample" ]]; then
    echo "gl_roundtrip=skip reason=missing_sample=$sample"
    exit 77
fi

for tool in gst-launch-1.0 gst-inspect-1.0 ffmpeg ffprobe python3; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "gl_roundtrip=skip reason=missing_tool=$tool"
        exit 77
    fi
done

# vah264dec registers its element only when the VA driver is discoverable
# at plugin load (same requirement as tools/verify-gst-export.sh).
gst_env=(
    GST_VA_ALL_DRIVERS=1
    GST_VAAPI_ALL_DRIVERS=1
    LIBVA_DRIVERS_PATH="$driver_dir"
)

if ! env "${gst_env[@]}" gst-inspect-1.0 vah264dec >/dev/null 2>&1; then
    echo "gl_roundtrip=skip reason=missing_element=vah264dec"
    exit 77
fi
for element in glupload gldownload videoconvert; do
    if ! gst-inspect-1.0 "$element" >/dev/null 2>&1; then
        echo "gl_roundtrip=skip reason=missing_element=$element"
        exit 77
    fi
done

# The dumps hold unpacked I420: up to ~1.4 MB per frame per 720p stream.
avail_gb="$(df -BG --output=avail "$work_dir" | tail -1 | tr -dc '0-9')"
if (( avail_gb < 2 )); then
    echo "gl_roundtrip=skip reason=low_disk_space avail_gb=$avail_gb dir=$work_dir"
    exit 77
fi

dims="$(ffprobe -v error -select_streams v:0 \
    -show_entries stream=width,height -of csv=p=0 "$sample")"
width="${dims%,*}"
height="${dims#*,}"
if [[ ! "$width" =~ ^[0-9]+$ || ! "$height" =~ ^[0-9]+$ ]]; then
    echo "gl_roundtrip=skip reason=unreadable_dimensions dims=$dims"
    exit 77
fi

# Reference: Rust VA decode + vaGetImage CPU copy, tightly packed I420
# (ffmpeg's rawvideo encoder packs with alignment 1).
ref_log="$work_dir/ffmpeg-ref.log"
rm -f "$work_dir/ref.raw" "$ref_log"
set +e
run_kernel_checked "$ref_log" timeout -k 5s 120s env "${gst_env[@]}" \
    ffmpeg -nostdin -hide_banner -v warning \
    -hwaccel vaapi -hwaccel_device "$drm_device" -hwaccel_output_format vaapi \
    -i "$sample" -map 0:v:0 "${reference_limit[@]}" \
    -vf 'hwdownload,format=nv12,format=yuv420p' -threads:v 1 -f rawvideo "$work_dir/ref.raw"

ref_status=$?
set -e
if [[ "$ref_status" -ne 0 ]]; then
    echo "gl_roundtrip=fail reason=reference_decode_failed status=$ref_status log=$ref_log"
    exit "$ref_status"
fi

# GL path: decode, export the dmabuf, import it in EGL, download, convert.
gl_log="$work_dir/gst-gl.log"
rm -f "$work_dir/gl.raw" "$gl_log"
set +e
run_kernel_checked "$gl_log" timeout -k 5s 180s env "${gst_env[@]}" V4L2_VA_DEBUG=1 \
    gst-launch-1.0 -q -e \
    filesrc location="$sample" ! qtdemux ! h264parse ! queue ! vah264dec ! \
    glupload ! gldownload ! videoconvert ! \
    video/x-raw,format=I420 ! filesink location="$work_dir/gl.raw"

gl_status=$?
set -e
if ! grep -q 'msm_drv_video_rs: ExportSurfaceHandle succeeded' "$gl_log"; then
    if grep -q 'msm_drv_video_rs: ExportSurfaceHandle' "$gl_log"; then
        echo "gl_roundtrip=fail reason=driver_export_failed log=$gl_log"
    else
        echo "gl_roundtrip=fail reason=driver_export_not_reached log=$gl_log"
    fi
    exit 1
fi
# The probe's primary question is the exported descriptor layout, so a
# pipeline that aborted mid-stream still yields a decisive answer for every
# frame that completed the GL path. Report both facts separately: a nonzero
# pipeline status is itself evidence for the export-lifetime investigation,
# and the layout comparison runs on whatever frames were downloaded.
if [[ "$gl_status" -ne 0 ]]; then
    echo "gl_roundtrip=warn reason=gl_pipeline_failed status=$gl_status log=$gl_log"
    if [[ ! -s "$work_dir/gl.raw" ]]; then
        echo "gl_roundtrip=fail reason=no_frames_downloaded status=$gl_status"
        exit "$gl_status"
    fi
    set +e
    python3 "$repo_root/tools/gst_gl_roundtrip.py" \
        "$work_dir/gl.raw" "$work_dir/ref.raw" \
        --width "$width" --height "$height" --frames "$frames" --max-missing 0 "${comparison_args[@]}"
    layout_status=$?
    set -e
    if [[ "$layout_status" -eq 0 ]]; then
        echo "gl_roundtrip=partial_layout_pass pipeline_status=$gl_status log=$gl_log"
        # Correct partial pixels cannot certify successful playback.
        exit 1
    fi
    echo "gl_roundtrip=fail reason=layout_mismatch pipeline_status=$gl_status"
    exit "$layout_status"
fi

python3 "$repo_root/tools/gst_gl_roundtrip.py" \
    "$work_dir/gl.raw" "$work_dir/ref.raw" \
    --width "$width" --height "$height" --frames "$frames" --max-missing 0 "${comparison_args[@]}"
