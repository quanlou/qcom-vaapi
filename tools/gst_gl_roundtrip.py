#!/usr/bin/env python3
"""GL-importer NV12 layout validation for the Rust V4L2 VA driver.

Compares two raw I420 dumps of the same H.264 sample:

  GL path   vah264dec ! glupload ! gldownload ! videoconvert ! I420 ! filesink
            (pixels sampled through the driver's exported dma-buf descriptor)
  Reference ffmpeg -hwaccel vaapi -f rawvideo
            (pixels moved by the driver's vaGetImage CPU-copy path; the
            framemd5 matrix in tools/verify-rust-driver.sh already proved
            that view byte-equal to the native decoder)

If the exported descriptor had wrong plane offsets, strides, or sizes, the
GL-sampled pixels would not match the CPU-copy pixels and the per-frame
hashes would diverge.

Production qualification uses --ordered and compares the complete stream:
frame count, display order, and every visible pixel must match the reference.
Extra, missing, or reordered frames fail even if all reference hashes appear
somewhere in the GL dump.

Diagnostic comparisons may omit --ordered and compare a reference prefix by
hash occurrence counts. This mode can isolate plane layout errors despite
reordering, but it does not establish successful full-stream playback.
Explicit --max-missing tolerance is diagnostic only; the shell release gate
always requires zero missing frames.

Both files are hashed stride-aware. ffmpeg's rawvideo encoder packs with
alignment 1 (stride == width); GStreamer pools may pad the stride, so the
GL side usually needs derivation. Ambiguous stride matches fail.

This replaces the earlier PyGObject appsink design: the installed
python3-gst bindings crash inside GstVideo boxed types on this host.

Output: one "ref_index,hash,found_at_gl_index|missing" line per ref frame
and a final "gl_roundtrip=pass|fail ..." summary. Exit 0 when the requested
comparison contract is satisfied.
"""

import argparse
import hashlib
import os
import sys

# Stride paddings seen in the wild: ffmpeg rawvideo packs at alignment 1,
# GStreamer pools round strides up to a multiple of 4..256 depending on the
# element and platform.
STRIDE_PADS = (0, 4, 8, 16, 32, 64, 128, 256, 512)


def frame_bytes(stride, height):
    return stride * height * 3 // 2


def chroma_stride(stride):
    return stride // 2


def packed_i420_md5(data, stride, width, height, index):
    """MD5 of one frame's visible pixels from a contiguous I420 dump."""
    fb = frame_bytes(stride, height)
    base = index * fb
    cstride = chroma_stride(stride)
    chroma_h = height // 2
    chroma_w = width // 2
    u_off = base + stride * height
    v_off = base + stride * height * 5 // 4

    digest = hashlib.md5()
    for row in range(height):
        start = base + row * stride
        digest.update(data[start:start + width])
    for plane_off in (u_off, v_off):
        for row in range(chroma_h):
            start = plane_off + row * cstride
            digest.update(data[start:start + chroma_w])
    return digest.hexdigest()


def stride_candidates(size, width, height):
    """Strides that make the file size an integer number of I420 frames."""
    luma_total = size * 2 // (3 * height)  # == frames * stride
    if luma_total * 3 * height != size * 2:
        return []
    out = []
    for pad in STRIDE_PADS:
        stride = width + pad
        if stride % 2:
            continue
        if luma_total % stride == 0 and luma_total // stride >= 1:
            if stride not in out:
                out.append(stride)
    return out


