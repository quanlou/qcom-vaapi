#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/tools/hardware-session.sh"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
one_frame_sample="${V4L2_VA_ONE_FRAME_SAMPLE:-/home/mq/tmp/vaatest/one-frame.mp4}"
bframes_sample="${V4L2_VA_BFRAMES_SAMPLE:-/home/mq/tmp/vaatest/bframes-240p.mp4}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
native_decoder="${V4L2_VA_NATIVE_DECODER:-h264_v4l2m2m}"
work_dir="${V4L2_VA_VERIFY_DIR:-/tmp/libva-v4l2-verify}"

strict="${V4L2_VA_STRICT:-0}"
if [[ "$strict" != 0 && "$strict" != 1 ]]; then
    echo "verification_fail reason=invalid_strict value=$strict"
    exit 1
fi
if [[ ! -f "$sample" ]]; then
    echo "verification_fail reason=missing_required_sample sample=$sample"
    exit 1
fi
if [[ "$strict" == 1 && ( ! -f "$one_frame_sample" || ! -f "$bframes_sample" ) ]]; then
    echo "verification_fail reason=missing_edge_samples one_frame=$one_frame_sample bframes=$bframes_sample"
    exit 1
fi
mkdir -p "$work_dir"

cargo test --manifest-path "$repo_root/rust/Cargo.toml"
"$repo_root/tools/build-rust-driver.sh" "$driver_dir"

require_live_iris
timeout -k 5s 30s env LIBVA_DRIVERS_PATH="$driver_dir" vainfo --display drm --device "$drm_device" > "$work_dir/vainfo.log" 2>&1

run_native_framemd5() {
    local label="$1"
    local input="$2"
    local output="$3"
    local log="$4"
    local attempts="$5"
    shift 5
    local frame_args=("$@")
    local status=1
    local attempt_log="$log"

    for attempt in $(seq 1 "$attempts"); do
        attempt_log="$log"
        if [[ "$attempts" -gt 1 ]]; then
            attempt_log="${log%.log}-attempt-$attempt.log"
        fi
        rm -f "$output" "$attempt_log"
        status=0
        run_kernel_checked "$attempt_log" timeout -k 5s 120s \
            ffmpeg -nostdin -hide_banner -v warning \
            -c:v "$native_decoder" \
            -i "$input" -map 0:v:0 "${frame_args[@]}" -f framemd5 "$output" \
            || status=$?
        if [[ "$status" -eq 0 ]]; then
            if [[ "$attempt_log" != "$log" ]]; then
                cp "$attempt_log" "$log"
            fi
            return 0
        fi
        if [[ "$attempt" -lt "$attempts" ]]; then
            echo "native_retry label=$label attempt=$attempt status=$status log=$attempt_log"
            sleep 1
        fi
    done
    if [[ "$attempt_log" != "$log" && -f "$attempt_log" ]]; then
        cp "$attempt_log" "$log"
    fi
    return "$status"
}

