#!/usr/bin/env bash
set -uo pipefail

# Codec-expansion verification probe (Phase 5 track B): HEVC / VP9 / AV1
# decode through the Rust V4L2 VA driver, one codec at a time.
#
# Per codec (skip-77 taxonomy, per-codec skips — one wedged/unsupported codec
# must not mask the others):
#   1. sample present in V4L2_VA_CODEC5_DIR (see MANIFEST.txt there), else
#      `codec_<name>=skip reason=missing_sample=...`
#   2. vainfo through the driver must advertise the codec's VAProfile, else
#      `codec_<name>=skip reason=profile_not_advertised`
#   3. a 1-frame framemd5 decode through the driver as self-reference, then a
#      N-frame (V4L2_VA_CODEC_FRAMES, default 30) framemd5; frame 1 must be
#      byte-identical between the two legs. Both legs run under
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
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
drm_device="${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}"
codec5_dir="${V4L2_VA_CODEC5_DIR:-/home/mq/tmp/vaatest/codec5}"
codec_frames="${V4L2_VA_CODEC_FRAMES:-30}"
work_dir="${V4L2_VA_CODEC5_LOG_DIR:-/tmp/libva-v4l2-codec5}"
kernel_tool="$repo_root/tools/capture-iris-kernel-log.sh"

# ---- visible constants: VAProfile name -> sample mapping + grep patterns ----
# Fields separated by '|' (NOT ':': the patterns themselves contain colons):
#   <codec>|<VAProfile>|<sample file>|<vainfo grep pattern>
# Patterns are colon-anchored so VAProfileHEVCMain10 / VAProfileVP9Profile2
# etc. cannot false-match the base profile.
codec_specs=(
    "hevc|VAProfileHEVCMain|hevc-main-720p.mp4|VAProfileHEVCMain[[:space:]]*:"
    "vp9|VAProfileVP9Profile0|vp9-720p.webm|VAProfileVP9Profile0[[:space:]]*:"
    "av1|VAProfileAV1Profile0|av1-720p.mp4|VAProfileAV1Profile0[[:space:]]*:"
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
decode_leg() { # <frames> <out.md5> <log> <file>
    set +e
    timeout 120s "$kernel_tool" -- \
        env V4L2_VA_DEBUG=1 LIBVA_DRIVERS_PATH="$driver_dir" \
        ffmpeg -nostdin -hide_banner -v error \
        -hwaccel vaapi -hwaccel_device "$drm_device" \
        -i "$4" -map 0:v:0 -frames:v "$1" -f framemd5 "$2" \
        > "$3" 2>&1
    leg_status=$?
    set -e
}

run_codec() { # <name> <profile> <file> <pattern>
    local name="$1" profile="$2" file="$3" pattern="$4"
    local base="$work_dir/$name"
    local ref_md5="$base-1f.md5" ref_log="$base-1f.log"
    local n_md5="$base-$codec_frames.md5" n_log="$base-$codec_frames.log"
    local ref_status n_status ref_session ref_system n_session n_system
    local ref_line n_line verdict

    if [[ ! -f "$file" ]]; then
        echo "codec_$name=skip reason=missing_sample=$file"
        return 2
    fi
    if [[ "$vainfo_status" -ne 0 ]] \
        || ! grep -E "$pattern" "$work_dir/vainfo.log" >/dev/null; then
        echo "codec_$name=skip reason=profile_not_advertised profile=$profile vainfo_status=$vainfo_status log=$work_dir/vainfo.log"
        return 2
    fi

    # Leg 1: single-frame self-reference through the driver.
    decode_leg 1 "$ref_md5" "$ref_log" "$file"
    ref_status=$leg_status
    read -r ref_session ref_system <<< "$(kernel_counts "$ref_log")"

    # Leg 2: N-frame decode through the driver.
    decode_leg "$codec_frames" "$n_md5" "$n_log" "$file"
    n_status=$leg_status
    read -r n_session n_system <<< "$(kernel_counts "$n_log")"

    ref_line="$(first_frame_line "$ref_md5")"
    n_line="$(first_frame_line "$n_md5")"

    verdict="pass reason=frame1_matches_n"
    if [[ "$n_status" -ne 0 || ! -s "$n_md5" ]]; then
        verdict="fail reason=n_frame_decode_failed status=$n_status"
    elif [[ "$ref_status" -ne 0 || -z "$ref_line" ]]; then
        verdict="fail reason=self_ref_decode_failed status=$ref_status"
    elif [[ "$ref_line" != "$n_line" ]]; then
        verdict="fail reason=parity_mismatch frames=$codec_frames"
    elif [[ "$ref_system" != NA && "$ref_system" -gt 0 ]] \
        || [[ "$n_system" != NA && "$n_system" -gt 0 ]]; then
        verdict="fail reason=firmware_system_fatal"
    elif [[ "$ref_session" != NA && "$ref_session" -gt 0 ]] \
        || [[ "$n_session" != NA && "$n_session" -gt 0 ]]; then
        verdict="degraded reason=session_abort_rescued status=0"
    fi

    echo "codec_$name=$verdict profile=$profile frames=$codec_frames kernel_1f(session=$ref_session,system=$ref_system) kernel_n(session=$n_session,system=$n_system) log=$n_log"
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
    IFS='|' read -r name profile sample pattern <<< "$spec"
    # Per-codec returns are control flow (2=skip, 3=degraded), not errors;
    # `|| rc=$?` keeps them errexit-safe (set -e is on after the vainfo
    # capture, exactly as in tools/verify-eos-drain.sh).
    rc=0
    run_codec "$name" "$profile" "$codec5_dir/$sample" "$pattern" || rc=$?
    if [[ "$rc" -eq 2 ]]; then
        skipped=$((skipped + 1))
    elif [[ "$rc" -eq 1 ]]; then
        failed=$((failed + 1))
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
