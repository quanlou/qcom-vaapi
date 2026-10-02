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
maximum_rss="${V4L2_VA_4K_MAX_RSS_KIB:-0}"
minimum_seconds="${V4L2_VA_4K_MIN_SECONDS:-0}"
strict="${V4L2_VA_4K_STRICT:-${V4L2_VA_STRICT:-0}}"
if [[ "$strict" != 0 && "$strict" != 1 ]]; then
    echo "decode_4k=fail reason=invalid_strict_mode"
    exit 1
fi
if [[ "${LIBVA_DRIVER_NAME:-msm}" != msm ]]; then
    echo "decode_4k=fail reason=incorrect_driver"
    exit 1
fi
measurement_args=(--minimum-fps "$minimum_fps" --maximum-rss-kib "$maximum_rss" --minimum-seconds "$minimum_seconds")
[[ "$strict" == 0 ]] || measurement_args+=(--strict)
python3 "$repo_root/tools/check-playback-performance.py" thresholds "${measurement_args[@]}"
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
if [[ -n "$(ls -A "$work_dir")" ]]; then
    echo "decode_4k=fail reason=results_directory_not_empty path=$work_dir"
    exit 1
fi

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
    # Input and output thread options have separate FFmpeg scopes. Bound
    # decoder frame threading before -i: auto input threads can decode many
    # extra surfaces even in the 1-frame leg. Keep the output checksum encoder
    # single-threaded too, avoiding a downloaded-frame backlog unrelated to
    # hardware throughput. Codec reordering can still require lookahead.
    reference="$work_dir/native-$leg.md5"
    actual="$work_dir/driver-$leg.md5"
    timeout -k 5s 120s "$repo_root/tools/capture-iris-kernel-log.sh" -- ffmpeg -y -nostdin -hide_banner -v error \
        -c:v "$reference_decoder" -threads:v 1 -i "$sample" -map 0:v:0 "${frame_args[@]}" "${reference_args[@]}" \
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
        env LIBVA_DRIVERS_PATH="$driver_dir" LIBVA_DRIVER_NAME=msm V4L2_VA_DEBUG=1 \
        /usr/bin/time -f '%e %M' -o "$work_dir/driver-$leg.time" \
        ffmpeg -y -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" "${input_args[@]}" -threads:v 1 -i "$sample" \
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
    for kernel_log in "$work_dir/native-$leg.log" "$work_dir/driver-$leg.log"; do
        python3 "$repo_root/tools/check-playback-performance.py" kernel --log "$kernel_log" || {
            echo "decode_4k=fail leg=$leg reason=kernel_evidence log=$kernel_log"
            exit 1
        }
    done
    if [[ "$leg" == full ]]; then
        python3 "$repo_root/tools/check-playback-performance.py" 4k "${measurement_args[@]}" \
            --frames "$records" --time-file "$work_dir/driver-$leg.time" \
            --output "$work_dir/performance.json"
    fi
    echo "decode_4k_leg=pass leg=$leg decoded=$records dimensions=3840x2160 parity=byte_exact"
done
echo "decode_4k=pass codec=$codec scope=byte_exact_cpu_download performance=$work_dir/performance.json logs=$work_dir"
