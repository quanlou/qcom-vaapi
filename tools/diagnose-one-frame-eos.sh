#!/usr/bin/env bash
# One bounded native-vs-VA EOS diagnostic. No retries; stop after any kernel fault.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:?usage: diagnose-one-frame-eos.sh DRIVER_DIR FRESH_RESULT_DIR}"
work_dir="${2:?provide a fresh disk-backed result directory}"
sample="${V4L2_VA_ONE_FRAME_SAMPLE:-/home/mq/tmp/vaatest/one-frame.mp4}"
expected_sha="${V4L2_VA_EOS_EXPECTED_DRIVER_SHA256:-fc08226fcc18c257f52d2159ad8ebb5ed57f2a580dbf23640f9340b60ffa6e10}"
decoder="${V4L2_VA_DEVICE:-/dev/video16}"
drm="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
if [[ ! -c "$decoder" || ! -c "$drm" ]]; then
    echo "one_frame_diagnostic=blocked reason=missing_hardware decoder=$decoder drm=$drm"
    exit 77
fi
for command in ffmpeg ffprobe strace sha256sum flock timeout python3; do
    command -v "$command" >/dev/null || { echo "one_frame_diagnostic=blocked reason=missing_tool tool=$command"; exit 77; }
done
actual_sha="$(sha256sum "$driver_dir/msm_drv_video.so" | awk '{print $1}')"
[[ "$actual_sha" == "$expected_sha" ]] || { echo "one_frame_diagnostic=fail reason=unexpected_driver hash=$actual_sha"; exit 1; }
[[ ! -e "$work_dir" ]] || { echo "one_frame_diagnostic=fail reason=results_already_exist"; exit 1; }
mkdir -p "$work_dir"
exec 9>/tmp/libva-v4l2-hardware.lock
flock -n 9 || { echo "one_frame_diagnostic=blocked reason=hardware_busy"; exit 77; }
expected="$(ffprobe -v error -select_streams v:0 -count_frames -show_entries stream=nb_read_frames -of csv=p=0 "$sample")"
[[ "$expected" == 1 ]] || { echo "one_frame_diagnostic=fail reason=fixture_not_exactly_one_frame count=$expected"; exit 1; }
sha256sum "$sample" "$driver_dir/msm_drv_video.so" > "$work_dir/input-sha256.txt"
uname -a > "$work_dir/kernel.txt"
ffprobe -v error -select_streams v:0 -show_streams -show_packets "$sample" > "$work_dir/fixture.txt"
timeout -k 5s 30s ffmpeg -y -nostdin -hide_banner -v error -c:v h264 \
    -i "$sample" -map 0:v:0 -vf format=nv12 -threads:v 1 -f framemd5 "$work_dir/software.md5" \
    > "$work_dir/software.log" 2>&1
python3 "$repo_root/tools/compare-frame-repeats.py" "$work_dir/software.md5" "$work_dir/software.md5" --reference-frames 1
clean_window() {
    # Require the complete classifier summary, not a truncated first two fields.
    grep -Eq 'summary: session-fatal\(0x4000003\)=0 +system-fatal\(0x5000003\)=0 +power-cycles=0 +vb2-warns=0 +other-session=0 +other-system=0 +kernel-bugs=0$' "$1" &&
        ! grep -Eq '(session-fatal\(0x4000003\)|system-fatal\(0x5000003\)|power-cycles|vb2-warns|other-session|other-system|kernel-bugs)=[1-9]' "$1"
}
# Trace the native wrapper's actual EOS ioctl/drain sequence. A zero-output
# native exit is evidence about this reference decoder, not a successful decode.
status=0
timeout -k 5s 45s "$repo_root/tools/capture-iris-kernel-log.sh" -- \
    strace -f -o "$work_dir/native-ioctl.trace" -e trace=ioctl \
    ffmpeg -y -nostdin -hide_banner -v verbose -c:v h264_v4l2m2m -i "$sample" \
    -map 0:v:0 -vf format=nv12 -threads:v 1 -f framemd5 "$work_dir/native.md5" \
    > "$work_dir/native.log" 2>&1 || status=$?
if [[ "$status" != 0 ]] || ! clean_window "$work_dir/native.log"; then
    echo "one_frame_diagnostic=stop phase=native status=$status reason=failed_command_or_kernel_observation log=$work_dir/native.log"
    exit 1
fi
native_frames="$(awk '/^[0-9]+,/ {n++} END {print n+0}' "$work_dir/native.md5")"
echo "one_frame_native frames=$native_frames expected=1 status=$status log=$work_dir/native.log"
# Proceed exactly once when native cleanly returned zero output, so reference
# failure can be distinguished from actual driver output/drain failure.
status=0
timeout -k 5s 45s "$repo_root/tools/capture-iris-kernel-log.sh" -- \
    env LIBVA_DRIVERS_PATH="$driver_dir" LIBVA_DRIVER_NAME=msm V4L2_VA_DEVICE="$decoder" V4L2_VA_DEBUG=1 \
    strace -f -o "$work_dir/va-ioctl.trace" -e trace=ioctl \
    ffmpeg -y -nostdin -hide_banner -v verbose -hwaccel vaapi -hwaccel_output_format vaapi \
    -hwaccel_device "$drm" -i "$sample" -map 0:v:0 -vf hwdownload,format=nv12 \
    -threads:v 1 -f framemd5 "$work_dir/va.md5" > "$work_dir/va.log" 2>&1 || status=$?
if [[ "$status" != 0 ]] || ! clean_window "$work_dir/va.log"; then
    echo "one_frame_diagnostic=stop phase=va status=$status reason=failed_command_or_kernel_observation log=$work_dir/va.log"
    exit 1
fi
[[ "$(sha256sum "$driver_dir/msm_drv_video.so" | awk '{print $1}')" == "$expected_sha" ]] || { echo "one_frame_diagnostic=fail reason=driver_changed"; exit 1; }
python3 "$repo_root/tools/compare-frame-repeats.py" "$work_dir/software.md5" "$work_dir/va.md5" --reference-frames 1 || {
    echo "one_frame_diagnostic=fail reason=driver_zero_or_incorrect_output log=$work_dir/va.log"; exit 1;
}
echo "one_frame_diagnostic=pass va_frames=1 native_frames=$native_frames qualification=single_fixture_only results=$work_dir"
