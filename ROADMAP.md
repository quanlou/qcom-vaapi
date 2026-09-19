# Roadmap to production-grade VAAPI/browser support

The Rust driver is currently good enough for controlled FFmpeg VAAPI smoke tests, but it is not production-grade yet. The Rust-only tree can load through libva, report H.264 Baseline/Main/High VLD, and match native `h264_v4l2m2m` byte-for-byte on the current full 300-frame H.264 sample.

The main production blockers are browser-grade zero-copy validation, exported-buffer lifetime hardening, real client compatibility callbacks, CAPTURE reconfiguration, and module/API cleanup now that the C prototype has been purged.

## Current status

Validated locally on `/home/mq/tmp/vaatest/test_720p.mp4`:

- Rust H.264 SPS/PPS unit tests pass.
- `vainfo` loads the Rust driver successfully.
- The last clean baseline matched native `h264_v4l2m2m` byte-for-byte for the
  30-frame framemd5 test and all 300 frames of the full 10-second sample.
- The CPU-copy queue boundary is now explicit: ordinary FFmpeg/mpv sessions
  queue the CAPTURE pool and match completed buffers by timestamp, while
  pre-decode PRIME export clients reserve a fixed CAPTURE slot per surface.
  Host validation covers the current split with 90 Rust tests. The current
  Phase 2 source-change work now prevents false abort recovery on the empty
  CAPTURE/EOS marker pair, matches the sample's original SPS/VUI bytes, aligns
  startup order and timestamps with native, and can publish the first frame
  byte-exactly. Required `sample-1` still exits nonzero because the current
  STOP/START drain used to flush that frame breaks later reference continuity
  when FFmpeg decodes ahead, so repeated playback is not yet production-grade.
- The last clean baseline passed `tools/verify-rust-driver.sh`, including Rust
  unit tests, isolated-driver build, `vainfo`, and the required H.264 framemd5
  matrix: one decoded frame, 30 decoded frames, and the full 300-frame 720p
  sample. The current51 run is recorded above as blocked at sample-1 by the
  poisoned device.
- Optional edge probes now run after the required 720p matrix so native/firmware failures in `one-frame-eos` or `bframes-240p` cannot poison the baseline verification run.
- Native reference generation in the verifier has a bounded retry for transient native `h264_v4l2m2m` POLLERR/abort storms; the Rust decode matrix remains mandatory.
- `/home/mq/tmp/vaatest/one-frame.mp4` is skipped in the verifier when native `h264_v4l2m2m` produces no frame rows; it is useful as an environment probe, not a required target.
- `/home/mq/tmp/vaatest/bframes-240p.mp4` is now tracked as an explicit `framemd5_xfail`: native V4L2 emits frames, but the Rust VA path still stalls after source-change and returns decode errors. This is a real H.264 synthesis/firmware compatibility gap, not a surface publication issue.
- Follow-up diagnostics on that xfail: Rust-assembled Annex-B frames are software-decodable, and the first displayed frames match the original stream. Simply raising the OUTPUT in-flight limit past two frames lets three packets queue but does not produce CAPTURE output and regresses GStreamer; switching V4L2 timestamps from POC-derived values to tiny monotonic sequence numbers also regresses mpv/GStreamer. Keep the stable client behavior while debugging the remaining H.264/V4L2 compatibility detail.
- 2026-09-18 deep diagnosis of that xfail: the failure is a per-session probabilistic FIRMWARE session abort correlated with small picture height, not a bitstream synthesis problem. Under `V4L2_VA_DEBUG=1` the signature is 1-2 OUTPUT QBUFs, same-dims `SOURCE_CHANGE`, one EMPTY CAPTURE (bytes=0, ts=0.0), a spontaneous `V4L2_EVENT_EOS` without any `DECODER_CMD STOP`, then no OUTPUT DQBUF ever. Ruled out: synthesized SPS/PPS content (x264 re-encodes are field-identical across sizes; a 1280x720 x264 re-encode passes while 640x480 fails), B-frames (B-free Constrained-Baseline 320x240 fails too), the small-picture level pick (640x480 and 1280x480 both synthesize level 4.2 and still fail), `V4L2_DEC_CMD_START` on same-dims SOURCE_CHANGE (A/B tested), and first-submit timing (30 ms delay tested). Failure probability scales inversely with picture height (<=512 px fails most sessions, 544-576 flaky, >=640 solid; the 720p matrix never failed), and the same session can pass on retry. Native `h264_v4l2m2m` decodes the failing clips reliably on the same node without ever sending DECODER_CMD. A fix requires kernel-side evidence: privileged `dmesg`/venus HFI traces around a failing small-stream session.
- Initial read-only `vaExportSurfaceHandle` is implemented with `VIDIOC_EXPBUF` and a local ABI-compatible `VADRMPRIMESurfaceDescriptor` shim because this machine's installed libva headers do not expose `va_drmcommon.h`.
- `tools/verify-gst-export.sh` now proves a runtime importer/export path reaches `vaExportSurfaceHandle`: `vah264dec ! glupload ! fakesink` negotiates through GStreamer and exits 0 after the callback is hit. `V4L2_VA_GST_EXPORT_BUFFERS=N` can request multiple imported buffers, and `V4L2_VA_GST_EXPORT_HOLD_MS=N` delays their release to exercise lifetime pressure; the main verifier intentionally keeps the stable one-buffer default. The local ffmpeg `hwmap=derive_device=drm` probe still fails before calling `vaExportSurfaceHandle`, so browser/importer validation remains open beyond this first GStreamer path.
- PRIME export retention is bounded at 64 tracked duplicates per live surface; further exports return `VA_STATUS_ERROR_MAX_NUM_EXCEEDED` until the surface is retired. This prevents a client from growing driver-owned fd state without bound when the VA API gives the driver no client-close callback.
- Exported CAPTURE-buffer lifetime is tracked per surface: exports mark the surface, keep driver-owned duplicate fds as Rust `OwnedFd`s for automatic cleanup, and `destroy_surfaces` / surface reuse (`vaBeginPicture`) retire export fds before requeueing the owning CAPTURE buffer. If the driver cannot duplicate the exported dma-buf fd, export now fails instead of handing out an untracked descriptor.
- GStreamer `vah264dec` now completes the full 720p sample (`gst-launch ... ! vah264dec ! fakesink` exits 0). This required publishing finished CAPTURE buffers into VA surface state at `vaEndPicture`, `vaQuerySurfaceStatus`, and inside the sync loop; previously they only became visible to clients in `vaSyncSurface`, so GStreamer's pipelining exhausted the 32-buffer CAPTURE pool and wedged the decoder. Export also accepts read-capable flag combinations (`vaExportSurfaceHandle(flags=READ_WRITE|SEPARATE_LAYERS)` as requested by GStreamer).
- `tools/verify-resolution-churn.sh` now verifies all 780 CPU-copy frames across a 960x640 to 1280x720 to 960x640 to 1280x720 playlist, four `SOURCE_CHANGE` events, zero firmware faults, and a healthy follow-up decode. Seeks over the same stable resolution pair are covered by `tools/verify-seek-storm.sh`; mixed seeks finish without a wedge, although Iris emits recoverable per-session aborts under that artificial storm. `tools/verify-long-playback.sh` separately verifies 3,600 frames over 12 same-resolution segments.
- The last clean baseline had mpv `--hwdec=vaapi-copy` completing 60 frames
  with the Rust driver, and GStreamer `vah264dec ! fakesink` completing the
  full 720p sample with `GST_VA_ALL_DRIVERS=1 GST_VAAPI_ALL_DRIVERS=1`.
  Repeated playback must be re-established after `/dev/video16` firmware
  recovery against the current CPU-copy queue split.
