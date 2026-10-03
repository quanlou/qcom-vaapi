# Direct surface decoding — 2026-10-03

RC7 decodes H.264, HEVC and VP9 directly into the driver-owned DMA-BUF exported
to the browser. This removes both the CAPTURE snapshot and the publication
copy from that path. The implementation is enabled automatically when Iris
exposes the decode-order controls and the surface has a compatible layout.

## How the allocation is selected

Iris has a separate internal reference-picture pool. Its linear display output
can use a chosen allocation when only that allocation is available on CAPTURE.
The driver therefore imports the current VA surface's DMA-BUF into CAPTURE
slot zero, queues no other CAPTURE targets, and checks the returned timestamp
and surface owner before binding the next allocation. Compressed input can
still use four bounded OUTPUT slots. A completed surface retains its own
storage independently of the reused queue index or decode-context lifetime.

The browser obtains the driver's allocation through the normal VA PRIME
export API. This does not require changing Chrome's allocator or Firefox.
The direct path performs no CPU transfer of decoded pixels. Compressed input
submission, initial allocation, CPU image downloads and GPU rendering still
do their usual work.

GPU importer fences are waited before storage reuse. Iris does not attach a
decode-completion fence to these allocations, so a surface exported before
decode completes during `EndPicture`. Post-decode exporters synchronize at
surface access. This preserves Chrome's early export behavior while allowing
Firefox to submit a packet containing several VP9 pictures asynchronously.

VP9 hidden references remain unchanged in the submitted bitstream. A standard
`show_existing_frame` command exposes the newly decoded reference into its VA
surface. The original frame and its export share one owner; publication occurs
once. Hidden pictures with no refreshed reference slot fail closed.

Implementation: [target selection](rust/src/v4l2/direct.rs),
[submission](rust/src/decode.rs), [completion](rust/src/v4l2/poll.rs),
[publication](rust/src/sync.rs), and [VP9 handling](rust/src/codec/raw/vp9.rs).

## Hardware and binary

- Dell XPS 13 9345, Snapdragon X Elite X1E80100, ARM64.
- Kernel `7.3.0-15-qcom-x1e`; Iris build ID
  `231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2`.
- Boot ID `1ee83e5c-6871-4794-81e7-ffbb876c1a04`.
- Driver `0.1.1-rc.7`, built with `system-av1`.
- Driver SHA-256
  `bd734b985e73d74b209b200cea071728ced2096b9916244d75ea92579f90ef7f`.
- Unchanged AV1 companion SHA-256
  `5ce5b3fc9bf59bbab03ad94ff50c3bc45d04cfae92c112278fe8d618407a2fef`.

No kernel or firmware replacement was needed for this direct path. The tested
Iris module already implements the required decode-order controls. These
results do not establish compatibility with an arbitrary distribution kernel.

## Evidence

An independent V4L2 probe cycled four explicitly chosen external MSM GEM
allocations through CAPTURE slot zero. All 60 H.264 outputs matched software
pixels after accounting for decode/display reordering. A VP9 prefix matched
all 27 visible outputs in order and exposed three hidden references in the
chosen allocations. The full VP9 probe completed 970 VA pictures, including
70 hidden pictures, with 1,040 compressed submissions and a clean kernel
window. This established hardware support before the Rust implementation.

The exact RC7 binary above passed:

| Check | Result |
| --- | --- |
| Default and all-feature unit suites | 291 passed, 4 ignored in each |
| Defensive/concurrent host suite | 295 passed, 4 ignored |
| Verifier regressions | 222 passed |
| Formatting, all-feature Clippy, shell syntax | Passed |
| YouTube VP9 3840×2160 sample | All 900 visible frames byte-exact against software, in order |
| H.264 3840×2160 | All 60 GPU-imported frames matched CPU output in order; visible pixels also matched native decode |
| HEVC Main 3840×2160 | 1/30/full 60 frames byte-exact against native decode |
| HEVC Main10 3840×2160 | 1/30/full 60 frames byte-exact against software P010 |
| Resolution replacement | 780/780 frames, four 960×640/1280×720 transitions, unchanged post-test sanity frame |
| Existing AV1 complete-buffer replay | 99 coded frames and 96 visible projections byte-exact |

