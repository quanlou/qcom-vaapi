# Reduce decoded-frame CPU copies

**Experimental candidate; not enabled in normal builds or installed.**
GPU parity passed all 48 transfers across NV12/P010 through 4K, including padding
and caller guard bytes. A captured 32-frame 4K AV1 VA replay also matched its
software reference; every frame published on the GPU with zero CPU pixel-copy
bytes. These tests ran after the user authorized acknowledging five earlier
recovered Iris session errors.

After reboot, the combined candidate passed H.264 1/30/full, export/GL,
resolution changes, 3,600-frame playback and HEVC/Main10. VP9 failed between
sessions. A stable-pool VP9 trial also failed and was withdrawn. The next
changed candidate matched one VP9 output frame with 14 direct publications,
then stopped on kernel memory-accounting BUGs during the following session.
The boot has one Iris system error and three memory BUG records; their cause
is unproven. No further device tests are running on that boot.

The drain fix now has hardware evidence: a separate scratch CAPTURE slot
received STOP's final marker after the published frame, without overwriting its
pixels. The next VP9 session still failed. Keeping VP9's default output mode
also failed after one GPU publication and added an Iris system error.

The current private candidate uses kernel-allocated VP9 CAPTURE buffers and
exports completed frames internally to the GPU. Export handles stay with their
allocation, do not count as client reservations, and need no CPU pixel mapping
when GPU transfer succeeds. The source remains pinned through publication.
A recovered-firmware diagnostic matched one requested VP9 output and logged
14 GPU publications, but the next session stalled and added two system errors.
The native-buffer path remains experimental.

The latest change stops OUTPUT and CAPTURE before releasing either allocation
pool, matching native FFmpeg's close ordering. A mocked ioctl test proves both
stop requests precede pixel unmapping even when input stop fails. Debug logs
record both stop and queue-release results. This change passes host checks and
has not run on hardware. Full matrix, session churn and Chrome/Firefox
validation remain pending. The candidate is not installed or release-ready.

Ordinary FFmpeg AV1 input was rejected during assembly before GPU publication;
the successful AV1 check used frozen complete VA buffers.

| Frame path | Pixel movement |
| --- | --- |
| Matching H.264 / HEVC / VP9 buffers | Iris writes the surface directly |
| Other linear layouts | Optional GPU transfer |
| AV1 retained frames | Optional GPU transfer to independent surfaces |
| Unsupported GPU imports | CPU fallback |
| Requested CPU images | CPU readback |

The `gpu-copy` feature imports source and destination DMA-BUF planes into an
Adreno EGL context and blits raw bytes. NV12 and P010 use byte views, preserving
10-bit values without color conversion. Planes can have separate strides and
offsets. Caller prefixes, padding, gaps and tails are preserved; driver-owned
padding is cleared on the GPU.

This avoids decoded-pixel CPU transfers where the GPU accepts the layout.
It still moves pixels on the GPU when buffers cannot be shared directly.
Compressed-packet assembly and explicit CPU image access can still copy bytes.
Chrome and Firefox must also use their hardware decode/import paths; a VA-API
profile listing alone cannot prove that.

## Ownership

Direct AV1 CAPTURE remains disabled. Hidden owners keep separate surface storage
and publish with the displayed frame. Their shared source stays pinned until
all publications complete. A GPU fence completes before CAPTURE becomes free.
An uncertain fence stops publication and further decode, retaining imported
storage; CPU fallback is allowed only before destination writes begin.
Separate GPU completion fences order padding clears before visible-plane
blits: independently imported views can alias one allocation without Mesa
tracking that relationship. Page tails use a four-row view to satisfy
Freedreno's linear-import over-fetch margin without exceeding the allocation.

The context uses the app's existing DRM fd and restores the calling thread's
EGL API, context and surfaces. Runtime requirements are `libEGL.so.1`,
`libGLESv2.so.2`, `libgbm.so.1`, GLES 3.1 and renderable linear R8 imports.
Unavailable support leaves the CPU path in place. There are no new mandatory
libraries for normal builds.

## Check the candidate

Host checks open neither the GPU nor decoder:

```sh
./tools/test-gpu-copy-host.sh
cargo test --manifest-path rust/Cargo.toml --features gpu-copy,system-av1
cargo clippy --manifest-path rust/Cargo.toml --all-targets \
    --features gpu-copy,system-av1 -- -D warnings
```

Build a private candidate:

```sh
V4L2_VA_BUILD_FEATURES=gpu-copy,system-av1 \
    ./tools/build-rust-driver.sh /tmp/qcom-vaapi-gpu-copy
```

`tools/qualify-gpu-copy.py` requires a new evidence directory, the expected
kernel and all three loaded module build IDs. It checks the whole boot, takes
the shared hardware lease and requires an idle decoder before opening the GPU.
It runs 48 transfers across NV12/P010, odd sizes, HD and 4K, with repeated owned
and imported buffers. Comparisons include padding and guard bytes. CPU fixture
setup/readback is explicit; a transfer that falls back to the CPU cannot pass.
The packet is sealed after one attempt and must not be retried automatically.
An explicitly authorized `--acknowledged-session-baseline` can pin an exact
prior set of recovered session errors to the boot and loaded modules. Any new
error or system/memory/GPU fault still stops the diagnostic. Normal checks keep
their original strict whole-boot gate.

Passing GPU parity is the first gate. Run `tools/verify-rust-driver.sh` and
`tools/verify-session-churn.sh` next, preserving all required checks. Finally
observe Chrome/Firefox with driver diagnostics: GPU publication records report
`cpu_copy_bytes=0`; direct completion records report `copy_bytes=0`. Unsupported
layouts and sandbox restrictions must be counted as fallbacks, not successes.