- Session bring-up and teardown are hardened after diagnosing an intermittent cross-session wedge: `try_start` re-applies NV12 and reinitializes the CAPTURE queue (STREAMOFF/REQBUFS(0)/realloc) between bounded STREAMON retries; sessions flush pending OUTPUT/decode work (DECODER_CMD STOP drain, bounded to 500 ms) before teardown; CAPTURE is streamed off before OUTPUT. Without the flush, a session closed with frames in flight (mpv cut, killed process, ffmpeg probe abort) made the NEXT session's CAPTURE STREAMON fail with EIO until the hardware recovered on its own.
- The V4L2 session now detects firmware session aborts with pending work and recovers transparently. The abort has two firmware variants: a spontaneous `V4L2_EVENT_EOS` without a drain, or a silent empty CAPTURE dequeue (bytes=0) with work pending and no drain in progress. Detection arms on either. The session then snapshots replayable OUTPUT chunks in submission order, rebuilds the device session on a fresh fd, preserves old CAPTURE mappings for already-published surfaces as read-only legacy pools, and replays the pending chunks with the last SPS/PPS prepended. Recovery is capped at one rebuild (`MAX_SESSION_RECOVERIES=1`) because repeated aborted-session teardowns transiently poison the firmware. The current51 matrix and churn run are blocked by that poison; `bframes-240p` remains an expected firmware-related xfail.
- `tools/verify-session-churn.sh` covers repeated open/close recovery: mpv
  mid-stream cuts followed by GStreamer playback, SIGKILL/SIGTERM teardown
  probes, and parity-checked full decodes after each poison leg. The latest
  run could not enter those legs because its reference decode produced no
  output.
- `lib.rs` is now a small driver entrypoint. `state.rs` owns handle tables and object state, `config.rs` owns profile/configuration capability negotiation, `context.rs` owns decode-context lifecycle, `va_drm.rs` owns DRM PRIME descriptor construction, `image.rs` owns NV12 VAImage layout plus CPU-copy helpers, `sync.rs` owns CAPTURE publication and surface synchronization diagnostics, `surface_export.rs` owns export-state/fd bookkeeping, export validation, and descriptor publication with unit coverage, `surface/status.rs` owns surface status/error callbacks, `buffer.rs` owns allocation/map lifecycle, `buffer/handles.rs` owns buffer metadata/external-handle/sync callbacks, `vtable.rs` owns ABI callback installation, and `vtable/unsupported.rs` owns exact-signature unsupported callbacks.
- V4L2 session diagnostics are isolated in `rust/src/v4l2/debug.rs`; the main
  session module is now 343 lines after the queue/capture/setup/recovery/
  teardown splits, with the same queue-state formatting coverage.
