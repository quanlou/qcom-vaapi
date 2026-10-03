# Producer-side AV1 diagnostics

The C probes in this directory inspect AV1 packets using FFmpeg's CBS
interfaces. Most are host-side diagnostics. The complete-buffer companion is an opt-in
experimental runtime component described below. The
project's MIT license applies to these original probes.

The `ffmpeg-*.patch` files are diagnostic changes for an FFmpeg checkout and
are licensed GPL-2.0-or-later; see
[`LICENSE-GPL-2.0-or-later`](LICENSE-GPL-2.0-or-later). The repository-root
MIT license does not relicense those patches or FFmpeg source files. They are
experimental aids, not production dependencies.

`ffmpeg-av1-cbs-va-transport.patch` is an opt-in paired-producer experiment.
It calls the CBS copy-on-write helper from FFmpeg's actual AV1 VA callbacks,
preserving the parser's original hidden-frame/reference state. Standard slice
data contains serialized sequence/frame OBUs; standard tile offsets address
the unchanged tile payload within that buffer. It uses no reserved VA fields.
Only complete FRAME OBUs with one complete tile group are accepted. Duplicate,
partial, changed or out-of-bounds payload submissions fail closed.

The transport and original parser ownership passed software corpus checks,
including 10-bit input. The patched FFmpeg builds, and its actual slice callback
passed seven offline cases with VA submission/cancel replaced by in-process
captures. The paired driver now validates the real ownership prefix, tile bounds,
reference maps and live surfaces. Frozen hardware experiments passed all 492
displayed frames of the original, rav1e and SVT 8-bit samples with exact pixels
and order, clean teardown and clean kernel windows. The first libaom gate failed
because its software reference used a different pixel layout; a separate NV12
audit matches all 96 frames, and the failed gate remains preserved.

These results describe the paired FFmpeg experiment. The rc.5 system package
also supports Chrome through the complete-buffer companion below; 10-bit AV1
hardware remains untested. The default source build keeps AV1 opt-in.

`av1-cbs-complete-buffer.h` also handles complete original buffers, as submitted
by the installed Chromium 152 VA delegate. It preserves original CBS reader
state, normalizes each coded frame separately, and retains original tile offsets
for validation. All 591 corpus captures match the paired producer output,
including packets containing multiple coded frames. Bounded ownership and
peek/commit checks reject replacement of unconsumed frames.

`av1-cbs-complete-library.c` builds this helper as a separate opt-in companion
linked with FFmpeg CBS. Its six private, versioned symbols are the only exports;
FFmpeg symbols remain local to avoid interposing on the browser's libraries.
The Rust loader validates the ABI, tile bytes, original visibility and refresh
mask before committing each generation. A frozen qualification packet explicitly
selects the companion with `V4L2_VA_AV1_COMPLETE_LIBRARY`; this is a private helper
interface, not an extension of the VA ABI. The linked FFmpeg build's license and
redistribution obligations also apply to the companion artifact.

Actual complete-original-buffer VA replay passed all 591 coded frames of the
four 8-bit samples, including hidden frames, plus all 588 original display
projections with exact NV12 pixels and order, clean teardown and kernel windows.
The rc.5 decode source additionally passed a 510-frame 4K replay, strict
codec/export/session checks, and Chrome 4K playback with seeking. Live YouTube
also used hardware decoding, with some frame timing unevenness. The system-av1
package advertises AV1 by default. See [release details](../docs/releases/0.1.1-rc.5.md)
for the tested kernel, binary hashes, limitations and companion build recipe.
