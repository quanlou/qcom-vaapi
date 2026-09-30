#!/usr/bin/env bash
# Required 4K correctness gate: codec reference versus VA CPU-copy.
# A missing sample or failed native reference is a failure, never a pass/skip.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
codec="${V4L2_VA_4K_CODEC:-h264}"
loops="${V4L2_VA_4K_LOOPS:-1}"
if [[ ! "$loops" =~ ^[1-9][0-9]*$ || "$loops" -gt 100 ]]; then
    echo "decode_4k=fail reason=invalid_loop_count value=$loops"
    exit 1
fi
minimum_fps="${V4L2_VA_4K_MIN_FPS:-0}"
if [[ ! "$minimum_fps" =~ ^[0-9]+([.][0-9]+)?$ ]]; then
    echo "decode_4k=fail reason=invalid_minimum_fps value=$minimum_fps"
    exit 1
fi
input_args=(-stream_loop "$((loops - 1))")
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

source_frames="$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$sample")"
if [[ ! "$source_frames" =~ ^[0-9]+$ || "$source_frames" -lt 30 ]]; then
    echo "decode_4k=fail reason=invalid_sample_frame_count frames=$source_frames"
    exit 1
fi

for leg in 1 30 full; do
    frame_args=()
    if [[ "$leg" != full ]]; then
        frame_args=(-frames:v "$leg")
    fi
    # The rawvideo checksum encoder otherwise auto-threads across frames,
    # retaining a large backlog of downloaded 4K frames unrelated to decode.
    reference="$work_dir/native-$leg.md5"
    actual="$work_dir/driver-$leg.md5"
    timeout -k 5s 120s "$repo_root/tools/capture-iris-kernel-log.sh" -- ffmpeg -y -nostdin -hide_banner -v error \
        -c:v "$reference_decoder" -i "$sample" -map 0:v:0 "${frame_args[@]}" "${reference_args[@]}" \
        -threads:v 1 -f framemd5 "$reference" > "$work_dir/native-$leg.log" 2>&1 || {
        echo "decode_4k=fail leg=$leg reason=native_decode log=$work_dir/native-$leg.log"
        exit 1
    }
    # Native stream_loop can return success after only one clip. Decode one
    # complete native reference, then check every repeated pixel and timestamp.
    reference_frames="$source_frames"
    repeats="$loops"
    if [[ "$leg" != full ]]; then
        reference_frames="$leg"
        repeats=1
    fi
    native_records="$(awk '!/^#/ && NF {n++} END {print n+0}' "$reference")"
    if [[ "$native_records" != "$reference_frames" ]]; then
        echo "decode_4k=fail leg=$leg reason=short_reference decoded=$native_records expected=$reference_frames"
        exit 1
    fi
    timeout -k 5s 120s "$repo_root/tools/capture-iris-kernel-log.sh" -- \
        env LIBVA_DRIVERS_PATH="$driver_dir" V4L2_VA_DEBUG=1 \
        /usr/bin/time -f '%e %M' -o "$work_dir/driver-$leg.time" \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" "${input_args[@]}" -i "$sample" \
        -map 0:v:0 "${frame_args[@]}" -vf "hwdownload,format=$format" -threads:v 1 -f framemd5 "$actual" \
        > "$work_dir/driver-$leg.log" 2>&1 || {
        echo "decode_4k=fail leg=$leg reason=driver_decode log=$work_dir/driver-$leg.log"
        exit 1
    }
    records="$(awk '!/^#/ && NF {n++} END {print n+0}' "$actual")"
    if ! python3 "$repo_root/tools/compare-frame-repeats.py" "$reference" "$actual" \
        --reference-frames "$reference_frames" --repeats "$repeats"; then
        echo "decode_4k=fail leg=$leg reason=frame_parity log=$work_dir/driver-$leg.log"
        exit 1
    fi
    if rg 'session-fatal\(0x4000003\)=[1-9]|system-fatal\(0x5000003\)=[1-9]|kernel-bugs=[1-9]|vb2-warns=[1-9]|other-session=[1-9]|other-system=[1-9]' "$work_dir/driver-$leg.log" >/dev/null; then
        echo "decode_4k=fail leg=$leg reason=firmware_fault log=$work_dir/driver-$leg.log"
        exit 1
    fi
    if ! rg 'summary: session-fatal\(0x4000003\)=0  system-fatal\(0x5000003\)=0' "$work_dir/driver-$leg.log" >/dev/null; then
        echo "decode_4k=fail leg=$leg reason=missing_clean_kernel_evidence log=$work_dir/driver-$leg.log"
        exit 1
    fi
    if [[ "$leg" == full ]]; then
        read -r elapsed peak_rss_kib < "$work_dir/driver-$leg.time"
        python3 - "$records" "$elapsed" "$peak_rss_kib" "$minimum_fps" <<'PY_FPS'
import sys
frames, elapsed, peak_rss, minimum = map(float, sys.argv[1:])
if elapsed <= 0:
    raise SystemExit('decode_4k=fail reason=invalid_elapsed_time')
fps = frames / elapsed
print(f'decode_4k_throughput frames={frames:g} elapsed_s={elapsed:g} '
      f'fps={fps:.2f} peak_rss_kib={peak_rss:g} minimum_fps={minimum:g}')
if fps < minimum:
    raise SystemExit('decode_4k=fail reason=throughput_below_requirement')
PY_FPS
    fi
    echo "decode_4k_leg=pass leg=$leg decoded=$records dimensions=3840x2160 parity=byte_exact"
done
echo "decode_4k=pass codec=$codec logs=$work_dir"