- Raw V4L2 ioctl and libc boundary definitions are isolated in
  `rust/src/v4l2/abi.rs`; the kernel-facing declarations have one focused
  module to audit.
- OUTPUT pacing, QBUF construction, replay-compatible submission, and explicit
  drain initiation are isolated in `rust/src/v4l2/submit.rs`.
- Readiness polling, DQBUF/event handling, CAPTURE lookup, and export lookup are
  isolated in `rust/src/v4l2/poll.rs`.
- CPU-copy queue-all mode and pre-decode PRIME CAPTURE-slot reservation are
  isolated in `rust/src/v4l2/capture.rs`; the parent session module is 343
  lines and the queue ownership boundaries are explicit.
- `tools/verify-browser-vaapi.sh` now provides a standalone graphical browser
  probe with a fresh profile and a localhost sample. Chromium's older libva
  loader ABI is supported through `__vaDriverInit_1_0`; the installed Chromium
  snap still disables its GPU process before VAAPI decode (the in-process
  variant crashes), while Firefox reaches the page but selects software FFmpeg
  H.264 decoding.
- The VAImage callback group has been moved into `rust/src/image.rs`, leaving
  `lib.rs` focused on shared entrypoint wiring and decode lifecycle while the
  image module owns its handle validation and CPU-copy behavior. Pure NV12
  layout/copy tests now live beside `rust/src/image/layout.rs`.
- Strict Clippy now passes for handwritten Rust after the module split; generated
  libva bindings are explicitly excluded from linting because they preserve the
  C ABI surface.
- VA buffer allocation and mapping callbacks live in `rust/src/buffer.rs`;
  metadata, external-handle, and sync callbacks live in
  `rust/src/buffer/handles.rs`, with direct unsupported-memory and validation
  regressions. The remaining large group in `lib.rs` is the H.264
  render/decode lifecycle.
- The H.264 picture lifecycle is now in `rust/src/decode.rs`; `lib.rs` is down
  to driver-wide lifecycle, vtable setup, surface management, and shared sync.
- Surface allocation, destruction, and CAPTURE retirement are now in
  `rust/src/surface.rs`; attribute negotiation lives in
  `rust/src/surface/attributes.rs`, and status/error callbacks live in
  `rust/src/surface/status.rs`.
- H.264 bit-level writing, Exp-Golomb coding, RBSP-to-EBSP escaping, and NAL
  wrapping now live in `rust/src/h264/bitstream.rs`; `h264.rs` remains focused
  on SPS/PPS/VUI synthesis policy and frame assembly. The 720p sample's
  synthesized SPS now byte-matches the original SPS; VA still does not provide
  the original user-data SEI NAL.
- `vaGetImage` now validates the requested rectangle with checked arithmetic
  against both the decoded surface and destination VAImage dimensions before
  copying pixels.
- Vtable installation is now isolated in `rust/src/vtable.rs`; the 19
  unsupported core callbacks and three VPP callbacks live in
  `rust/src/vtable/unsupported.rs`, use exact C signatures, and return
  `VA_STATUS_ERROR_UNIMPLEMENTED`. The previous incompatible function-pointer
  transmute path is gone.
- V4L2 queue state is isolated in `rust/src/v4l2/queue.rs`, leaving session
  setup, polling, recovery, and teardown in `v4l2.rs` while keeping buffer
  ownership types together.
- V4L2 setup is isolated in `rust/src/v4l2/setup.rs`, covering capability and
  format negotiation, queue allocation, CAPTURE STREAMON retries, and pending
  OUTPUT snapshots. The parent module now concentrates on session runtime,
  recovery, and teardown.
- Bounded firmware-session rebuild and OUTPUT replay are isolated in
  `rust/src/v4l2/recovery.rs`; the parent module now concentrates on runtime
  polling and deterministic teardown.
- Deterministic streamoff, queue release, legacy-pool cleanup, and `Drop` are
  isolated in `rust/src/v4l2/teardown.rs`; the remaining parent module is
  focused on active session polling and buffer submission.

## Missing pieces

### End-of-stream drain

The driver now resolves the current full H.264 sample to 300/300 frames and matches native `h264_v4l2m2m`. The drain logic is still young and must stay under regression coverage.

Needed work:

- Keep full-file framemd5 parity in `tools/verify-rust-driver.sh`.
- Keep the short/long matrix in `tools/verify-rust-driver.sh`.
- Fix the strict 240p High-profile B-frame probe now recorded as `framemd5_xfail`. Userspace triggers are ruled out (see the 2026-09-18 diagnosis in Current status). The firmware error CLASS is now observable unprivileged via `journalctl -k` / `tools/capture-iris-kernel-log.sh` (see `docs/08-iris-firmware-errors.md`): it is a `qcom-iris` `session error 0x4000003: fatal error`, reproduced on native decode. HFI-level detail (the provoking command) is still root-gated (dynamic_debug/debugfs). Until a root capture is possible, treat small-stream decode failures as retryable in clients.
- Produce clearer debug state when sync times out.
- Exercise drain after seek/flush once those callbacks exist.