All completed hardware checks had clean kernel observation windows and
released their decoder sessions. The AV1 replay checks the existing copy path;
AV1 does not use the new direct-target path. Stock FFmpeg's tile-only AV1 VA
transport was rejected before hardware submission, as expected by the existing
complete-buffer implementation; it is not a supported AV1 producer.

## Browser results and limits

Chrome 154.0.8037.97 played the 4K60 VP9 sample in a private native browser
profile, acknowledged a seek, resumed playback and exited cleanly. The final
run presented 817 frames over 15.0008 seconds: 54.46 presented frames/s, zero
reported dropped frames, and 885 hardware completions. It passed the configured
50 frames/s minimum, 1% drop ceiling, 10-second minimum and 3,000,000 KiB summed
RSS ceiling. **This is not a sustained 60 frames/s qualification.**

An eight-second process CPU sample measured Chrome's GPU process at 9.80% of
one CPU core, down from 47.19% with the prior one-copy candidate on the same
fixture. GPU CPU time per submitted picture fell from 12.475 ms to 1.677 ms.
The final browser family used 14.27% of one core; the measurement process is
excluded from that total. These are isolated local-fixture measurements, not
a promise about total CPU use on a live YouTube page.

Firefox 157.0 on native Wayland confirmed direct driver completion and
successful zero-copy EGL imports of both video planes. Increasing bounded
compressed-input pipelining stopped its slow-decode software fallback. Its
4K60 tests nevertheless dropped roughly 80% of frames and failed the strict
playback gate. The remaining bottleneck has not been established; zero-copy
GPU imports alone do not prove smooth presentation.

A separate 4K30 test retimes the same compressed VP9 packets without
re-encoding. The exact RC7 binary presented 435 frames in 15.057 seconds
(28.89 frames/s), with three dropped frames out of 438 total, 495 hardware
completions and 1,607,792 KiB summed RSS. Playback, seek and cleanup pass at
the 28 frames/s minimum and 1% drop ceiling. It qualifies that fixture at
30 fps, not the original 60 fps video. Browser tests use temporary profiles and explicit hardware
acceleration settings; they do not establish that every user profile enables
the VA path automatically.

AV1, caller-imported storage, contexts declaring any imported target, and
kernels without decode-order controls retain the compatibility copy path.
An active direct session cannot switch to an undeclared incompatible imported
target. CPU image requests download pixels explicitly. Other chips, 10-bit
AV1, general suspend/resume and sustained Firefox 4K60 remain unqualified.

## Reproduction and saved receipts

Build with `V4L2_VA_BUILD_FEATURES=system-av1 tools/build-rust-driver.sh OUTDIR`.
Use `tools/verify-4k-decode.sh` for Main10 parity,
`tools/verify-gl-roundtrip.sh` with `V4L2_VA_STRICT=1` for full ordered GPU import,
and `tools/verify-resolution-churn.sh` for context replacement. The browser
verifier supports `V4L2_VA_BROWSER`, `V4L2_VA_SAMPLE` and strict performance
thresholds; Firefox's native Wayland check uses `GDK_BACKEND=wayland`.

On the tested host, complete receipts are preserved under
`/home/mq/.cache/libva-v4l2-qualification/true-zero-copy-20261003/`.
`rc7/final/` contains the exact-binary parity, import, resolution and browser
checks; `chosen-*.json` and the chosen-buffer probe preserve the independent
hardware experiment. Earlier failed gates and intermediate candidates remain
separate. The installed development package carries `qualification.json`
with exact artifact hashes and receipt paths.
