#!/usr/bin/env bash
# Required 4K correctness gate: codec reference versus VA CPU-copy.
# A missing sample or failed native reference is a failure, never a pass/skip.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
codec="${V4L2_VA_4K_CODEC:-h264}"
reference_args=()
case "$codec" in
    h264) stream_codec=h264; reference_decoder=h264_v4l2m2m; format=nv12; extension=mp4 ;;
    hevc) stream_codec=hevc; reference_decoder=hevc_v4l2m2m; format=nv12; extension=mp4 ;;
    hevc10) stream_codec=hevc; reference_decoder=hevc; format=p010le; extension=mp4
            reference_args=(-pix_fmt p010le) ;;
    vp9) stream_codec=vp9; reference_decoder=vp9_v4l2m2m; format=nv12; extension=webm ;;
    *) echo "decode_4k=fail reason=unknown_codec codec=$codec"; exit 1 ;;
esac
sample="${V4L2_VA_4K_SAMPLE:-/home/mq/tmp/vaatest/quality4k/$codec-2160p.$extension}"
work_dir="${V4L2_VA_4K_LOG_DIR:-/tmp/libva-v4l2-4k/$codec}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
mkdir -p "$work_dir"

dimensions="$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,width,height -of csv=p=0 "$sample")"
if [[ "$dimensions" != "$stream_codec,3840,2160" ]]; then
    echo "decode_4k=fail reason=wrong_sample expected=$stream_codec,3840,2160 actual=$dimensions"
    exit 1
fi

# Other hardware probes must use this same lease to avoid firmware contention.
exec 9>/tmp/libva-v4l2-hardware.lock
flock -n 9 || { echo "decode_4k=fail reason=hardware_busy"; exit 1; }

for leg in 1 30 full; do
    frame_args=()
    if [[ "$leg" != full ]]; then
        frame_args=(-frames:v "$leg")
    fi
    reference="$work_dir/native-$leg.md5"
    actual="$work_dir/driver-$leg.md5"
    timeout -k 5s 120s ffmpeg -y -nostdin -hide_banner -v error \
        -c:v "$reference_decoder" -i "$sample" -map 0:v:0 "${frame_args[@]}" "${reference_args[@]}" \
        -f framemd5 "$reference" > "$work_dir/native-$leg.log" 2>&1 || {
        echo "decode_4k=fail leg=$leg reason=native_decode log=$work_dir/native-$leg.log"
        exit 1
    }
    timeout -k 5s 120s "$repo_root/tools/capture-iris-kernel-log.sh" -- \
        env LIBVA_DRIVERS_PATH="$driver_dir" V4L2_VA_DEBUG=1 \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" -i "$sample" \
        -map 0:v:0 "${frame_args[@]}" -vf "hwdownload,format=$format" -f framemd5 "$actual" \
        > "$work_dir/driver-$leg.log" 2>&1 || {
        echo "decode_4k=fail leg=$leg reason=driver_decode log=$work_dir/driver-$leg.log"
        exit 1
    }
    records="$(awk '!/^#/ && NF {n++} END {print n+0}' "$actual")"
    expected="$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$sample")"
    if [[ "$leg" != full ]]; then expected="$leg"; fi
    if [[ "$records" != "$expected" ]] || ! cmp -s "$reference" "$actual"; then
        echo "decode_4k=fail leg=$leg reason=frame_parity decoded=$records expected=$expected log=$work_dir/driver-$leg.log"
        exit 1
    fi
    if rg 'session-fatal\(0x4000003\)=[1-9]|system-fatal\(0x5000003\)=[1-9]' "$work_dir/driver-$leg.log" >/dev/null; then
        echo "decode_4k=fail leg=$leg reason=firmware_fault log=$work_dir/driver-$leg.log"
        exit 1
    fi
    if ! rg 'summary: session-fatal\(0x4000003\)=0  system-fatal\(0x5000003\)=0' "$work_dir/driver-$leg.log" >/dev/null; then
        echo "decode_4k=fail leg=$leg reason=missing_clean_kernel_evidence log=$work_dir/driver-$leg.log"
        exit 1
    fi
    echo "decode_4k_leg=pass leg=$leg decoded=$records dimensions=3840x2160 parity=byte_exact"
done
echo "decode_4k=pass codec=$codec logs=$work_dir"