Production requirement:

```sh
LIBVA_DRIVERS_PATH=/tmp/libva-v4l2-rust-driver \
ffmpeg -hide_banner -v warning \
  -hwaccel vaapi -hwaccel_device /dev/dri/renderD128 \
  -i test.mp4 -map 0:v:0 -f framemd5 rust.md5

ffmpeg -hide_banner -v warning \
  -c:v h264_v4l2m2m \
  -i test.mp4 -map 0:v:0 -f framemd5 native.md5

cmp rust.md5 native.md5
```

For the current sample, this means preserving the verified 300/300-frame result.

### Browser-grade zero-copy

The driver now has an initial read-only DRM PRIME 2 export path: `vaExportSurfaceHandle` calls `VIDIOC_EXPBUF` for a ready NV12 CAPTURE buffer and fills composed or separate-layer descriptors. That is the first zero-copy building block, but it is not browser-grade yet.

Needed work:

- Keep `tools/verify-gst-export.sh` green: it proves GStreamer can reach `vaExportSurfaceHandle` through `vah264dec ! glupload`.
- Broaden importer coverage: the local ffmpeg `hwmap` command still fails during derived DRM device creation before calling the driver.
- Build/run `tools/verify-export-prime.sh` once the machine has `libva-dev libavcodec-dev libavformat-dev libavutil-dev`; it currently exits 77 because those headers and unversioned development `.so` links are missing.
- Preserve CAPTURE buffer lifetime while exported handles are held. Per-surface
  `OwnedFd` tracking and cleanup exist; the remaining work is to validate the
  client/importer reference lifetime and make requeue behavior match it.
- Requeue CAPTURE buffers only after exported handles are safe to retire, with a
  multi-buffer importer stress test that holds handles across surface reuse.
- The original linear 16-buffer hold reached export and stalled after the
  decoder had submitted seven frames, because downstream delay prevented the
  client from submitting the future frames needed to finish reorder output.
  The diagnostic now uses a bounded leaky tee branch to hold imported buffers
  while decoder input continues; rerun this version before attributing a stall
  to CAPTURE ownership.
- Verify NV12 planes, offsets, strides, and modifiers with an importer.
  DONE for planes/offsets/strides: `tools/verify-gl-roundtrip.sh` +
  `tools/gst_gl_roundtrip.py` decode the 720p sample through
  `vah264dec ! glupload ! gldownload ! videoconvert ! I420` and compare
  stride-aware per-frame hashes against an ffmpeg vaapi-copy rawvideo
  reference. First hardware run: all 28 frames that completed the GL path
  were byte-identical (gl_stride=ref_stride=1280), so the exported
  descriptor's plane offsets/strides/sizes are correct. Modifiers remain
  unexercised (the export path uses linear DRM PRIME without modifiers), and
  the full 300-frame GL run still aborts at ~frame 29 — see the
  export-lifetime signature in PROGRESS.md blockers.
- Test Chromium/Firefox GPU-process import behavior.
- Environment note: this machine's GStreamer VA plugin filters custom drivers ("Unsupported driver"); `GST_VA_ALL_DRIVERS=1` is required to expose `vah264dec` for this driver.

Production requirement:

- Browser VAAPI logs show hardware decode selected.
- Playback does not fall back to software decode.
- 1080p and 4K samples play without driver-caused frame drops.

### Image and buffer compatibility

The current image copy path is enough for the 30-frame FFmpeg test, but the libva ecosystem may call other paths.

Needed work:

- Keep `vaDeriveImage` covered; it is currently copy-backed, not zero-copy.
  Direct callback coverage now includes rejecting a null output pointer before
  sync or surface-state access.
- Finish image lifecycle cleanup.
- Mapped VA buffers now cannot be resized or destroyed until unmapped, and
  image destruction rejects a mapped backing buffer.
- Repeated `vaMapBuffer` calls now preserve the existing allocation and return
  the same pointer until `vaUnmapBuffer`; an API-level regression test covers
  the map/resize/destroy/unmap lifetime sequence.
- Image backing buffers are now protected from generic resize and destroy
  callbacks until their owning `vaDestroyImage` call retires the image.
- `vaGetImage` now rejects writes while the destination image buffer is mapped,
  avoiding a client-visible data race on the CPU-copy path.
- `vaBeginPicture` validates nested-picture state before retiring the target's
  CAPTURE slot or tracked export state, making rejected begins side-effect
  free.
- `vaRenderPicture` bounds the client-supplied buffer list and uses checked
  slice range arithmetic at the FFI boundary.
- Configurations with live decode contexts can no longer be destroyed, and
  context teardown detaches owned surfaces before dropping their V4L2 mappings;
  detached surfaces become explicitly dead and safe to destroy or reuse.