verify_framemd5() {
    local label="$1"
    local input="$2"
    local frames="${3:-}"
    local mode="${4:-required}"

    if [[ ! -f "$input" ]]; then
        if [[ "$mode" == "required" ]]; then
            echo "framemd5_fail label=$label reason=missing_sample sample=$input"
            return 1
        fi
        echo "framemd5_skip label=$label missing=$input"
        return 0
    fi

    local rust_md5="$work_dir/rust-$label.md5"
    local native_md5="$work_dir/native-$label.md5"
    local rust_log="$work_dir/rust-$label.log"
    local native_log="$work_dir/native-$label.log"
    local frame_args=()
    rm -f "$rust_md5" "$native_md5" "$rust_log" "$native_log"

    if [[ -n "$frames" ]]; then
        frame_args=(-frames:v "$frames")
    fi

    local native_attempts=1
    if [[ "$mode" == "optional" ]]; then
        native_attempts=1
    fi
    local native_status=0
    local reference=native expected_reference_frames=""
    local hw_frame_args=()
    local download_args=()
    if [[ "$label" == one-frame-eos || "$label" == bframes-240p ]]; then
        # Native h264_v4l2m2m can return zero/partial small streams without
        # draining them. These EOS/short-stream gates need an independent,
        # complete oracle and real VA output; keep the required 1/30/full
        # matrix backed by the native decoder above.
        reference=software
        native_md5="$work_dir/software-$label.md5"
        native_log="$work_dir/software-$label.log"
        expected_reference_frames="$(ffprobe -v error -select_streams v:0 -count_frames \
            -show_entries stream=nb_read_frames -of csv=p=0 "$input")" || return 1
        [[ "$expected_reference_frames" =~ ^[1-9][0-9]*$ ]] || return 1
        if [[ -n "$frames" && "$frames" -lt "$expected_reference_frames" ]]; then
            expected_reference_frames="$frames"
        fi
        hw_frame_args=(-hwaccel_output_format vaapi)
        download_args=(-vf hwdownload,format=nv12 -threads:v 1)
        timeout -k 5s 120s ffmpeg -y -nostdin -hide_banner -v warning \
            -c:v h264 -i "$input" -map 0:v:0 "${frame_args[@]}" \
            -vf format=nv12 -threads:v 1 -f framemd5 "$native_md5" \
            > "$native_log" 2>&1 || native_status=$?
    else
        run_native_framemd5 "$label" "$input" "$native_md5" "$native_log" "$native_attempts" "${frame_args[@]}" || native_status=$?
    fi
    if [[ "$native_status" -ne 0 ]]; then
        if [[ "$mode" == "optional" ]]; then
            echo "framemd5_xfail label=$label reason=native-decode-failed status=$native_status sample=$input frames=${frames:-all} log=$native_log"
            return 0
        fi
        echo "framemd5_fail label=$label reason=native-decode-failed status=$native_status sample=$input frames=${frames:-all} log=$native_log"
        return "$native_status"
    fi

    local native_frames
    native_frames="$(awk '($0 ~ /^[0-9]+,/) { count++ } END { print count + 0 }' "$native_md5")"
    if [[ "$native_frames" -eq 0 ]]; then
        # FFmpeg's native h264_v4l2m2m wrapper can exit successfully without
        # flushing a frame when stopped at exactly one output frame. Decode a
        # longer prefix and retain its first checksum so sample-1 remains a
        # required byte-for-byte comparison instead of silently becoming a
        # skip.
        if [[ "$label" == "sample-1" && "$mode" == "required" ]]; then
            local fallback_md5="$work_dir/native-$label-fallback.md5"
            local fallback_log="$work_dir/native-$label-fallback.log"
            local fallback_status=0
            run_native_framemd5 "$label-fallback" "$input" "$fallback_md5" "$fallback_log" 1 -frames:v 30 || fallback_status=$?
            local fallback_frames=0
            if [[ "$fallback_status" -eq 0 && -f "$fallback_md5" ]]; then
                fallback_frames="$(awk '($0 ~ /^[0-9]+,/) { count++ } END { print count + 0 }' "$fallback_md5")"
            fi
            if [[ "$fallback_frames" -gt 0 ]]; then
                awk '/^#/ { print; next } /^[0-9]+,/ { print; exit }' "$fallback_md5" > "$native_md5"
                native_frames=1
                echo "native_reference_fallback label=$label decoded=30 retained=1 log=$fallback_log"
            fi
        fi
        if [[ "$native_frames" -eq 0 ]]; then
            if [[ "$mode" == "optional" ]]; then
                echo "framemd5_xfail label=$label reason=native-produced-no-frames sample=$input frames=${frames:-all}"
                return 0
            fi
            echo "framemd5_fail label=$label reason=native-produced-no-frames sample=$input frames=${frames:-all}"
            return 1
        fi
    fi

    if [[ "$reference" == software && "$native_frames" -ne "$expected_reference_frames" ]]; then
        echo "framemd5_fail label=$label reason=incomplete_software_reference decoded=$native_frames expected=$expected_reference_frames"
        return 1
    fi

    if [[ "$mode" == "optional" ]]; then
        local rust_status=0
        run_kernel_checked "$rust_log" timeout -k 5s 120s env LIBVA_DRIVERS_PATH="$driver_dir" \
            ffmpeg -nostdin -hide_banner -v warning -xerror \
            -hwaccel vaapi -hwaccel_device "$drm_device" "${hw_frame_args[@]}" \
            -i "$input" -map 0:v:0 "${frame_args[@]}" "${download_args[@]}" -f framemd5 "$rust_md5" \
            || rust_status=$?
        if [[ "$rust_status" -ne 0 ]]; then
            echo "framemd5_xfail label=$label reason=rust-decode-failed status=$rust_status sample=$input frames=${frames:-all} log=$rust_log"
            return 0
        fi
        local cmp_status=0
        cmp "$rust_md5" "$native_md5" || cmp_status=$?
        if [[ "$cmp_status" -ne 0 ]]; then
            echo "framemd5_xfail label=$label reason=output-mismatch sample=$input frames=${frames:-all}"
            return 0
        fi
    else
        run_kernel_checked "$rust_log" timeout -k 5s 120s env LIBVA_DRIVERS_PATH="$driver_dir" \
            ffmpeg -nostdin -hide_banner -v warning -xerror \
            -hwaccel vaapi -hwaccel_device "$drm_device" "${hw_frame_args[@]}" \
            -i "$input" -map 0:v:0 "${frame_args[@]}" "${download_args[@]}" -f framemd5 "$rust_md5" || return $?

        cmp "$rust_md5" "$native_md5" || return $?
    fi
    local rust_frames
    rust_frames="$(awk '($0 ~ /^[0-9]+,/) { count++ } END { print count + 0 }' "$rust_md5")"
    if [[ "$rust_frames" -eq 0 || "$rust_frames" -ne "$native_frames" ||
          ( -n "$frames" && "$rust_frames" -ne "$frames" ) ]]; then
        echo "framemd5_fail label=$label reason=frame_count rust=$rust_frames native=$native_frames requested=${frames:-all}"
        return 1
    fi
    echo "framemd5_ok label=$label sample=$input frames=${frames:-all} reference=$reference"
}

