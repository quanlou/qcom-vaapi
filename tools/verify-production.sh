#!/usr/bin/env bash
# Release gate: no skipped required probes or expected decode failures.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/tools/hardware-session.sh"
driver_dir="${1:-/tmp/libva-v4l2-production-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
work_dir="${V4L2_VA_PRODUCTION_DIR:-/tmp/libva-v4l2-production}"
# A production result directory belongs to one invocation. Reusing logs can
# make an interrupted attempt look like a completed release qualification.
if [[ -d "$work_dir" && -n "$(ls -A "$work_dir")" ]]; then
    echo "production=fail reason=results_directory_not_empty path=$work_dir"
    exit 1
fi
mkdir -p "$work_dir"

if [[ "${LIBVA_DRIVER_NAME:-msm}" != msm ||
      "${V4L2_VA_NATIVE_DECODER:-h264_v4l2m2m}" != h264_v4l2m2m ]]; then
    echo "production=fail reason=incorrect_driver_or_native_decoder"
    exit 1
fi
export LIBVA_DRIVER_NAME=msm V4L2_VA_NATIVE_DECODER=h264_v4l2m2m
one_frame_sample="${V4L2_VA_ONE_FRAME_SAMPLE:-/home/mq/tmp/vaatest/one-frame.mp4}"
bframes_sample="${V4L2_VA_BFRAMES_SAMPLE:-/home/mq/tmp/vaatest/bframes-240p.mp4}"

if [[ "${V4L2_VA_EXPERIMENTAL_AV1:-0}" == 1 ]]; then
    echo "production=fail reason=experimental_av1_not_qualified"
    exit 1
fi

if [[ ! -f "$sample" ]]; then
    echo "production=fail reason=missing_sample sample=$sample"
    exit 1
fi
for fixture in "$one_frame_sample" "$bframes_sample"; do
    if [[ ! -f "$fixture" ]]; then
        echo "production=fail reason=missing_edge_fixture path=$fixture"
        exit 1
    fi
done
# Bind every supplied codec fixture, including directory membership. Keep
# generated fixtures outside a supplied directory or prepare them before release.
fixture_args=(--fixture "$sample" --fixture "$one_frame_sample" --fixture "$bframes_sample")
for fixture in "${V4L2_VA_RESOLUTION_LOW_SAMPLE:-}" "${V4L2_VA_RESOLUTION_HIGH_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"; do
    [[ -n "$fixture" ]] || continue
    if [[ ! -f "$fixture" ]]; then
        echo "production=fail reason=missing_resolution_fixture path=$fixture"
        exit 1
    fi
    fixture_args+=(--fixture "$fixture")
done
codec_fixture_dir="${V4L2_VA_CODEC5_DIR:-/home/mq/tmp/vaatest/codec5}"
if [[ -d "$codec_fixture_dir" ]]; then
    fixture_args+=(--fixture "$codec_fixture_dir")
fi
provenance="$work_dir/provenance.json"
python3 "$repo_root/tools/production-provenance.py" snapshot "$provenance" \
    --root "$repo_root" "${fixture_args[@]}"
# Compare the entire GL stream, rather than only a short prefix.
frames="$(ffprobe -v error -select_streams v:0 -count_frames \
    -show_entries stream=nb_read_frames -of csv=p=0 "$sample")"
if [[ ! "$frames" =~ ^[1-9][0-9]*$ ]]; then
    echo "production=fail reason=invalid_frame_count value=$frames"
    exit 1
fi
export V4L2_VA_SAMPLE="$sample" V4L2_VA_STRICT=1 V4L2_VA_GL_RT_FRAMES="$frames"
export V4L2_VA_VERIFY_DIR="$work_dir/matrix"
export V4L2_VA_GL_RT_DIR="$work_dir/gl"
export V4L2_VA_RESOLUTION_DIR="$work_dir/resolution"
export V4L2_VA_LONG_DIR="$work_dir/long"
export V4L2_VA_CODEC5_LOG_DIR="$work_dir/codecs"
export V4L2_VA_CHURN_DIR="$work_dir/churn"
export V4L2_VA_EOS_DIR="$work_dir/eos"
export V4L2_VA_SEEK_DIR="$work_dir/seek"

cargo fmt --manifest-path "$repo_root/rust/Cargo.toml" -- --check
cargo clippy --manifest-path "$repo_root/rust/Cargo.toml" --all-targets -- -D warnings
python3 -m unittest discover -s "$repo_root/tools/tests" -v
host_stress_dir="$(mktemp -d "$work_dir/host-stress.XXXXXX")"
"$repo_root/tools/verify-host-stress.sh" "$host_stress_dir"

python3 "$repo_root/tools/production-provenance.py" check "$provenance" --root "$repo_root"

# Serialize complete hardware qualification, including reference decodes.
exec 9> /tmp/libva-v4l2-hardware.lock
if ! flock -n 9; then
    echo "production=fail reason=hardware_in_use"
    exit 1
fi
decoder_device="${V4L2_VA_DEVICE:-/dev/video16}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
if [[ ! -c "$decoder_device" || ! -c "$drm_device" ]]; then
    echo "production=fail reason=missing_hardware decoder=$decoder_device drm=$drm_device"
    exit 1
fi
require_live_iris

"$repo_root/tools/build-rust-driver.sh" "$driver_dir"
python3 "$repo_root/tools/production-provenance.py" bind-driver "$provenance" \
    --root "$repo_root" --driver "$driver_dir/msm_drv_video.so"

for probe in rust-driver session-churn eos-drain seek-storm; do
    status=0
    "$repo_root/tools/capture-iris-kernel-log.sh" --         "$repo_root/tools/verify-$probe.sh" "$driver_dir" > "$work_dir/$probe.log" 2>&1 || status=$?
    if [[ "$status" != 0 ]]; then
        echo "production=fail probe=$probe status=$status log=$work_dir/$probe.log"
        exit 1
    fi
    if ! python3 "$repo_root/tools/check-playback-performance.py" kernel --log "$work_dir/$probe.log"; then
        echo "production=fail probe=$probe reason=kernel_errors_or_missing_observation log=$work_dir/$probe.log"
        exit 1
    fi
    python3 "$repo_root/tools/production-provenance.py" check "$provenance" --root "$repo_root"
    echo "production_probe=pass probe=$probe log=$work_dir/$probe.log"
done
# This gate covers the supplied media and headless import/lifecycle paths.
# Browser launch/selection must additionally pass in the deployment session.
echo "production=pass scope=provided_media_and_headless_lifecycle browser=separate"