- VA context creation now rejects dimensions outside the driver's supported
  range before opening a V4L2 session.
- `vaCreateConfig` now rejects unsupported render formats and decode modes
  before allocating a configuration, keeping advertised capabilities aligned
  with the actual H.264 VLD path.
- `vaCreateContext` now rejects malformed render-target arguments before
  opening a V4L2 session, preventing null-list dereferences and avoiding
  partial setup for invalid callers.
- `vaCreateSurfaces2` now rejects malformed attribute lists and unsupported
  settable formats or memory types before allocating surface IDs. `vaBeginPicture`
  validates the context before retiring a prior CAPTURE assignment, preserving
  surface state on invalid-handle failures.
- Contexts now validate and retain their nonempty render-target lists, and
  `vaBeginPicture` rejects a surface outside that list. An empty list remains
  an unrestricted compatibility path for clients that do not provide targets
  at context creation time.
- `vaDestroyImage` now validates the image-to-buffer ownership link before
  removing either object and preserves both objects while the backing buffer is
  mapped; the lifecycle path has focused regression coverage.
- `vaDestroySurfaces` now validates the complete input list before mutation,
  handles duplicate IDs deterministically, and rejects a surface attached to
  an open picture, avoiding partial teardown and stale render targets.
- `vaDestroyContext` now rejects teardown while `vaBeginPicture` is open,
  preserving the active V4L2 session and surface mappings until the frame is
  closed.
- Decode buffers now retain their owning context, reject cross-context render
  use, and are reclaimed with context teardown when unmapped; mapped buffers
  keep teardown rejected until the client releases them.
- `vaGetImage` now waits for a pending source surface through the normal VA
  sync path before copying, reducing dependence on clients issuing a separate
  `vaSyncSurface` call first.
- Pure NV12 layout and bounded copy math now live in `rust/src/image/layout.rs`,
  keeping image FFI callbacks focused on handle validation and ownership.
- Surface attribute negotiation now lives in `rust/src/surface/attributes.rs`,
  reducing the lifecycle callback module while keeping allocation and CAPTURE
  ownership in one place.
- Buffer ownership now has API-level coverage from creation through context
  association, complementing the teardown and mapped-buffer tests.
- Keep `vaAcquireBufferHandle` / `vaReleaseBufferHandle` returning precise unsupported-memory behavior until a real external buffer path exists; `rust/src/buffer/handles.rs` now has direct callback coverage for this contract.
- `vaPutImage`, `vaPutSurface`, `vaLockSurface`, and `vaUnlockSurface` are
  cleanly rejected through type-correct callbacks with focused tests. Keep
  them unsupported unless a real client requires CPU surface locking or
  display rendering.
- `vaCreateSurfaces2` now has direct callback coverage for malformed
  pointer/count pairs, and the GL round-trip helper has a synthetic padded
  stride/plane comparison proving its parser detects visible I420 pixels
  independently of buffer padding.
- NV12 copy bounds now use checked offset arithmetic for kernel/client stride
  values, with an overflow regression test at the pure layout boundary.
- H.264 header assembly now rejects overflowed bit-writer inputs and uses
  checked capacity/clamping arithmetic, with a regression test for malformed
  extreme values in `rust/src/h264/bitstream.rs`.
- Client buffer allocation and resize now enforce a 64 MiB ceiling, returning
  a VA error before a large allocation can abort the driver process.
- Aggregate H.264 frame assembly enforces the same ceiling before allocating
  the OUTPUT packet, covering the case of many valid slice buffers in one
  picture.
- Expand `vaQuerySurfaceError` beyond the current ready/dead distinction when V4L2 exposes richer decode errors.
- Pending surfaces now fail promptly when the owning V4L2 session has latched
  an unrecoverable error; `vaSyncSurface`, `vaQuerySurfaceStatus`, and
  `vaQuerySurfaceError` no longer wait for a generic timeout in that state.

Production requirement:

- FFmpeg VAAPI copy path passes the required hardware matrix with byte-exact
  native parity.
- mpv `--hwdec=vaapi-copy` passes repeated playback and churn recovery.
- Repeated image creation/destruction does not leak capture buffers, fds, or mmap regions.

### CAPTURE reconfiguration

The real-dimension path now passes a CPU-copy FFmpeg mixed-resolution probe
through four 960x640/1280x720 transitions: 780/780 decoded frames, four
observed `SOURCE_CHANGE` events, and zero Iris session/system faults.

Known environment issue (external): under heavy session churn the hardware occasionally refuses a session at CAPTURE bring-up (STREAMON EIO, no SOURCE_CHANGE, no decoded frames) and the native `h264_v4l2m2m` decoder hits the same class of POLLERR storms. Userspace mitigations are in place (NV12 re-apply retry, CAPTURE queue reinit between STREAMON retries, teardown flush); kernel `dmesg` access is needed to diagnose the firmware side further.

Needed work:

- Repeat CAPTURE cycling across several resolution changes without leaking or
  reusing stale surfaces. DONE for the four-transition CPU-copy gate.
- Invalidate or mark affected surfaces correctly. DONE for published CPU
  snapshots and session-fatal surfaces.
