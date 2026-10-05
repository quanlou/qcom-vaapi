#!/usr/bin/env bash
set -uo pipefail

# Codec-expansion verification probe (Phase 5 track B): HEVC / HEVC Main10 / VP9 / AV1
# decode through the Rust V4L2 VA driver, one codec at a time.
#
# Per codec (skip-77 taxonomy, per-codec skips — one wedged/unsupported codec
# must not mask the others):
#   1. sample present in V4L2_VA_CODEC5_DIR (see MANIFEST.txt there), else
#      `codec_<name>=skip reason=missing_sample=...`
#   2. vainfo through the driver must advertise the codec's VAProfile, else
#      `codec_<name>=skip reason=profile_not_advertised`
#   3. an N-frame reference decode, a 1-frame decode through this driver, then
#      an N-frame decode through this driver. HEVC Main and VP9 use the native
#      V4L2 wrapper as the reference. HEVC Main10 uses the software HEVC decoder
#      converted to P010 because ffmpeg's hevc_v4l2m2m wrapper currently emits
#      invalid Main10 frames on this platform while the Rust VA path decodes the
#      same stream cleanly. Driver output must be byte-identical to the
#      reference output. Driver legs run under
#      tools/capture-iris-kernel-log.sh so iris firmware errors are attributed
#      to the leg that caused them, with V4L2_VA_DEBUG=1.
#   4. kernel classification exactly like tools/verify-eos-drain.sh:
#      fail on system-fatal(0x5000003) > 0, degraded on
#      session-fatal(0x4000003) > 0, else pass.
#
# Exit codes: 0 = at least one codec verified with no failure;
#             1 = at least one codec failed;
#             77 = nothing was verified (all codecs skipped, or preflight).
# No retries anywhere: a wedged node is reported, never hammered.
# All ffmpeg invocations use -nostdin (from automation, ffmpeg otherwise
# parks on a non-EOF stdin under SIGTERM and ignores the timeout).

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$repo_root/tools/hardware-session.sh"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
codec5_dir="${V4L2_VA_CODEC5_DIR:-/home/mq/tmp/vaatest/codec5}"
codec_frames="${V4L2_VA_CODEC_FRAMES:-30}"
work_dir="${V4L2_VA_CODEC5_LOG_DIR:-/tmp/libva-v4l2-codec5}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

# ---- visible constants: VAProfile name -> sample mapping + grep patterns ----
# Fields separated by '|' (NOT ':': the patterns themselves contain colons):
#   <codec>|<VAProfile>|<sample file>|<vainfo grep pattern>|<reference decoder>|<reference pix_fmt>
# Patterns are colon-anchored so VAProfileHEVCMain10 / VAProfileVP9Profile2
# etc. cannot false-match the base profile.
codec_specs=(
    "hevc|VAProfileHEVCMain|hevc-main-720p.mp4|VAProfileHEVCMain[[:space:]]*:|hevc_v4l2m2m|"
    "hevc10|VAProfileHEVCMain10|hevc-main10-720p.mp4|VAProfileHEVCMain10[[:space:]]*:|hevc|p010le"
    "vp9|VAProfileVP9Profile0|vp9-720p.webm|VAProfileVP9Profile0[[:space:]]*:|vp9_v4l2m2m|"
    "av1|VAProfileAV1Profile0|av1-720p.mp4|VAProfileAV1Profile0[[:space:]]*:|libdav1d|nv12"
)

mkdir -p "$work_dir"

if ! [[ "$codec_frames" =~ ^[1-9][0-9]*$ ]]; then
    codec_frames=30
fi

for tool in ffmpeg ffprobe vainfo; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "codec_expansion=skip reason=missing_tool=$tool"
        exit 77
    fi
done
if [[ ! -f "$kernel_tool" ]]; then
    echo "codec_expansion=skip reason=missing_kernel_tool=$kernel_tool"
    exit 77
fi

# Every decode child must hit the Rust driver; export once (the run_codec
# legs also pass it inline through env for self-containment).
export LIBVA_DRIVERS_PATH="$driver_dir"

# One vainfo probe for all codecs (profile advertisement is per-driver, not
# per-codec). Plain grep against the saved output below — never grep -q in a
# pipeline (under pipefail its early exit SIGPIPEs large producers).
set +e
timeout 60s vainfo --display drm --device "$drm_device" \
    > "$work_dir/vainfo.log" 2>&1
vainfo_status=$?
set -e