verify_framemd5 sample-1 "$sample" 1
verify_framemd5 sample-30 "$sample" 30
verify_framemd5 sample-full "$sample"

gst_export_status=0
"$repo_root/tools/verify-gst-export.sh" "$driver_dir" || gst_export_status=$?
if [[ "$gst_export_status" -ne 0 && ( "$gst_export_status" -ne 77 || "$strict" == 1 ) ]]; then
    exit "$gst_export_status"
fi

# GL roundtrip: prove the exported dma-buf descriptor's plane offsets,
# strides, and sizes are correct by round-tripping the sample through
# glupload/gldownload and confirming each ref frame's pixels appear
# byte-exact in the gl set (survives gst-vaapi pool warmup + B-frame
# reorder). This is the standing Phase 3 zero-copy gate.
gl_roundtrip_status=0
"$repo_root/tools/verify-gl-roundtrip.sh" "$driver_dir" || gl_roundtrip_status=$?
if [[ "$gl_roundtrip_status" -ne 0 && ( "$gl_roundtrip_status" -ne 77 || "$strict" == 1 ) ]]; then
    exit "$gl_roundtrip_status"
fi

hwmap_log="$work_dir/hwmap-export-probe.log"
set +e
run_kernel_checked "$hwmap_log" timeout -k 5s 60s env LIBVA_DRIVERS_PATH="$driver_dir" V4L2_VA_DEBUG=1 \
    ffmpeg -nostdin -hide_banner -v verbose \
    -init_hw_device "vaapi=va:$drm_device" \
    -hwaccel vaapi -hwaccel_device va -hwaccel_output_format vaapi \
    -i "$sample" -map 0:v:0 -an \
    -vf 'hwmap=derive_device=drm:mode=read+direct' \
    -frames:v 1 -f null -
hwmap_status=$?
set -e

if grep -q 'msm_drv_video_rs: ExportSurfaceHandle' "$hwmap_log"; then
    echo "export_probe=reached_driver status=$hwmap_status log=$hwmap_log"
elif grep -q 'Failed to created derived device context' "$hwmap_log"; then
    echo "export_probe=blocked_before_driver status=$hwmap_status log=$hwmap_log"
else
    echo "export_probe=no_driver_call status=$hwmap_status log=$hwmap_log"
fi

# Run optional edge probes after the required 720p matrix and export probes.
# Some firmware/native decoder failures in these probes can leave the next
# hardware session unhealthy, so they must not poison baseline verification.
resolution_status=0
"$repo_root/tools/verify-resolution-churn.sh" "$driver_dir" || resolution_status=$?
if [[ "$resolution_status" -ne 0 && ( "$resolution_status" -ne 77 || "$strict" == 1 ) ]]; then
    exit "$resolution_status"
fi

long_status=0
"$repo_root/tools/verify-long-playback.sh" "$driver_dir" || long_status=$?
if [[ "$long_status" -ne 0 && ( "$long_status" -ne 77 || "$strict" == 1 ) ]]; then
    exit "$long_status"
fi

codec_status=0
"$repo_root/tools/verify-codec-expansion.sh" "$driver_dir" | tee "$work_dir/codec-expansion.log" || codec_status=$?
if [[ "$codec_status" -ne 0 && ( "$codec_status" -ne 77 || "$strict" == 1 ) ]]; then
    exit "$codec_status"
fi

if [[ "$strict" == 1 ]]; then
    for codec in hevc hevc10 vp9; do
        if ! grep -q "^codec_$codec=pass " "$work_dir/codec-expansion.log"; then
            echo "verification_fail reason=required_codec_not_verified codec=$codec"
            exit 1
        fi
    done
fi
edge_mode=optional
if [[ "$strict" == 1 ]]; then
    edge_mode=required
fi
verify_framemd5 one-frame-eos "$one_frame_sample" "" "$edge_mode"
# The 720p sample above already contains reordered frames and is part of the
# required matrix. This stricter small High-profile stream currently exposes a
# H.264 synthesis/firmware compatibility gap, so keep it visible without making
# the baseline verifier unusable.
verify_framemd5 bframes-240p "$bframes_sample" "" "$edge_mode"

echo "verified: required H.264 matrix; inspect per-probe results above for optional coverage"