- Reallocate buffers with the new format. DONE through the bounded rebuild path.
- Force SPS/PPS re-emit after reconfiguration. DONE in the H.264 aggregate
  frame path.
- Resume the Iris session reliably. DONE for the covered CPU-copy gates; longer
  mixed playlists are limited by native Iris firmware behavior.
- Cover seeking and drain across a resolution boundary. DONE as behavior
  validation: `tools/verify-seek-storm.sh` mixed phase drives 12 real mpv IPC
  seeks across a generated 960x640+1280x720 mpegts concat -- no wedge, no
  deadlock, mpv exits cleanly; one run recorded 15 recoverable per-session
  0x4000003 aborts, all rescued by the capped recovery, with zero system-fatal
  firmware faults.

Production requirement:

- Mixed-resolution streams decode without deadlock.
- Seeking across resolution changes works.

### Flush, seek, and recovery

Browsers and media players stress decoder lifecycle behavior much harder than a short FFmpeg command.

Needed work:

- Flush on seek. NOTE: VA-API has no flush callback; seek/flush reaches the
  driver as sync/drop/teardown/submit cycles, which the seek-storm probe now
  exercises end to end (drain state resets on new submissions in submit.rs).
- Drain after flush. Same: the storm's stop-drain + resubmit path is
  exercised on every seek; post-storm framemd5 stays byte-identical.
- Destroy pending surfaces safely.
- Recover from anomalous decode errors without wedging the device; the bounded
  abort-recovery path (spontaneous EOS or silent empty CAPTURE) is now
  implemented, but broader seek and resolution-change recovery is still open.
  Never loop session opens/rebuilds on aborts: repeated aborted teardowns
  poison the firmware transiently (~90 s), which is why recovery is capped at
  one rebuild and clients must treat small-stream decode failures as
  retryable-at-the-client instead.
- Streamoff/release all V4L2 resources deterministically.
- Add leak checks for fds and mmap regions.

Production requirement:

- Repeated playback open/close cycles work.
- Seek storm tests do not deadlock. MET: `tools/verify-seek-storm.sh` (24
  seeks on 720p, 12 on a mixed-resolution concat, mpv JSON IPC,
  `--hwdec=vaapi-copy`) completes with mpv exiting cleanly, zero
  system-fatal aborts, zero power-cycles, and a byte-identical post-storm
  sanity decode; per-session 0x4000003 aborts on mixed-res seeks are rescued
  without wedging the node.
- The driver returns useful errors instead of hanging.

### Context and client behavior

The current driver permits one active decode context at a time. This matches
the single-session hardware path that has been validated and gives a second
client a deterministic VA error before device setup.

Needed work:

- Decide whether production hardware needs a multi-session implementation; the
  current single-session rejection must remain explicit until that path is
  tested.
- Keep the typed unsupported callbacks explicit; add a real implementation only
  when a client requires it and its lifetime contract is tested.
- Test against FFmpeg, mpv, GStreamer, Chromium, and Firefox.
- Add targeted compatibility callbacks based on what real clients call.

Production requirement:

- Browser process behavior does not trigger unexpected `VA_STATUS_ERROR_UNIMPLEMENTED` on required paths.

### Rust structure and idioms

The Rust rewrite removed the old C implementation. `lib.rs` now contains only
shared status helpers, driver state lookup, and the two libva initialization
symbols; callback installation lives in `vtable.rs` and feature groups live in
their own modules.

Needed work:

- Move VA object tables and ID allocation into a state module.
- Keep profile/configuration negotiation in its own capability module.
- Move image creation/copy/derive paths into an image module.
- Keep DRM PRIME ABI definitions and descriptor construction in `va_drm`.
- Keep raw FFI pointer handling at the libva boundary and pass typed Rust values internally.
- Keep every installed unsupported callback type-correct at the FFI boundary;
  do not use a one-signature function pointer as a generic stub. The stubs now
  live in `rust/src/vtable/unsupported.rs`.
- Add comments for non-obvious VA/V4L2 lifetime rules, especially CAPTURE ownership and exported fd ownership.
- Add tests around descriptor construction, ID validation, and H.264 bitstream assembly.

Production requirement:

- New callbacks can be implemented without growing one monolithic unsafe module.
- Unsafe code remains localized and justified by a clear FFI boundary.

### Codec expansion

H.264 Baseline/Main/High, HEVC Main, and VP9 Profile 0 are advertised only when
the live V4L2 node enumerates the matching coded formats. Decode now routes
through codec-specific VA buffer parsing and access-unit assembly before a
codec-neutral V4L2 submit path sets the coded `S_FMT`.

HEVC Main synthesizes VPS/SPS/PPS from VA long-format picture parameters,
preserves the original slice payloads, and matches native `hevc_v4l2m2m` for
30 frames. Unsupported stream shapes are rejected when required SPS-resident
syntax is not available from VA buffers. VP9 Profile 0 forwards complete frame
payloads and matches native `vp9_v4l2m2m` for 30 frames.

