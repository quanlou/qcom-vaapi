#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
browser="${V4L2_VA_BROWSER:-chromium}"
duration="${V4L2_VA_BROWSER_SECONDS:-20}"
work_dir="${V4L2_VA_BROWSER_WORK_DIR:-$HOME/.cache/libva-v4l2-browser-verify}"
chromium_mode="${V4L2_VA_BROWSER_CHROMIUM_MODE:-auto}"

if [[ ! -f "$sample" ]]; then
    echo "browser_vaapi_probe=skip missing_sample=$sample"
    exit 77
fi
if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    echo "browser_vaapi_probe=skip reason=no-graphical-session"
    exit 77
fi

find_browser() {
    case "$browser" in
        chromium)
            command -v chromium || command -v chromium-browser || command -v google-chrome || command -v google-chrome-stable
            ;;
        firefox)
            command -v firefox || command -v firefox-esr
            ;;
        *)
            command -v "$browser"
            ;;
    esac
}

browser_bin="$(find_browser || true)"
if [[ -z "$browser_bin" ]]; then
    echo "browser_vaapi_probe=skip reason=browser-not-found browser=$browser"
    exit 77
fi

# Ubuntu's /usr/bin/firefox is a wrapper around the Firefox snap. Resolve that
# wrapper here so the sample, profile, and driver all live in snap-visible
# storage instead of producing a misleading "no page request" result.
if [[ "$browser" == firefox && "$browser_bin" == /usr/bin/firefox && -x /snap/bin/firefox ]]; then
    browser_bin="/snap/bin/firefox"
fi

