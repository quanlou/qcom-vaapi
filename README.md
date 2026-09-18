# msm_drv_video — Rust VA-API driver for Qualcomm Iris (X1E80100)

A Rust VA-API/libva driver backed by the stateful V4L2 M2M `iris` decoder at
`/dev/video16`. libva derives the driver name `msm` from the DRM driver on
X1E80100, so the built module must be named `msm_drv_video.so`.

**Current scope:** H.264 decode, NV12, CPU-copy output through
`vaCreateImage`/`vaGetImage`/`vaDeriveImage`, plus an initial read-only DRM
PRIME export path for ready V4L2 CAPTURE buffers. Browser-grade zero-copy still
needs importer/lifetime validation in a browser.

## Documentation

New to Linux video drivers, VA-API, or this codebase? Start here — a
progressive "book" with PlantUML diagrams, written for beginners:

- [docs/README.md](docs/README.md) — index & learning path
- [PROGRESS.md](PROGRESS.md) — short handoff file for active work and verifier status
- [Background primer](docs/01-primer.md) — video, hardware, and Linux basics
- [The big picture](docs/02-stack.md) — the full stack and driver loading
- [VA-API crash course](docs/03-vaapi-tutorial.md) — object model & decode lifecycle
- [Tour of the code](docs/04-code-tour.md) — how this driver works, file by file
- [Upstream & downstream](docs/05-upstream-downstream.md) — ecosystem map
- [Roadmap, explained](docs/06-roadmap.md) — what's left and why
- [Write your own driver](docs/07-write-your-own-driver.md) — the reusable recipe

## Build

Build the Rust driver into an isolated libva driver directory:

```sh
./tools/build-rust-driver.sh /tmp/libva-v4l2-rust-driver
```

The helper compiles `rust/` in release mode and copies
`rust/target/release/libmsm_drv_video.so` to
`/tmp/libva-v4l2-rust-driver/msm_drv_video.so`, which is the filename libva
expects.

## Test without installing

```sh
LIBVA_DRIVERS_PATH=/tmp/libva-v4l2-rust-driver \
  vainfo --display drm --device /dev/dri/renderD128
```

Expected profiles:

- `VAProfileH264ConstrainedBaseline : VAEntrypointVLD`
- `VAProfileH264Main : VAEntrypointVLD`
- `VAProfileH264High : VAEntrypointVLD`

Run the full local verification script:

```sh
./tools/verify-rust-driver.sh /tmp/libva-v4l2-rust-driver
```

The script runs Rust unit tests, builds the isolated driver, checks `vainfo`,
compares a H.264 framemd5 matrix against native `h264_v4l2m2m`, and records DRM
PRIME/export probe logs under `/tmp/libva-v4l2-verify/`. The required matrix
covers one decoded frame, 30 decoded frames, and the full 300-frame 720p sample.
It also keeps stricter local probes visible: the one-frame EOS file is skipped
when native V4L2 produces no frame rows, and `bframes-240p.mp4` is reported as
an expected failure until the remaining H.264 synthesis edge is fixed. When
GStreamer VA and OpenGL plugins are available, the script also runs
`tools/verify-gst-export.sh`, which decodes one frame through
`vah264dec ! glupload` and verifies that `vaExportSurfaceHandle` is reached.
Set `V4L2_VA_GST_EXPORT_BUFFERS=N` to make that probe ask GStreamer for more
output buffers during manual stress testing.

Exercise same-decoder resolution changes separately:

```sh
./tools/verify-resolution-churn.sh /tmp/libva-v4l2-rust-driver
```

The probe concatenates 720x480 and 1280x720 H.264 clips twice before one
`vah264dec`, then checks that both negotiated formats and at least four driver
`SOURCE_CHANGE` events are observed. The main verifier runs it after the
required framemd5 and export probes when both sample clips are available.

Run the graphical browser probe separately when a display session is available:

```sh
V4L2_VA_BROWSER_SECONDS=12 \
  ./tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver

V4L2_VA_BROWSER=firefox V4L2_VA_BROWSER_SECONDS=12 \
  ./tools/verify-browser-vaapi.sh /tmp/libva-v4l2-rust-driver
```

This probe serves the sample from localhost, uses a fresh browser profile, and
looks for a driver call in the browser log. It is intentionally separate from
the headless verifier because browser GPU-process setup and sandbox packaging
are host-specific. Chromium also needs the legacy `__vaDriverInit_1_0` symbol;
the Rust driver exports it alongside the current libva init symbol. The result
also identifies software FFmpeg fallback separately from a browser driver call.

Manual 30-frame correctness check against native libavcodec V4L2 decode:

