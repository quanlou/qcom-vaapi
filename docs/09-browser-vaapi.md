# 09 · Browser VA-API: why hardware decode falls back to software

> **Status:** historical diagnosis from 2026-09-18, qualification tooling updated
> 2026-10-01. Earlier logs demonstrate browser initialization blockers and later
> driver selection; they do not qualify current playback, GL correctness or
> teardown. Current source still requires a deployment-host browser run.

## The Phase 3 browser exit criterion

> Browser VA-API logs show hardware decode selected; playback does not fall back
> to software decode.

Two browsers are installed, both as **snaps**, on **aarch64 + Wayland**:
Chromium 152 and Firefox 147. The initial experiments below did not reach this
driver. The later Chromium experiment did reach it and encountered a decode
error; both observations are historical evidence rather than release approval.

## Evidence: the whole non-browser stack works

Measured on this host (outside any browser sandbox):

| Layer | Result |
|---|---|
| libva + this driver | `vainfo` loads `msm_drv_video_rs` and reports H.264 CB/Main/High VLD |
| Vulkan | `Adreno X1-85`, `turnip` Mesa driver, API 1.4.311 |
| OpenGL / EGL | `freedreno` / Adreno X1-85, OpenGL 4.6, GLES 3.2, Mesa 25.1.4 |
| Decode via FFmpeg/mpv/GStreamer | 720p H.264 works, byte-exact vs native |

These individual smoke checks do not establish sustained browser playback or
current GL export correctness. Browser flags and confinement can prevent driver
selection; decoder completion, synchronization and firmware stability remain
separate concerns after selection succeeds.

## Firefox: capability gate off, VAAPI never attempted

With `media.ffmpeg.vaapi.enabled=true` and
`media.hardware-video-decoding.force-enabled=true`, the RDD (decode) process
still builds this module order and decodes in software:

```
PDMInitializer, RDD PDM order:
  0: FFmpeg(FFVPX)
  1: FFmpeg(OS library)
  2: Agnostic
FFmpegVideoDecoder, init, IsHardwareAccelerated=0
```

- **No VA-API decoder module is registered at all**, and the log contains
  **zero** `vaapi` lines. Firefox never opens our driver
  (`msm_drv_video_rs` appears 0 times).
- The gate is `CanUseHardwareVideoDecoding`, computed in the GPU process from a
  DMABUF / VA-API probe and propagated to RDD. In the snap it comes back false,
  so the VA-API path is never even added. The `force-enabled` pref does not
  override it in this build/sandbox.

Probe result: `browser_vaapi_probe=hw_decode_gate_off`.

## Chromium: GPU process dies before VaapiVideoDecoder exists

```
ERROR ui/gl/init/gl_factory.cc: Requested GL implementation (gl=none,angle=none)
      not found in allowed implementations:
      [(gl=egl-angle,angle=opengl),(gl=egl-angle,angle=opengles),(gl=egl-angle,angle=vulkan)]
ERROR viz_main_impl.cc: Exiting GPU process due to errors during initialization
```

- ANGLE's GL/GLES backend fails to initialize inside the snap, software GL
  (SwiftShader) is disallowed, so the GPU process exits. Chromium's
  `VaapiVideoDecoder` **requires a live GPU process**, so VA-API is impossible —
  the process never gets far enough to open any libva driver.
- This is confinement, not the GL stack: the same Adreno GL/Vulkan works
  natively (table above), and the user's normal (unforced) Chromium renders
  fine.

Probe result (forced-flags mode): `browser_vaapi_probe=gpu_gl_init_failed`.

### RESOLVED for snap Chromium (2026-09-18)

The GPU-process death was caused by the probe's own forced GL flags. With
`V4L2_VA_BROWSER_CHROMIUM_MODE=native` (no `--use-gl`/`--use-angle` at all),
snap Chromium keeps its normal working GL path, the GPU process survives, and
`VaapiVideoDecoder` **selects this driver** for H.264 Main 1280×720. It then hit
one driver bug — `vaCreateSurfaces (allocate mode) failed, VA error: attribute
not supported` — because Chromium passes a SETTABLE `VASurfaceAttribUsageHint`
that `validate_surface_creation_attributes` rejected. That is now accepted and
ignored (advisory hint), and Chromium drives the full pipeline: create decoder →
negotiate OUTPUT=H264 / CAPTURE=NV12 → `REQBUFS count=32` → decode →
`vaSyncSurface`. Probe result: `browser_vaapi_probe=reached_driver`.

The remaining failure is `vaSyncSurface: internal decoding error`
(`VA_STATUS_ERROR_DECODING_ERROR`) — the same decode-failure class every client
hits while `/dev/video16` is firmware-poisoned, not a browser issue. Confirming
clean end-to-end browser decode needs a quiet node window and possibly more work
on Chromium's allocate-mode surface/decode flow (which differs from FFmpeg's).

## What actually unblocks this

Ordered by likelihood of success:

1. **Run an unconfined (non-snap) browser.** This removes every blocker above in
   one move: the GPU process gets the working host GL/Vulkan, `LIBVA_DRIVER_NAME=msm`
   + `LIBVA_DRIVERS_PATH` reach the decode process, and `/dev/video16` is
   accessible. On aarch64 the practical option is Mozilla's official Firefox
   **aarch64 tarball** (Chrome ships no ARM64 Linux build). This requires the
   user to install it; it is the single highest-value step for Phase 3's browser
   criterion.