def load_layout(path, width, height, forced_stride, label):
    """Return (data, [(stride, frame_count), ...]) for one raw dump."""
    size = os.path.getsize(path)
    if size == 0:
        raise SystemExit(
            "gst_gl_roundtrip: {} dump is empty".format(label)
        )
    with open(path, "rb") as f:
        data = f.read()
    candidates = [forced_stride] if forced_stride else \
        stride_candidates(size, width, height)
    if not candidates:
        raise SystemExit(
            "gst_gl_roundtrip: no integer I420 frame count fits {} "
            "(size={} width={} height={}); pass --stride/--ref-stride"
            .format(path, size, width, height)
        )
    if any(s < width or s % 2 or size % frame_bytes(s, height) for s in candidates):
        raise SystemExit("gst_gl_roundtrip: invalid stride or truncated {} dump".format(label))
    layouts = [(s, size // frame_bytes(s, height)) for s in candidates]
    return data, layouts


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("gl_raw", help="raw I420 from the GL download path")
    parser.add_argument("ref_raw", help="raw I420 from the ffmpeg reference")
    parser.add_argument("--width", type=int, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--frames", type=int, default=0,
                        help="compare at most this many frames "
                             "(default: all comparable frames)")
    parser.add_argument("--stride", type=int, default=0,
                        help="force the GL dump stride")
    parser.add_argument("--ref-stride", type=int, default=0,
                        help="force the reference dump stride")
    parser.add_argument("--max-missing", type=int, default=0,
                        help="tolerate up to this many ref frames missing "
                             "from the gl set (gst-vaapi occasionally drops "
                             "a frame under heavy pool churn; default: 0)")
    parser.add_argument("--ordered", action="store_true",
                        help="require exactly the reference frames in display order")
    args = parser.parse_args()
    if args.width <= 0 or args.height <= 0 or args.width % 2 or args.height % 2:
        parser.error("I420 width and height must be positive and even")
    if args.stride < 0 or args.ref_stride < 0:
        parser.error("strides must be nonnegative")
    if args.frames < 0 or args.max_missing < 0:
        parser.error("--frames and --max-missing must be nonnegative")

    gl_data, gl_layouts = load_layout(
        args.gl_raw, args.width, args.height, args.stride, "GL"
    )
    ref_data, ref_layouts = load_layout(
        args.ref_raw, args.width, args.height, args.ref_stride, "reference"
    )

    def hash_frames(data, stride, frame_count, limit):
        """MD5 of every frame's visible pixels for the given stride."""
        n = frame_count if limit <= 0 else min(frame_count, limit)
        return [
            packed_i420_md5(data, stride, args.width, args.height, i)
            for i in range(n)
        ]

    # Cap the reference at args.frames when set; the gl dump usually holds
    # many more (300 vs 30 for the standard 720p sample), and every ref
    # frame must appear byte-exact in the gl set for the layout to be
    # judged correct.
    ref_scan_limit = args.frames if args.frames > 0 else 0

    # Score each (gl_stride, ref_stride) pair by how many ref frames land
    # byte-exact in the gl frame set. Correct layout → high score for
    # exactly one pair; wrong layout at any plane → zero for every pair.
    scored = []
    for gl_stride, gl_frames in gl_layouts:
        gl_hashes = hash_frames(gl_data, gl_stride, gl_frames, 0)
        gl_set = set(gl_hashes)
        for ref_stride, ref_frames in ref_layouts:
            ref_hashes = hash_frames(
                ref_data, ref_stride, ref_frames, ref_scan_limit
            )
            hits = sum(1 for h in ref_hashes if h in gl_set)
            scored.append((
                hits, gl_stride, gl_frames, gl_hashes,
                ref_stride, ref_frames, ref_hashes,
            ))

    scored.sort(key=lambda t: t[0], reverse=True)
    top = scored[0]
    top_hits = top[0]
    if top_hits == 0:
        raise SystemExit(
            "gst_gl_roundtrip: no stride pair produces any layout match "
            "(gl={} ref={}); pass --stride/--ref-stride to force a layout"
            .format(gl_layouts, ref_layouts)
        )
    ties = [t for t in scored if t[0] == top_hits]
    if len(ties) > 1:
        raise SystemExit(
            "gst_gl_roundtrip: ambiguous stride derivation "
            "(tied at {} hits: {}); pass --stride/--ref-stride".format(
                top_hits,
                [(t[1], t[4]) for t in ties],
            )
        )

    _, gl_stride, gl_frames, gl_hashes, ref_stride, ref_frames, ref_hashes = top

    if args.frames > 0 and len(ref_hashes) != args.frames:
        raise SystemExit("gst_gl_roundtrip: reference has {} frames, expected {}".format(
            len(ref_hashes), args.frames))

    if args.ordered and gl_hashes != ref_hashes:
        print("gl_roundtrip=fail reason=display_order_or_frame_count "
              "gl_frames={} ref_frames={}".format(len(gl_hashes), len(ref_hashes)))
        return 1

    # Build the ref→gl position index once so per-frame lines report where
    # each ref frame landed in the gl dump (helpful for reorder audits).
    gl_index = {}
    for i, h in enumerate(gl_hashes):
        gl_index.setdefault(h, []).append(i)

    misses = 0
    for i, h in enumerate(ref_hashes):
        pos = gl_index.get(h)
        if not pos:
            misses += 1
            print("{},{},missing".format(i, h))
        else:
            print("{},{},{}".format(i, h, pos.pop(0)))

    total = len(ref_hashes)
    if args.frames > 0:
        total = min(total, args.frames)
    tolerated = args.max_missing
    if misses > tolerated:
        print(
            "gl_roundtrip=fail ref_frames={} missing={} tolerated={} "
            "gl_stride={} ref_stride={}".format(
                total, misses, tolerated, gl_stride, ref_stride
            )
        )
        return 1
    print(
        "gl_roundtrip=pass ref_frames={} missing={} tolerated={} "
        "gl_stride={} ref_stride={}".format(
            total, misses, tolerated, gl_stride, ref_stride
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