```sh
LIBVA_DRIVERS_PATH=/tmp/libva-v4l2-rust-driver \
ffmpeg -hide_banner -v warning \
  -hwaccel vaapi -hwaccel_device /dev/dri/renderD128 \
  -i test.mp4 -map 0:v:0 -frames:v 30 -f framemd5 rust-va.md5

ffmpeg -hide_banner -v warning \
  -c:v h264_v4l2m2m \
  -i test.mp4 -map 0:v:0 -frames:v 30 -f framemd5 native-v4l2.md5

cmp rust-va.md5 native-v4l2.md5
```

## Install system-wide

```sh
sudo cp /tmp/libva-v4l2-rust-driver/msm_drv_video.so /usr/lib/aarch64-linux-gnu/dri/
```

## Environment variables

- `V4L2_VA_DEBUG=1` — verbose Rust driver tracing.
- `V4L2_VA_DEVICE=/dev/video16` — override the V4L2 device node.
- `V4L2_VA_DUMP=/tmp/frame` — dump assembled Annex-B frames as
  `/tmp/frame_00.bin`, `/tmp/frame_01.bin`, ... for replay/debugging.
- `V4L2_VA_GST_EXPORT_BUFFERS=1` — number of buffers requested by
  `tools/verify-gst-export.sh`; increase manually for export/import stress.
- `V4L2_VA_GST_EXPORT_HOLD_MS=0` — delay each imported GStreamer buffer before
  release; combine with a larger `V4L2_VA_GST_EXPORT_BUFFERS` value to stress
  exported-buffer lifetime. Hold mode uses a bounded leaky side branch so
  downstream delay does not stop decoder input. The probe requests a graceful
  interrupt before force-killing a timed-out GStreamer pipeline.
- `V4L2_VA_RESOLUTION_LOW_SAMPLE=...` and
  `V4L2_VA_RESOLUTION_HIGH_SAMPLE=...` — override the two clips used by
  `tools/verify-resolution-churn.sh`.
- `V4L2_VA_RESOLUTION_DIR=...` — scratch directory for the resolution probe's
  GStreamer log and temporary pipeline files.
- `V4L2_VA_BROWSER=chromium` — browser executable selected by
  `tools/verify-browser-vaapi.sh`; `firefox` is also supported.
- `V4L2_VA_BROWSER_SECONDS=20` — timeout for the graphical browser probe.
- `V4L2_VA_BROWSER_CHROMIUM_MODE=auto` — use `in-process` to test Chromium's
  alternate GPU setup when the packaged browser disables its GPU process.
- `V4L2_VA_BROWSER_WORK_DIR=...` — browser-visible scratch directory override.
- `V4L2_VA_SAMPLE=...` — sample video used by the browser probe.

## Design notes

- VA-API hands the driver parsed H.264 picture parameters plus raw slice NAL
  bytes. The Rust driver synthesizes SPS/PPS from those fields and prepends
  them to the original slices, producing an Annex-B stream for the stateful
  V4L2 decoder.
- The V4L2 flow mirrors libavcodec's working `h264_v4l2m2m` path: configure
  OUTPUT as H.264, configure CAPTURE as NV12, queue compressed frames, start
  OUTPUT/CAPTURE, pump DQBUF, then bind CAPTURE buffers back to VA surfaces.
- CAPTURE buffers are returned to the decoder when libav reuses a VA surface,
  preventing CAPTURE pool starvation during threaded decode.
- The unsafe boundary is limited to libva/V4L2/mmap FFI. Driver-owned VA state,
  H.264 synthesis, buffer ownership, and surface bookkeeping live in Rust data
  structures.

## Verification status

Validated locally on the sample at `/home/mq/tmp/vaatest/test_720p.mp4`:

- Rust unit tests pass for H.264 SPS/PPS synthesis, DRM PRIME descriptor
  construction, surface publish behavior, export-state bookkeeping, and NV12
  image layout/copy helpers.
- `cargo clippy --all-targets -- -D warnings` passes for handwritten Rust; the
  generated libva bindings are excluded from project linting because their C ABI
  naming and bindgen transmute patterns are intentional.
- `vainfo` loads the Rust driver and reports H.264 Baseline/Main/High VLD.
- Rust VA decode matches native `h264_v4l2m2m` byte-for-byte for the 30-frame
  framemd5 test.
- The full 10-second sample now matches native `h264_v4l2m2m` for all 300 frames.
- `tools/verify-rust-driver.sh` passes the required matrix locally:
  `sample-1`, `sample-30`, and `sample-full`.
- `tools/verify-resolution-churn.sh` passes a same-decoder 720x480 to 1280x720
  GStreamer playback repeated twice, observing both negotiated formats and
  at least four `SOURCE_CHANGE` events.
