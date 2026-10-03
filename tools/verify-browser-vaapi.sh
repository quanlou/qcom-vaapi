#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
driver_dir="${1:-/tmp/libva-v4l2-rust-driver}"
sample="${V4L2_VA_SAMPLE:-/home/mq/tmp/vaatest/test_720p.mp4}"
browser="${V4L2_VA_BROWSER:-chromium}"
duration="${V4L2_VA_BROWSER_SECONDS:-30}"
work_dir="${V4L2_VA_BROWSER_WORK_DIR:-$HOME/.cache/libva-v4l2-browser-verify}"
chromium_mode="${V4L2_VA_BROWSER_CHROMIUM_MODE:-native}"
strict="${V4L2_VA_BROWSER_STRICT:-${V4L2_VA_STRICT:-0}}"
if [[ ! "$duration" =~ ^[0-9]+$ || "$duration" -lt 20 || "$duration" -gt 300 ||
      ( "$strict" != 0 && "$strict" != 1 ) ]]; then
    echo "browser_vaapi_probe=fail reason=invalid_duration_or_strict_mode"
    exit 1
fi
if [[ "${LIBVA_DRIVER_NAME:-msm}" != msm ]]; then
    echo "browser_vaapi_probe=fail reason=incorrect_driver"
    exit 1
fi
measurement_args=(--minimum-fps "${V4L2_VA_BROWSER_MIN_FPS:-0}"
    --maximum-rss-kib "${V4L2_VA_BROWSER_MAX_RSS_KIB:-0}"
    --minimum-seconds "${V4L2_VA_BROWSER_MIN_SECONDS:-0}")
[[ "$strict" == 0 ]] || measurement_args+=(--strict)
python3 "$repo_root/tools/check-playback-performance.py" thresholds "${measurement_args[@]}"
missing() {
    echo "browser_vaapi_probe=blocked reason=$1 qualification=not_proven"
    [[ "$strict" == 0 ]] && exit 77
    exit 1
}

if [[ ! -f "$sample" ]]; then
    missing "missing_sample:$sample"
fi
if [[ -z "${DISPLAY:-}" && -z "${WAYLAND_DISPLAY:-}" ]]; then
    missing no-graphical-session
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
    missing "browser-not-found:$browser"
fi
browser_kind="${V4L2_VA_BROWSER_KIND:-}"
if [[ -z "$browser_kind" ]]; then
    case "$(basename "$browser_bin")" in
        *firefox*) browser_kind=firefox ;;
        *) browser_kind=chromium ;;
    esac
fi
if [[ "$browser_kind" != firefox && "$browser_kind" != chromium ]]; then
    echo "browser_vaapi_probe=fail reason=invalid_browser_kind"
    exit 1
fi

# Ubuntu's /usr/bin/firefox is a wrapper around the Firefox snap. Resolve that
# wrapper here so the sample, profile, and driver all live in snap-visible
# storage instead of producing a misleading "no page request" result.
if [[ "$browser_kind" == firefox && "$browser_bin" == /usr/bin/firefox && -x /snap/bin/firefox ]]; then
    browser_bin="/snap/bin/firefox"
fi