# Kernel attribution for one wrapped leg; prints "session system" counts,
# exactly like tools/verify-eos-drain.sh.
kernel_counts() { # <log>
    local kernel_line session_fatal system_fatal
    kernel_line="$(grep -m1 'summary:' "$1" || true)"
    session_fatal="$(sed -n 's/.*session-fatal(0x4000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    system_fatal="$(sed -n 's/.*system-fatal(0x5000003)=\([0-9]*\).*/\1/p' <<< "$kernel_line")"
    [[ -n "$session_fatal" ]] || session_fatal=NA
    [[ -n "$system_fatal" ]] || system_fatal=NA
    echo "$session_fatal $system_fatal"
}

# First per-frame record of a framemd5 file (comments start with '#').
first_frame_line() { # <framemd5 file>
    [[ -s "$1" ]] || { echo ""; return; }
    awk '!/^#/ && NF { print; exit }' "$1"
}

# Decode one bounded leg through the driver under the kernel-log wrapper.
# Sets leg_status; caller captures it immediately.
leg_status=0
decode_leg() { # <frames> <out.md5> <log> <file> <download format>
    set +e
    "$kernel_tool" -- timeout -k 5s 120s \
        env V4L2_VA_DEBUG=1 LIBVA_DRIVERS_PATH="$driver_dir" \
        ffmpeg -y -nostdin -hide_banner -v error -xerror \
        -hwaccel vaapi -hwaccel_output_format vaapi -hwaccel_device "$drm_device" \
        -i "$4" -map 0:v:0 -frames:v "$1" \
        -vf "hwdownload,format=$5" -f framemd5 "$2" \
        > "$3" 2>&1
    leg_status=$?
    set -e
    require_clean_kernel "$3"
}