2. **Snap Chromium without forced GL flags** (`V4L2_VA_BROWSER_CHROMIUM_MODE=native`)
   or **ANGLE-over-Vulkan** (`=vulkan`, Adreno turnip). Forcing
   `--use-gl=egl-angle --use-angle=opengles` may itself be killing the GPU
   process; letting Chromium keep its working default, or using Vulkan, may keep
   the GPU process alive far enough to probe VA-API. Even then, the snap may
   still block `/dev/video16` and the driver `.so` path, so this is a partial
   experiment, not a guaranteed unblock. `tools/verify-browser-vaapi.sh` now
   supports both modes.
3. **Snap permission plugs / connecting the snap to the driver and device** —
   requires root and per-snap `snap connect`; out of scope unprivileged.

## How to reproduce / iterate

```sh
# Firefox (software fallback, capability gate off):
V4L2_VA_BROWSER=firefox tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver

# Chromium, try to keep the GPU process alive:
V4L2_VA_BROWSER=chromium V4L2_VA_BROWSER_CHROMIUM_MODE=native \
  tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver
V4L2_VA_BROWSER=chromium V4L2_VA_BROWSER_CHROMIUM_MODE=vulkan \
  tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver

# Unconfined browser (once installed), pass its binary explicitly:
V4L2_VA_BROWSER=/opt/firefox/firefox tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver
```

The diagnostic probe prints one of: `reached_driver` (driver-load diagnostic,
`qualification=not_proven`, not a playback qualification),
`hw_decode_gate_off` (Firefox never tried VAAPI), `gpu_gl_init_failed` (Chromium
GPU process died), `software_decoder`, or `blocked_*`.

## Deployment playback and performance qualification

`V4L2_VA_BROWSER_STRICT=1` (also inherited from `V4L2_VA_STRICT=1`) requires an
explicit positive FPS requirement, RSS budget, and sustained measurement
duration. Select these for the deployment workload; the numbers below are
examples, not universal hardware limits:

```sh
V4L2_VA_BROWSER=chromium V4L2_VA_BROWSER_STRICT=1 \
V4L2_VA_BROWSER_MIN_FPS=24 V4L2_VA_BROWSER_MAX_RSS_KIB=2097152 \
V4L2_VA_BROWSER_MIN_SECONDS=20 V4L2_VA_BROWSER_SECONDS=30 \
  tools/verify-browser-vaapi.sh /path/to/qualified-driver
```

The sample must be more than eight seconds long. A fresh private profile and
run ID bind telemetry to one browser invocation. The page reports forward
playback, requests an actual seek, checks the resulting playback position,
reports progress afterwards, and closes its window. Chromium runs an app
window; the private Firefox profile enables script-driven window closure. Use
`V4L2_VA_BROWSER_KIND=firefox` for a custom Firefox launcher whose executable
name does not identify it. Chromium defaults to `native` GL selection.

Strict acceptance additionally requires at least as many actual driver CAPTURE
publications as observed video frames, rejects logged software fallback or
decode failure, checks all seven kernel/firmware summary counters, and requires
a natural zero-status browser exit with no observed surviving descendants.
A timeout, blocked closure, failed seek, missing frame-quality API or missing
kernel log access fails qualification. The driver name is fixed to `msm`.
The local HTTP server only accepts bounded same-origin telemetry.

`measurement.json` records observed process-tree peak RSS; `events.jsonl` holds
the playback evidence; `performance.json` records presented FPS (dropped frames
excluded), the memory budget result, seek evidence and clean process exit.
`V4L2_VA_BROWSER_MAX_DROP_RATIO` defaults to 0.01. RSS is sampled every 100 ms
and sums shared pages separately for each observed process; it is a conservative
process budget rather than unique memory. PPID ancestry and PID/start-time
tracking retain observed descendants that change sessions or are reparented.
Very short-lived peaks or descendants that detach completely between samples
may evade observation. This does not prove pixel parity at seek targets,
zero-copy rendering, or absence of gradual leaks across days of use.

The default diagnostic mode can still return zero for `reached_driver` even
when the browser later times out. Only a strict `browser_vaapi_probe=pass
qualification=playback_seek_and_clean_exit` result satisfies this browser gate.
The headless production gate records browser qualification as separate; run
this strict check in the actual deployment session on the same driver, kernel,
firmware and fixtures. Browser playback/performance has not yet been qualified
for the active candidate. The physical host currently has an active Iris
candidate and a serialized strict hardware gate; wait for that gate to finish
before opening another decode session. See the [current resumption report](production-resumption-20261001.txt).

## Sustained 4K CPU-download qualification

The 4K verifier preserves required 1-frame, 30-frame and full-stream byte-exact
checks, including repeated playback. It forces `LIBVA_DRIVER_NAME=msm` and
rejects stale result directories. Default measurements report
`performance=unqualified` because no deployment thresholds have been supplied.
Strict mode requires positive thresholds and rejects nonfinite, malformed,
zero or fractional frame/RSS measurements. It also checks complete clean kernel
evidence for both the native reference and driver legs.

```sh
V4L2_VA_4K_STRICT=1 V4L2_VA_4K_LOOPS=10 \
V4L2_VA_4K_MIN_FPS=30 V4L2_VA_4K_MAX_RSS_KIB=1048576 \
V4L2_VA_4K_MIN_SECONDS=30 V4L2_VA_4K_LOG_DIR=/tmp/fresh-4k-results \
  tools/verify-4k-decode.sh /path/to/qualified-driver
```

Choose a clip and repeat count that exercise the required duration within the
120-second command limit. `performance.json` reports end-to-end FFmpeg decode,
CPU download and checksum throughput and peak FFmpeg RSS; this is not a
zero-copy rendering benchmark or a full device-memory accounting result.
Run for every supported codec/workload needed in the deployment. Both browser
and 4K tools take the shared hardware lock to avoid simultaneous firmware use.