# Chromium installed as a snap cannot reliably see arbitrary hidden host paths
# such as ~/.cache. Use the snap common directory unless the caller explicitly
# selected a work directory.
if [[ -z "${V4L2_VA_BROWSER_WORK_DIR:-}" && "$browser_bin" == /snap/* ]]; then
    work_dir="$HOME/snap/$browser/common/libva-v4l2-browser-verify"
fi
mkdir -p "$work_dir"
exec 9>/tmp/libva-v4l2-hardware.lock
flock -n 9 || { echo "browser_vaapi_probe=fail reason=hardware_busy"; exit 1; }
if [[ "$strict" == 1 && ( ! -c "${V4L2_VA_DEVICE:-/dev/video16}" || ! -c "${V4L2_VA_DRM_DEVICE:-/dev/dri/renderD128}" ) ]]; then
    echo "browser_vaapi_probe=blocked reason=missing_hardware qualification=not_proven"
    exit 1
fi

# Chromium installed as a snap cannot reliably see /tmp from the host namespace.
# Build/copy the driver under $HOME so the sandboxed browser can at least try to
# load the custom libva driver.
if [[ "$browser_bin" == /snap/* && "$driver_dir" == /tmp/* ]]; then
    driver_dir="$work_dir/driver"
    "$repo_root/tools/build-rust-driver.sh" "$driver_dir" >/dev/null
fi

run_dir="$work_dir/run-$(date +%s)-$$"
mkdir -p "$run_dir"
run_id="$(python3 -c 'import secrets; print(secrets.token_hex(16))')"
cp "$sample" "$run_dir/sample.mp4"

html="$run_dir/video.html"
cat > "$html" <<HTML
<!doctype html>
<meta charset="utf-8">
<style>html,body{margin:0;background:#111}video{display:block;max-width:100vw;max-height:100vh;margin:auto}</style>
<video id="v" src="/sample.mp4" autoplay muted loop playsinline controls></video>
<script>
const v = document.getElementById('v');
const started = performance.now();
let seekStarted = false, seekFinished = false, afterSeek = false;
let playing = false, soughtTime = 0, finished = false;
let reports = Promise.resolve();
function report(event, extra={}) {
  const q = v.getVideoPlaybackQuality();
  const payload = {event, run_id:'$run_id', time:v.currentTime, elapsed:(performance.now()-started)/1000,
    total:q.totalVideoFrames, dropped:q.droppedVideoFrames, ready:v.readyState, ...extra};
  reports = reports.then(() => fetch('/telemetry', {method:'POST',
    headers:{'Content-Type':'application/json'}, body:JSON.stringify(payload)}))
    .then(response => {if (!response.ok) throw new Error('telemetry rejected');});
  return reports;
}
v.addEventListener('playing', () => {if (!playing) {playing=true; report('playing');}});
v.addEventListener('error', () => report('error', {message:'video error'}));
v.addEventListener('seeked', () => {
  if (seekStarted && !seekFinished) {seekFinished=true; soughtTime=v.currentTime; report('seeked');}
});
const tick = setInterval(() => {
  const elapsed = (performance.now()-started)/1000;
  if (!seekStarted && playing && elapsed >= 5 && Number.isFinite(v.duration) && v.duration > 8) {
    report('before_seek');
    const target = v.currentTime < v.duration/2 ? v.duration*0.75 : v.duration*0.25;
    seekStarted=true; report('seek_requested', {target}); v.currentTime=target;
  }
  if (seekFinished && !afterSeek && v.currentTime-soughtTime >= 1.5) {
    afterSeek=true; report('after_seek');
  }
  if (!finished && elapsed >= $((duration - 5))) {
    finished=true; clearInterval(tick); v.pause();
    report('finished').then(() => {
      v.removeAttribute('src'); v.load();
      report('close_requested').then(() => {
        try {window.close();} catch (e) {report('close_error', {message:String(e)});}
        setTimeout(() => report('close_still_open', {hidden:document.hidden}), 1000);
      });
    });
  }
}, 100);
v.play().catch(e => report('error', {message:String(e)}));
</script>
HTML

server_log="$run_dir/http.log"
python3 "$repo_root/tools/browser-playback-server.py" --directory "$run_dir" \
    --events "$run_dir/events.jsonl" --port-file "$run_dir/port" > "$server_log" 2>&1 &
server_pid=$!
cleanup() {
    if [[ -n "${monitor_pid:-}" ]]; then
        kill "$monitor_pid" >/dev/null 2>&1 || true
        wait "$monitor_pid" 2>/dev/null || true
    fi
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
for _ in {1..50}; do
    [[ ! -f "$run_dir/port" ]] || break
    kill -0 "$server_pid" 2>/dev/null || { echo "browser_vaapi_probe=fail reason=server_start_failed"; exit 1; }
    sleep 0.1
done
[[ -f "$run_dir/port" ]] || { echo "browser_vaapi_probe=fail reason=server_start_timeout"; exit 1; }
port="$(cat "$run_dir/port")"

log="$run_dir/$browser.log"
profile="$run_dir/$browser-profile"
mkdir -p "$profile"
url="http://127.0.0.1:$port/video.html"

status=0
# The configured launch deadline includes the five-second natural-close window.
# Cleanup after a timeout cannot turn a timed-out process into a passing exit.
measure=(python3 "$repo_root/tools/measure-process-tree.py" --seconds "$duration" --output "$run_dir/measurement.json" --run-id "$run_id" --)
kernel=()
[[ "$strict" == 0 ]] || kernel=("$repo_root/tools/capture-iris-kernel-log.sh" --)
case "$browser_kind" in
    firefox)
        cat > "$profile/user.js" <<'JS'
user_pref("media.ffmpeg.vaapi.enabled", true);
user_pref("media.hardware-video-decoding.force-enabled", true);
user_pref("media.autoplay.default", 0);
user_pref("dom.allow_scripts_to_close_windows", true);
user_pref("browser.aboutwelcome.enabled", false);
// A background first-run privacy tab otherwise survives the video window.
user_pref("datareporting.policy.firstRunURL", "");
user_pref("browser.startup.homepage_override.mstone", "ignore");
user_pref("browser.shell.checkDefaultBrowser", false);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.warnOnQuit", false);
user_pref("browser.tabs.warnOnClose", false);
JS
        set +e
        "${kernel[@]}" "${measure[@]}" env \
            LIBVA_DRIVERS_PATH="$driver_dir" \
            LIBVA_DRIVER_NAME=msm \
            V4L2_VA_DEBUG=1 \
            MOZ_ENABLE_WAYLAND=1 \
            MOZ_DISABLE_RDD_SANDBOX=1 \
            MOZ_LOG="PlatformDecoderModule:5,Dmabuf:5,FFmpegVideo:5" \
            "$browser_bin" --no-remote --profile "$profile" "$url" \
            > "$log" 2>&1 &
        monitor_pid=$!
        wait "$monitor_pid"
        status=$?
        monitor_pid=""
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
            --app="$url"
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
                echo "browser_vaapi_probe=fail reason=unknown-chromium-mode mode=$chromium_mode"
                exit 1
                ;;
        esac
        set +e
        "${kernel[@]}" "${measure[@]}" env \
            LIBVA_DRIVERS_PATH="$driver_dir" \
            LIBVA_DRIVER_NAME=msm \
            V4L2_VA_DEBUG=1 \
            "$browser_bin" "${chromium_flags[@]}" \
            > "$log" 2>&1 &
        monitor_pid=$!
        wait "$monitor_pid"
        status=$?
        monitor_pid=""
        set -e
        ;;
esac

# Strict qualification uses progressing video, a real seek, actual driver
# CAPTURE publications, no fallback, a bounded memory budget and clean exit.
if [[ "$strict" == 1 ]]; then
    if [[ "$status" != 0 ]] || ! python3 "$repo_root/tools/check-playback-performance.py" browser \
        "${measurement_args[@]}" --log "$log" --events "$run_dir/events.jsonl" \
        --run-id "$run_id" \
        --measurement "$run_dir/measurement.json" \
        --maximum-drop-ratio "${V4L2_VA_BROWSER_MAX_DROP_RATIO:-0.01}" --output "$run_dir/performance.json"; then
        echo "browser_vaapi_probe=fail reason=deployment_evidence_incomplete status=$status log=$log telemetry=$run_dir/events.jsonl"
        exit 1
    fi
    echo "browser_vaapi_probe=pass qualification=playback_seek_and_clean_exit browser=$browser log=$log"
    exit 0
fi

# A diagnostic load result says only that the driver was opened. It does not
# satisfy production browser playback, seek or teardown qualification.
if grep -q 'msm_drv_video_rs' "$log"; then
    echo "browser_vaapi_probe=reached_driver qualification=not_proven browser=$browser status=$status log=$log"
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
