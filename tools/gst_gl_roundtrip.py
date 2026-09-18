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

Both files are hashed stride-aware: the stride is derived from the file size
(contiguous I420: bytes = frames * stride * height * 3 / 2), with the
candidate set disambiguated by matching frame 0 across the two files when
more than one stride divides evenly. ffmpeg's rawvideo encoder packs with
alignment 1 (stride == width); GStreamer pools may pad the stride, so the
GL side usually needs the derivation.

This replaces the earlier PyGObject appsink design: the installed
python3-gst bindings crash inside GstVideo boxed types on this host.

Output: one "frame,gl_md5,ref_md5,status" line per compared frame and a
final "gl_roundtrip=pass|fail ..." summary. Exit 0 only on full match.
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
    args = parser.parse_args()

    gl_data, gl_layouts = load_layout(
        args.gl_raw, args.width, args.height, args.stride, "GL"
    )
    ref_data, ref_layouts = load_layout(
        args.ref_raw, args.width, args.height, args.ref_stride, "reference"
    )

    if len(gl_layouts) > 1 or len(ref_layouts) > 1:
        # Disambiguate: frame 0 holds the same pixels in both files, so
        # exactly one (gl_stride, ref_stride) pair can agree.
        ref_hashes = {
            (s, n): packed_i420_md5(ref_data, s, args.width, args.height, 0)
            for s, n in ref_layouts
        }
        gl_hashes = {
            (s, n): packed_i420_md5(gl_data, s, args.width, args.height, 0)
            for s, n in gl_layouts
        }
        matches = [
            (g, r)
            for g in gl_layouts for r in ref_layouts
            if gl_hashes[g] == ref_hashes[r]
        ]
        if len(matches) != 1:
            raise SystemExit(
                "gst_gl_roundtrip: ambiguous stride derivation "
                "(gl={} ref={} frame0_matches={}); pass --stride/--ref-stride"
                .format(gl_layouts, ref_layouts, len(matches))
            )
        (gl_stride, gl_frames), (ref_stride, ref_frames) = matches[0]
    else:
        gl_stride, gl_frames = gl_layouts[0]
        ref_stride, ref_frames = ref_layouts[0]

    frames = min(gl_frames, ref_frames)
    if args.frames > 0:
        frames = min(frames, args.frames)
    if frames < 1:
        raise SystemExit(
            "gst_gl_roundtrip: no frames to compare "
            "(gl={} ref={})".format(gl_frames, ref_frames)
        )

    mismatches = 0
    for index in range(frames):
        gl_md5 = packed_i420_md5(gl_data, gl_stride, args.width, args.height,
                                 index)
        ref_md5 = packed_i420_md5(ref_data, ref_stride, args.width, args.height,
                                  index)
        status = "match" if gl_md5 == ref_md5 else "mismatch"
        if status == "mismatch":
            mismatches += 1
        print("{},{},{},{}".format(index, gl_md5, ref_md5, status))

    if mismatches:
        print("gl_roundtrip=fail frames={} mismatches={} "
              "gl_stride={} ref_stride={}".format(
                  frames, mismatches, gl_stride, ref_stride))
        return 1
    print("gl_roundtrip=pass frames={} gl_stride={} ref_stride={}".format(
        frames, gl_stride, ref_stride))
    return 0


if __name__ == "__main__":
    sys.exit(main())