- The `/home/mq/tmp/vaatest/one-frame.mp4` probe is optional: it is skipped when
  native V4L2 produces no frame rows and remains an expected failure when the
  Rust path cannot recover a usable frame. The stricter
  `/home/mq/tmp/vaatest/bframes-240p.mp4` probe is kept as `framemd5_xfail`:
  native V4L2 can produce frames, while the Rust VA path still hits decode
  errors in that edge case.
- mpv `--hwdec=vaapi-copy --frames=60` and GStreamer `vah264dec ! fakesink`
  complete with the isolated Rust driver.
- `tools/verify-session-churn.sh` passes locally: mpv mid-stream cuts,
  GStreamer follow-up playback, SIGKILL/SIGTERM teardown probes, and full
  framemd5 recovery decodes do not wedge the next session.
- Initial `vaExportSurfaceHandle` support fills `VADRMPRIMESurfaceDescriptor`
  for read-only DRM PRIME 2 NV12 export. `tools/verify-gst-export.sh` reaches
  the callback locally through `vah264dec ! glupload`. Export bookkeeping keeps
  driver-owned duplicate fds as Rust `OwnedFd`s and retires those fds before
  requeueing the CAPTURE buffer on surface reuse/destroy. The local ffmpeg
  `hwmap` probe is still blocked before it reaches the driver export callback
  because derived DRM device creation returns `Function not implemented` in this
  environment. The standalone C export verifier is also blocked until
  libva/libav development headers and unversioned `.so` links are installed.
  The former linear GStreamer hold stress exposed a pending OUTPUT/session
  stall after seven submissions; the current hold probe uses a bounded leaky
  tee so that importer retention is tested while decoder input continues.
- The standalone browser probe now initializes the Rust driver from Chromium's
  bundled libva after the driver added `__vaDriverInit_1_0`. On this machine the
  Chromium snap still launches its GPU process with GL disabled, so playback
  falls back before a VAAPI decode call; Chromium's in-process GPU experiment
  crashes in the snap. Firefox reaches the page but selects its software FFmpeg
  H.264 decoder. These are browser-launch/selection blockers, not passing
  zero-copy validation.
- The VAImage callbacks now live with the NV12 layout/copy helpers in
  `rust/src/image.rs`; `lib.rs` retains only driver initialization and shared
  state/status helpers.
- VA buffer allocation, mapping, metadata, and handle callbacks now live in
  `rust/src/buffer.rs`; the shared entrypoint module no longer owns those
  storage details.
- H.264 `vaBeginPicture`/`vaRenderPicture`/`vaEndPicture` callbacks now live in
  `rust/src/decode.rs`, leaving the entrypoint module focused on driver-wide
  lifecycle while `rust/src/vtable.rs` owns callback installation.
- Surface attribute negotiation, allocation, status, destruction, and CAPTURE
  retirement now live in `rust/src/surface.rs`, with focused tests for NV12 and
  VA/DRM-PRIME memory types.
- Profile/configuration negotiation and the precise empty display/subpicture
  capability responses now live in `rust/src/config.rs`; the callback groups
  are independent of the small driver entrypoint.
- Decode-context creation and teardown now live in `rust/src/context.rs`,
  leaving picture submission in `rust/src/decode.rs` and keeping context
  lifecycle out of the entrypoint.
- Surface synchronization and timeout diagnostics now live in
  `rust/src/sync.rs`; `rust/src/vtable.rs` connects those callbacks to libva.
- `vaExportSurfaceHandle` validation and descriptor publication now live beside
  the export bookkeeping in `rust/src/surface_export.rs`; the export module
  owns its lifetime and descriptor rules.
- Vtable installation now lives in `rust/src/vtable.rs`; all unsupported core
  and VPP callbacks use exact C signatures instead of an incompatible generic
  function-pointer stub. `lib.rs` is now 84 lines and retains only driver
  initialization, state lookup, and shared status helpers.

## Remaining work

- Keep end-of-stream drain covered by full-file framemd5 regression tests.
- Fix the 240p High-profile B-frame compatibility probe.
- Complete exported dmabuf lifetime/importer validation before relying on
  zero-copy in browsers.
- Test Chromium and Firefox with a working hardware-decode launch path, then
  implement the callbacks and surface-import behavior they require.
- Extend CAPTURE reconfiguration coverage to repeated changes, seeks, and long
  mixed-resolution playlists.
- Keep splitting the Rust driver into smaller modules around VA entrypoints,
  sync/publish logic, export handling, codec handling, V4L2 backend, and DRM
  interop.
- Add HEVC / VP9 / AV1 profiles only after H.264 is production-grade.
