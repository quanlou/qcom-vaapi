#!/usr/bin/env bash
# Root enables kernel callsite logging; decoder clients still run as SUDO_USER.
# Restore the original logging flags even if the probe fails or is interrupted.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:?usage: sudo tools/capture-iris-dynamic-debug.sh DRIVER_DIR [LOG_DIR]}"
work_dir="${2:-/tmp/libva-v4l2-iris-debug-$(date +%s)}"
control=/sys/kernel/debug/dynamic_debug/control
sample_720p="${V4L2_VA_IRIS_SAMPLE_720P:-/home/mq/tmp/vaatest/test_720p.mp4}"
sample_240p="${V4L2_VA_IRIS_SAMPLE_240P:-/home/mq/tmp/vaatest/bframes-240p.mp4}"
small_decoder="${V4L2_VA_IRIS_SMALL_DECODER:-driver}"
case "$small_decoder" in
    native|driver) ;;
    *) echo "iris_debug=blocked reason=invalid_small_decoder"; exit 77 ;;
esac
if [[ "$EUID" != 0 || -z "${SUDO_USER:-}" || "$SUDO_USER" == root ]]; then
    echo "iris_debug=blocked reason=requires_sudo_from_decoder_user"
    exit 77
fi
if [[ ! -r "$control" || ! -w "$control" ]]; then
    echo "iris_debug=blocked reason=dynamic_debug_unavailable"
    exit 77
fi
# Validate everything before changing kernel logging or opening a session.
for program in python3 ffmpeg ffprobe timeout runuser flock; do
    command -v "$program" >/dev/null || { echo "iris_debug=blocked reason=missing_tool tool=$program"; exit 77; }
done
for path in "$driver_dir/msm_drv_video.so" "$sample_720p" "$sample_240p"; do
    [[ -r "$path" ]] || { echo "iris_debug=blocked reason=missing_input path=$path"; exit 77; }
done
for node in /dev/video16 /dev/dri/renderD128; do
    [[ -c "$node" ]] || { echo "iris_debug=blocked reason=missing_hardware path=$node"; exit 77; }
done
# Count an independent software decode, rather than trusting successful exit.
expected_720p="$(timeout -k 5s 60s ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$sample_720p")"
expected_240p="$(timeout -k 5s 60s ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$sample_240p")"
for count in "$expected_720p" "$expected_240p"; do
    [[ "$count" =~ ^[1-9][0-9]*$ ]] || { echo "iris_debug=blocked reason=invalid_reference_frame_count"; exit 77; }
done
(( expected_720p > 30 )) && expected_720p=30
(( expected_240p > 30 )) && expected_240p=30
if [[ -e "$work_dir" ]]; then
    echo "iris_debug=blocked reason=log_directory_already_exists path=$work_dir"
    exit 77
fi
mkdir -m 755 "$work_dir"
install -d -m 755 -o "$SUDO_USER" "$work_dir/frames"
exec 9>/tmp/libva-v4l2-hardware.lock
flock -n 9 || { echo "iris_debug=blocked reason=hardware_busy"; exit 77; }

python3 - "$control" "$work_dir/callsites.json" <<'PY'
import json, pathlib, re, sys
control, saved = map(pathlib.Path, sys.argv[1:])
sites = []
for line in control.read_text().splitlines():
    match = re.match(r'(\S+):(\d+)\s+\[[^]]*\]\S+\s+=(\S+)', line)
    if match and '/qcom/iris/' in match[1]:
        sites.append([match[1], int(match[2]), 'p' in match[3]])
if not sites:
    raise SystemExit('no Qualcomm Iris dynamic-debug callsites found')
saved.write_text(json.dumps(sites))
PY
restore() {
    python3 - "$control" "$work_dir/callsites.json" <<'PY'
import json, pathlib, sys
control, saved = map(pathlib.Path, sys.argv[1:])
for filename, line, was_enabled in json.loads(saved.read_text()):
    if not was_enabled:
        control.write_text(f'file {filename} line {line} -p\n')
PY
}
trap restore EXIT
printf '%s\n' 'file *qcom/iris/* +p' > "$control"

# Stop at the first failure; never repeatedly open an already-failing session.
for leg in native-720p driver-720p "$small_decoder-240p"; do
    sample="$sample_720p"
    expected="$expected_720p"
    decoder_args=(-c:v h264_v4l2m2m)
    output_args=()
    if [[ "$leg" == driver-* ]]; then
        decoder_args=(-hwaccel vaapi -hwaccel_output_format vaapi
                      -hwaccel_device /dev/dri/renderD128)
        output_args=(-vf hwdownload,format=nv12)
    fi
    if [[ "$leg" == *-240p ]]; then
        sample="$sample_240p"
        expected="$expected_240p"
    fi
    status=0
    timeout -k 5s 60s "$repo_root/tools/capture-iris-kernel-log.sh" -- \
        runuser -u "$SUDO_USER" -- env LIBVA_DRIVERS_PATH="$driver_dir" \
        LIBVA_DRIVER_NAME=msm V4L2_VA_DEBUG=1 \
        ffmpeg -y -nostdin -hide_banner -v error "${decoder_args[@]}" \
        -i "$sample" -map 0:v:0 -frames:v 30 "${output_args[@]}" \
        -pix_fmt nv12 -threads 1 -f framemd5 "$work_dir/frames/$leg.md5" > "$work_dir/$leg.log" 2>&1 || status=$?
    echo "iris_debug_leg=$leg status=$status log=$work_dir/$leg.log"
    if [[ "$status" != 0 ]]; then exit "$status"; fi
    check_args=("$work_dir/frames/$leg.md5" --expected "$expected")
    if [[ "$leg" == driver-720p ]]; then
        check_args+=(--reference "$work_dir/frames/native-720p.md5")
    fi
    python3 "$repo_root/tools/check-iris-probe-output.py" "${check_args[@]}" \
        >> "$work_dir/$leg.log" 2>&1 || { echo "iris_debug_leg=$leg status=1 reason=incomplete_or_corrupt_output"; exit 1; }
done