Main10 remains hidden until P010 render targets exist. AV1 remains hidden:
VA provides tile payloads, but Iris needs temporal delimiter, sequence, and
frame OBU headers. The conformance sample has a 41-byte header prefix before
the first tile, which identifies the remaining synthesis work.

## Phased plan

### Phase 1: finish H.264 correctness

- Keep EOS/drain covered by regression tests.
- Preserve full-sample native-output parity.
- Add regression commands for 1-frame, 30-frame, and full-file framemd5.

Exit criteria:

- Full H.264 sample matches native `h264_v4l2m2m` with `cmp`.

### Phase 2: complete CPU-copy client compatibility

- The implementation boundary is complete and host-tested: CPU-copy clients
  retain the legacy queue-all CAPTURE behavior, while pre-decode PRIME export
  clients use stable surface-to-buffer bindings. The code boundary now lives
  in `rust/src/v4l2/capture.rs`. Buffer metadata/handle behavior is host-tested
  in `rust/src/buffer/handles.rs`. The exit criterion is now MET: a clean device
  run proves repeated FFmpeg and mpv playback (see below).
- Keep `vaDeriveImage` covered; it is currently copy-backed, not zero-copy.
  Null-output callback coverage is in place.
- Finish image lifecycle cleanup.
- Keep `QuerySurfaceError` and surface status behavior covered; direct callback
  regressions now prove Ready vs Rendering status and Dead-surface decode-error
  reporting.
- Test FFmpeg and mpv `vaapi-copy`. DONE: the required 720p matrix
  (`sample-1`/`sample-30`/`sample-full`) is byte-exact vs native
  `h264_v4l2m2m`, and `verify-session-churn.sh` is `pass=7 fail=0` (mpv-cut,
  GStreamer, and SIGKILL/SIGTERM recovery legs all pass). The fix was to
  synthesize the SPS VUI with `max_num_reorder_frames=0` (decode-order output)
  so iris stops withholding frames until a drain; this removed the
  reorder-delay deadlock that blocked FFmpeg's VAAPI-copy sync path.

Exit criteria:

- FFmpeg and mpv copy paths are reliable over repeated playback. **MET**
  (2026-09-19).

### Phase 3: harden dmabuf zero-copy

- Keep `VIDIOC_EXPBUF` / `vaExportSurfaceHandle` working.
- Add an importer/export verifier that reaches the driver callback.
- Manage exported-handle lifetime and CAPTURE requeueing safely.
- Validate GPU import path.

Exit criteria:

- Browser can use the driver without CPU-copy fallback. **MET**
  (2026-09-20). Snap Chromium's `native` mode selects `VaapiVideoDecoder`,
  exports its 22-frame pool through this driver, and plays the sample
  end-to-end (2565 `BeginPicture` / `EndPicture` frames, 2561 zero-copy
  publishes, 0 sync/export errors, 0 internal decoding errors, 90+ s of
  looped playback). Landing fixes: `vaSyncSurface` on an Empty surface now
  returns SUCCESS instead of DECODING_ERROR (matches Mesa/Intel; Chromium
  syncs pool surfaces before their first decode as a validity check); the
  CAPTURE pool grew from 20 to 32 slots and `queue_working_capture` now
  caps queued working slots at `WORKING_QUEUE_MAX=6` so Chromium's
  interleaved export/decode loop cannot exhaust unreserved slots.

### Phase 4: harden lifecycle and reconfiguration

- Flush/seek/recovery. **MET** for CPU-copy playback; mixed-resolution seek
  storms recover from Iris per-session aborts without wedging the node.
- Real resolution changes. **MET** for four 960x640/1280x720 transitions and
  780/780 decoded frames with zero firmware faults.
- Long playback and repeated open/close tests. **MET**: 3,600/3,600 frames over
  12 segments and `verify-session-churn.sh` pass=7 fail=0.
- Better diagnostics. **MET**: lifecycle probes capture bounded logs and Iris
  session/system fault counts.

Exit criteria:

- Long playlists, seek storms, and mixed-resolution content do not wedge the
  driver. **MET** (2026-09-20). Recoverable Iris session aborts remain visible
  in the mixed seek-storm result.

### Phase 5: broaden codec support

- HEVC Main: VA picture/slice parsing, VPS/SPS/PPS synthesis, coded-format
  setup, and 30-frame native V4L2 parity are implemented. Stream shapes whose
  required SPS syntax is absent from VA long-format parameters are rejected.
- VP9 Profile 0: complete-frame forwarding, coded-format setup, and 30-frame
  native V4L2 parity are implemented.
- HEVC Main10 remains hidden until P010 render targets exist. AV1 remains
  hidden: VA supplies tile payloads, while Iris needs the omitted temporal,
  sequence, and frame OBU headers. The conformance sample has a 41-byte header
  prefix before the first tile, establishing the remaining synthesis work.

## Immediate next tasks