# Chromium installed as a snap cannot reliably see arbitrary hidden host paths
# such as ~/.cache. Use the snap common directory unless the caller explicitly
# selected a work directory.
if [[ -z "${V4L2_VA_BROWSER_WORK_DIR:-}" && "$browser_bin" == /snap/* ]]; then
    work_dir="$HOME/snap/$browser/common/libva-v4l2-browser-verify"
fi
mkdir -p "$work_dir"

# Chromium installed as a snap cannot reliably see /tmp from the host namespace.
# Build/copy the driver under $HOME so the sandboxed browser can at least try to
# load the custom libva driver.
if [[ "$browser_bin" == /snap/* && "$driver_dir" == /tmp/* ]]; then
    driver_dir="$work_dir/driver"
    "$repo_root/tools/build-rust-driver.sh" "$driver_dir" >/dev/null
fi

run_dir="$work_dir/run-$(date +%s)-$$"
mkdir -p "$run_dir"
cp "$sample" "$run_dir/sample.mp4"

html="$run_dir/video.html"
cat > "$html" <<HTML
<!doctype html>
<meta charset="utf-8">
<video id="v" src="/sample.mp4" autoplay muted loop playsinline controls></video>
<script>
const v = document.getElementById('v');
v.play().catch(e => console.log('play failed', e));
setInterval(() => console.log('video', v.readyState, v.currentTime), 1000);
</script>
HTML

port="$(python3 - <<'PY'
import socket
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
    s.bind(("127.0.0.1", 0))
    print(s.getsockname()[1])
PY
)"
server_log="$run_dir/http.log"
python3 -m http.server "$port" --bind 127.0.0.1 --directory "$run_dir" > "$server_log" 2>&1 &
server_pid=$!
cleanup() {
    kill "$server_pid" >/dev/null 2>&1 || true
}
trap cleanup EXIT

log="$run_dir/$browser.log"
profile="$run_dir/$browser-profile"
mkdir -p "$profile"
url="http://127.0.0.1:$port/video.html"

status=0
case "$browser" in
    firefox)
        cat > "$profile/user.js" <<'JS'
user_pref("media.ffmpeg.vaapi.enabled", true);
user_pref("media.hardware-video-decoding.force-enabled", true);
user_pref("media.autoplay.default", 0);
JS
        set +e
        timeout "$duration"s env \
            LIBVA_DRIVERS_PATH="$driver_dir" \
            LIBVA_DRIVER_NAME=msm \
            V4L2_VA_DEBUG=1 \
            MOZ_ENABLE_WAYLAND=1 \
            MOZ_DISABLE_RDD_SANDBOX=1 \
            MOZ_LOG="PlatformDecoderModule:5,DMABUF:5,FFmpegVideo:5" \
            "$browser_bin" --no-remote --profile "$profile" "$url" \
            > "$log" 2>&1
        status=$?
        set -e
        ;;
    *)
        chromium_flags=(
            --user-data-dir="$profile"
            --no-first-run
            --disable-background-networking
            --autoplay-policy=no-user-gesture-required
            --ignore-gpu-blocklist
            --enable-features=VaapiVideoDecoder,VaapiVideoDecodeLinuxGL,AcceleratedVideoDecodeLinuxGL
            --enable-logging=stderr
            --vmodule='*vaapi*=3,*video*=2,*media*=2'
            --ozone-platform="${XDG_SESSION_TYPE:-wayland}"
        )
        # GL backend selection. On this host GL/EGL (freedreno) and Vulkan
        # (turnip) both work natively (see docs/09-browser-vaapi.md), so a snap
        # GPU-process death is confinement, not the GL stack. Modes:
        #   auto      : snap -> ANGLE/GLES (historical default); else EGL.
        #   vulkan    : ANGLE-over-Vulkan (Adreno turnip) — most likely to bring
        #               up a snap GPU process when GLES ANGLE dies.
        #   native    : pass no --use-gl at all; let the browser pick the same
        #               working default it uses for normal rendering. Forced GL
        #               flags can themselves kill the GPU process.
        #   in-process: auto GL + --in-process-gpu (no separate GPU process).
        case "$chromium_mode" in
            auto)
                if [[ "$browser_bin" == /snap/* ]]; then
                    chromium_flags+=(--use-gl=egl-angle --use-angle=opengles)
                else
                    chromium_flags+=(--use-gl=egl)
                fi
                ;;
            vulkan)
                chromium_flags+=(--use-gl=angle --use-angle=vulkan --enable-features=Vulkan)
                ;;
            native)
                : # no forced GL flags; use the browser's working default
                ;;
            in-process)
                if [[ "$browser_bin" == /snap/* ]]; then
                    chromium_flags+=(--use-gl=egl-angle --use-angle=opengles)
                else
                    chromium_flags+=(--use-gl=egl)
                fi
                chromium_flags+=(--in-process-gpu)
                ;;
            *)
                echo "browser_vaapi_probe=skip reason=unknown-chromium-mode mode=$chromium_mode"
                exit 77
                ;;
        esac
        set +e
        timeout "$duration"s env \
            LIBVA_DRIVERS_PATH="$driver_dir" \
            LIBVA_DRIVER_NAME=msm \
            V4L2_VA_DEBUG=1 \
            "$browser_bin" "${chromium_flags[@]}" "$url" \
            > "$log" 2>&1
        status=$?
        set -e
        ;;
esac

# Success: our libva driver was actually loaded by the browser's GPU/decode
# process (its version string appears in the log).
if grep -q 'msm_drv_video_rs' "$log"; then
    echo "browser_vaapi_probe=reached_driver browser=$browser status=$status log=$log"
    exit 0
fi

if ! grep -q 'GET /video.html' "$server_log"; then
    echo "browser_vaapi_probe=blocked_no_page_request browser=$browser status=$status log=$log http_log=$server_log"
    exit 1
fi

# Chromium: the GPU process failed to initialize any GL/ANGLE backend, so it
# exits and no VaapiVideoDecoder can ever run. On this host GL and Vulkan both
# work natively (docs/09-browser-vaapi.md), so this is snap confinement, not the
# GL stack. Try `V4L2_VA_BROWSER_CHROMIUM_MODE=vulkan` or `=native`, or an
# unconfined (non-snap) Chromium.
if grep -q 'Requested GL implementation (gl=none' "$log" ||
    grep -q 'Exiting GPU process due to errors during initialization' "$log"; then
    diag="$(grep -aoE 'Requested GL implementation \(gl=[^)]*\)[^]]*|not found in allowed implementations: \[[^]]*\]' "$log" | head -1)"
    echo "browser_vaapi_probe=gpu_gl_init_failed browser=$browser mode=$chromium_mode status=$status log=$log"
    [[ -n "$diag" ]] && echo "  diag: $diag"
    exit 1
fi

# Firefox: the RDD process built its decoder module list with no VA-API entry
# and initialized FFmpeg in software (IsHardwareAccelerated=0). It never tried
# VAAPI and never opened our driver. The hardware-video-decoding capability gate
# (set in the GPU process from a DMABUF/VA-API probe) is off — again a snap
# confinement problem, upstream of this driver.
if grep -q 'IsHardwareAccelerated=0' "$log" && ! grep -qai 'vaapi' "$log"; then
    echo "browser_vaapi_probe=hw_decode_gate_off browser=$browser status=$status log=$log"
    echo "  diag: RDD initialized FFmpeg in software; no VA-API decoder module registered, no VAAPI attempt."
    exit 1
fi

if grep -Eqi 'FFMPEG: Initialising FFmpeg decoder|FFmpeg decoder init successful|FFmpegVideoDecoder, init, IsHardwareAccelerated=0' "$log"; then
    echo "browser_vaapi_probe=software_decoder browser=$browser status=$status log=$log"
    exit 1
fi

if grep -Eiq 'denied|sandbox|permission|not supported|unsupported|failed|error' "$log"; then
    echo "browser_vaapi_probe=blocked_or_no_driver_call browser=$browser status=$status log=$log"
    exit 1
fi

echo "browser_vaapi_probe=no_driver_call browser=$browser status=$status log=$log"
exit 1