run_codec() { # <name> <profile> <file> <pattern> <reference decoder> <reference pix_fmt>
    local name="$1" profile="$2" file="$3" pattern="$4" reference_decoder="$5" reference_pix_fmt="$6"
    local base="$work_dir/$name"
    local reference_md5="$base-reference-$codec_frames.md5" reference_log="$base-reference-$codec_frames.log"
    local ref_md5="$base-1f.md5" ref_log="$base-1f.log"
    local n_md5="$base-$codec_frames.md5" n_log="$base-$codec_frames.log"
    local reference_status ref_status n_status ref_session ref_system n_session n_system
    local reference_records ref_records n_records verdict reference_pix_args=()

    if [[ ! -f "$file" ]]; then
        echo "codec_$name=skip reason=missing_sample=$file"
        return 2
    fi
    if [[ "$vainfo_status" -ne 0 ]] \
        || ! grep -E "$pattern" "$work_dir/vainfo.log" >/dev/null; then
        echo "codec_$name=skip reason=profile_not_advertised profile=$profile vainfo_status=$vainfo_status log=$work_dir/vainfo.log"
        return 2
    fi
    if [[ -z "$reference_decoder" ]] || ! ffmpeg -hide_banner -decoders 2>/dev/null \
        | grep -F " $reference_decoder " >/dev/null; then
        echo "codec_$name=skip reason=reference_unavailable decoder=$reference_decoder"
        return 2
    fi
    if [[ -n "$reference_pix_fmt" ]]; then
        reference_pix_args=(-pix_fmt "$reference_pix_fmt")
    fi

    set +e
    run_kernel_checked "$reference_log" timeout -k 5s 120s ffmpeg -y -nostdin -hide_banner -v error \
        -c:v "$reference_decoder" -i "$file" -map 0:v:0 \
        -frames:v "$codec_frames" "${reference_pix_args[@]}" -f framemd5 "$reference_md5"

    reference_status=$?
    set -e
    if [[ "$reference_status" -ne 0 || ! -s "$reference_md5" ]]; then
        echo "codec_$name=skip reason=reference_failed status=$reference_status log=$reference_log"
        return 2
    fi

    local reference_count
    reference_count="$(awk '!/^#/ && NF {n++} END {print n+0}' "$reference_md5")"
    if [[ "$reference_count" != "$codec_frames" ]]; then
        echo "codec_$name=fail reason=reference_frame_count decoded=$reference_count expected=$codec_frames log=$reference_log"
        return 1
    fi

    # Leg 1: single-frame self-reference through the driver.
    decode_leg 1 "$ref_md5" "$ref_log" "$file" "${reference_pix_fmt:-nv12}"
    ref_status=$leg_status
    read -r ref_session ref_system <<< "$(kernel_counts "$ref_log")"
    if [[ "$ref_status" -ne 0 ]]; then
        echo "codec_$name=fail reason=self_ref_decode_failed status=$ref_status log=$ref_log"
        return 1
    fi

    # Leg 2: N-frame decode through the driver.
    decode_leg "$codec_frames" "$n_md5" "$n_log" "$file" "${reference_pix_fmt:-nv12}"
    n_status=$leg_status
    read -r n_session n_system <<< "$(kernel_counts "$n_log")"

    reference_records="$base-reference.records"
    ref_records="$base-1f.records"
    n_records="$base-$codec_frames.records"
    : > "$reference_records"
    : > "$ref_records"
    : > "$n_records"
    if [[ -s "$reference_md5" ]]; then
        awk '!/^#/ && NF {print $1, $2, $3, $4, $5, $6}' "$reference_md5" > "$reference_records"
    fi
    if [[ -s "$ref_md5" ]]; then
        awk '!/^#/ && NF {print $1, $2, $3, $4, $5, $6}' "$ref_md5" > "$ref_records"
    fi
    if [[ -s "$n_md5" ]]; then
        awk '!/^#/ && NF {print $1, $2, $3, $4, $5, $6}' "$n_md5" > "$n_records"
    fi

    verdict="pass reason=reference_parity"
    if [[ "$n_status" -ne 0 || ! -s "$n_md5" ]]; then
        verdict="fail reason=n_frame_decode_failed status=$n_status"
    elif [[ "$ref_status" -ne 0 || ! -s "$ref_records" ]]; then
        verdict="fail reason=self_ref_decode_failed status=$ref_status"
    elif ! cmp -s "$reference_records" "$n_records"; then
        verdict="fail reason=reference_parity_mismatch frames=$codec_frames"
    elif ! cmp -s "$ref_records" <(head -n 1 "$reference_records"); then
        verdict="fail reason=first_frame_reference_parity_mismatch"
    elif [[ "$ref_system" != NA && "$ref_system" -gt 0 ]] \
        || [[ "$n_system" != NA && "$n_system" -gt 0 ]]; then
        verdict="fail reason=firmware_system_fatal"
    elif [[ "$ref_session" != NA && "$ref_session" -gt 0 ]] \
        || [[ "$n_session" != NA && "$n_session" -gt 0 ]]; then
        verdict="degraded reason=session_abort_rescued status=0"
    fi

    echo "codec_$name=$verdict profile=$profile frames=$codec_frames reference=$reference_decoder pix_fmt=${reference_pix_fmt:-default} kernel_1f(session=$ref_session,system=$ref_system) kernel_n(session=$n_session,system=$n_system) log=$n_log"
    if [[ "$verdict" == fail* ]]; then
        return 1
    elif [[ "$verdict" == degraded* ]]; then
        return 3
    fi
    return 0
}

verified=0
failed=0
skipped=0

for spec in "${codec_specs[@]}"; do
    IFS='|' read -r name profile sample pattern reference_decoder reference_pix_fmt <<< "$spec"
    # Per-codec returns are control flow (2=skip, 3=degraded), not errors;
    # `|| rc=$?` keeps them errexit-safe (set -e is on after the vainfo
    # capture, exactly as in tools/verify-eos-drain.sh).
    rc=0
    run_codec "$name" "$profile" "$codec5_dir/$sample" "$pattern" "$reference_decoder" "$reference_pix_fmt" || rc=$?
    if [[ "$rc" -eq 2 ]]; then
        skipped=$((skipped + 1))
    elif [[ "$rc" -eq 1 ]]; then
        failed=$((failed + 1))
        # Preserve the failure and stop opening decoder sessions. A failed
        # direct-buffer leg is not a reason to submit another codec or retry.
        break
    else
        verified=$((verified + 1))
    fi
done

if [[ "$failed" -gt 0 ]]; then
    echo "codec_expansion=fail verified=$verified skipped=$skipped"
    exit 1
fi
if [[ "$verified" -eq 0 ]]; then
    # All codecs skipped: report no failures, but exit 77 per the skip-77
    # taxonomy so callers can tell "nothing verified" from a real pass.
    echo "codec_expansion=pass verified=0 skipped=$skipped"
    exit 77
fi
echo "codec_expansion=pass verified=$verified skipped=$skipped"
exit 0