1. `bframes-240p` `framemd5_xfail`: the bounded session-recovery path is now covered, but the probe still fails often enough to remain an expected failure. Userspace triggers were exonerated. Firmware-side evidence is now captured unprivileged with `tools/capture-iris-kernel-log.sh` (see `docs/08-iris-firmware-errors.md`): the abort is a `qcom-iris` session-fatal `0x4000003`; when it escalates to a device-wide `0x5000003` the node power-cycles (~90s poison) and even native decode fails. Remaining step needs root: enable `qcom_iris` dynamic_debug and diff the HFI sequence of a failing small session vs a passing 720p session. The required 720p matrix in the verifier stays as is.
2. Keep the runtime GStreamer export and resolution verifiers (`tools/verify-gst-export.sh` and `tools/verify-resolution-churn.sh`) in the main verification loop. The standalone C export verifier (`tools/verify-export-prime.sh`) still exits 77 with a package hint until `libva-dev libavcodec-dev libavformat-dev libavutil-dev` headers are installed.
3. Finish the same-dimension `SOURCE_CHANGE` resume path for short FFmpeg copy decodes: current driver handles the empty CAPTURE/EOS marker pair without false recovery, queues/drains OUTPUT, and can publish the first frame with a native-matching MD5, but the STOP/START drain workaround loses later H.264 reference continuity and FFmpeg still exits nonzero after decoding ahead. Replace the midstream STOP workaround with a frame-preserving resume: mirror native's OUTPUT-first/CAPTURE-later sequence without ending the decode stream, or make rebuild/replay preserve enough reference state to continue after the first published surfaces.
   [2026-09-18 diagnosis (offline strace analysis, PROGRESS.md "SOURCE_CHANGE resume DIAGNOSIS RESULT"): native receives the same-dims SOURCE_CHANGE EVERY session and its only handling is G_FMT + one EBUSY-ignored DECODER_CMD + keep pumping — NO successful STOP, NO START, NO queue cycle, and FLAG_LAST/EOS never appear mid-stream. The marker pair on our side means iris self-drained into the V4L2 Stopped state; per spec only V4L2_DEC_CMD_START resumes it, but the current code can never send one there (the START helper is gated on `eos||draining`, and the suppressed paired-EOS never sets `eos`). Recipe: never STOP for same-dims source change; START immediately at marker-pair completion; pump CAPTURE continuously (no deferral); keep false-abort suppression. If references still break, audit whether SPS/PPS are re-prepended onto every AU (iris may re-parse + auto-drain mid-stream).]
   [2026-09-19 DONE (claude agent): the required H.264 matrix is byte-exact vs
   native on hardware — sample-1/30/full rc=0 with `cmp`-equal framemd5s, churn
   7/7, resolution-churn pass (6 SOURCE_CHANGEs). The landing fix differs from
   the 2026-09-18 recipe: the same-dims marker path already worked; the actual
   short-decode blocker was a vaSyncSurface input-starvation deadlock (iris
   defers frame release until the next AU, which a syncing client never sends),
   broken with a bounded STOP drain whose resume replays SPS/PPS + keyframe
   history (iris drops its reference chain across STOP), plus dequeue-time
   pixel snapshots to fix CAPTURE-slot aliasing on late vaGetImage/
   vaDeriveImage reads. Evidence in PROGRESS.md Completed recently; the
   remaining gst-gl failure is the separate Phase 3 export wedge.]
4. Reference-count exported fds so CAPTURE requeue waits until the last exported handle is retired; add GStreamer dmabuf import as the first real lifetime test.
5. mpv `--hwdec=vaapi-copy` and GStreamer `vah264dec` pass for the three mpv-cut churn legs after the Phase 2 source-change fix. Still open: forced-kill/SIGTERM parity and a non-copy mpv/GStreamer GL path.
6. Keep `tools/verify-browser-vaapi.sh` as the browser diagnostic. Chromium's
   snap `native` mode now reaches this driver and negotiates its VAAPI decode
   path; rerun it on a clean device window to prove successful browser frames.
   An unconfined Firefox path remains useful, then use browser logs to implement
   any browser-specific callbacks and importer lifetime requirements.
7. Continue splitting the remaining surface lifecycle and shared VA entrypoint
   helpers after `state.rs`, `image.rs`, `buffer.rs`, `buffer/handles.rs`,
   `decode.rs`, `sync.rs`, `surface_export.rs`, and `va_drm.rs`.
8. Keep `DECODER_CMD STOP` limited to explicit teardown/recovery drains. The
   sync-timeout fallback was removed because backpressure can be followed by
   more submissions; cover normal EOS and teardown drain behavior with a
   dedicated regression sample. PROBE ADDED: `tools/verify-eos-drain.sh`
   proves the teardown drain end to end (`DECODER_CMD STOP drain started
   (pending=3)` → `teardown flush done pending=0`) with a clean kernel
   window and a healthy node afterward; the natural-EOS leg is written and
   currently gated on the intermittent kernel-silent full-decode stall
   documented in PROGRESS.md (`V4L2_VA_SAMPLE=one-frame.mp4` is a low-stress
   true-EOS variant).
9. Keep `tools/verify-session-churn.sh` in the pre-browser regression set so the cross-session wedge stays covered; keep `-nostdin` on ffmpeg invocations run from automation.
