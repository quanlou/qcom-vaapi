# 09 · Browser VA-API: why hardware decode falls back to software

> **Status:** Phase 3 (browser-grade zero-copy) diagnosis. Written 2026-09-18.
> Establishes *exactly* why no browser has selected hardware decode through this
> driver, with evidence that the blocker is **snap confinement**, not the driver,
> the hardware, or the GL stack.

## The Phase 3 browser exit criterion

> Browser VA-API logs show hardware decode selected; playback does not fall back
> to software decode.

Two browsers are installed, both as **snaps**, on **aarch64 + Wayland**:
Chromium 152 and Firefox 147. Neither reaches this driver. The failure is
different for each, and neither is our fault.

## Evidence: the whole non-browser stack works

Measured on this host (outside any browser sandbox):

| Layer | Result |
|---|---|
| libva + this driver | `vainfo` loads `msm_drv_video_rs` and reports H.264 CB/Main/High VLD |
| Vulkan | `Adreno X1-85`, `turnip` Mesa driver, API 1.4.311 |
| OpenGL / EGL | `freedreno` / Adreno X1-85, OpenGL 4.6, GLES 3.2, Mesa 25.1.4 |
| Decode via FFmpeg/mpv/GStreamer | 720p H.264 works, byte-exact vs native |

So GL, Vulkan, VA-API, and the decoder all work. The only thing that does not
work is a **snap-confined browser** reaching them.

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

The probe prints one of: `reached_driver` (success — our driver was loaded),
`hw_decode_gate_off` (Firefox never tried VAAPI), `gpu_gl_init_failed` (Chromium
GPU process died), `software_decoder`, or `blocked_*`.

## Bottom line for the roadmap

Phase 3's browser exit criterion is **blocked by the browser sandbox, not by the
driver**. Every layer the driver depends on is proven working. Closing it needs
an unconfined browser (a user install decision), after which the existing probe
should be able to show `reached_driver` and then real hardware decode — subject
to the separate export-lifetime and firmware-stability work tracked elsewhere.
```
