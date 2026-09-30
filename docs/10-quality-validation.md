# Quality acceptance and 4K validation

Known decode errors, missing frames, software fallback, and incorrect pixels
remain release blockers even when a shorter baseline passes. An unsupported
stream must fail explicitly rather than produce corrupted output. AV1 remains
experimental until full-stream parity works without encoder-specific guesses.

## Current AV1 boundary

Enable the diagnostic path with `V4L2_VA_EXPERIMENTAL_AV1=1`. The 720p sample's
first 30 displayed frames now match libdav1d exactly. Two corrections were
needed: translate VA restoration enums back to compressed syntax, and emit
hidden reference frames into their retained VA surfaces. FFmpeg handles a later
`show_existing_frame` by reusing a surface without another driver submission.

VA picture parameters omit reference refresh flags and some sequence syntax.
The current refresh heuristic disagrees with the original full sample at 43
headers, starting at order hint 64. A shadow reference map detects disagreement
with subsequent VA parameters before another dependent submission. Production
support needs authoritative header metadata or preserved compressed headers;
extending this particular encoder's refresh pattern is insufficient.

The frame writer also corrects 128x128-superblock restoration-unit shift syntax
and the condition for coding per-frame integer-motion selection. Regression
fixtures include the original hidden-reference header and original 4K H.264 PPS.

## Reproduce the 4K H.264 fixture

The fixture is a 60-frame, 3840x2160, 30 fps synthetic stream with B-frames:

```sh
mkdir -p /home/mq/tmp/vaatest/quality4k
ffmpeg -y -nostdin -hide_banner -v error \
  -f lavfi -i testsrc2=size=3840x2160:rate=30 -frames:v 60 \
  -c:v libx264 -threads 4 -preset veryfast -crf 22 \
  -profile:v high -level:v 5.1 -g 30 -bf 2 -pix_fmt yuv420p \
  /home/mq/tmp/vaatest/quality4k/h264-2160p.mp4
tools/verify-4k-decode.sh /path/to/driver
```

The original fixture SHA-256 is
`aeac1c9f234347db82df88fabdce10d22fd59f2d5341c7c50d64401680e34532`.
Encoder versions can change the file; the verifier generates its native
reference from the actual input rather than depending on this checksum.

The verifier requires byte-exact 1/30/full frame parity and clean kernel fault
counts. It requests hardware output explicitly and downloads those frames, so
software fallback cannot satisfy the gate. Missing inputs/references fail.

Use `V4L2_VA_4K_CODEC=hevc`, `hevc10`, or `vp9` to test the corresponding
`quality4k/<codec>-2160p.mp4` fixture (`.webm` for VP9). Main10 uses a software
HEVC-to-P010 reference because the native wrapper fails Main10 on this host.
Other codecs use their native V4L2 wrappers. Override the sample and log paths
with `V4L2_VA_4K_SAMPLE` and `V4L2_VA_4K_LOG_DIR`.

Probes are sequential and hold `/tmp/libva-v4l2-hardware.lock`; other hardware
test processes must share that lock. Host-only work can continue independently.

This correctness gate does not establish sustained 4K60 throughput, browser
import performance, HDR/color-metadata handling, or long-playback reliability.
Those requirements remain separate pending work.

## Additional verified lifecycle coverage

The 4K H.264 fixture also passes the session-churn probe with 30-frame cuts:

```sh
V4L2_VA_SAMPLE=/home/mq/tmp/vaatest/quality4k/h264-2160p.mp4 \
V4L2_VA_CHURN_CUT_FRAMES=30 \
V4L2_VA_CHURN_DIR=/tmp/libva-v4l2-4k-churn \
  tools/verify-session-churn.sh /path/to/driver
```

The HEVC malformed-tile-array regression from the host stress audit is fixed.
Counts must fit the supplied arrays, and explicit tile widths/heights must leave
space for a nonempty final tile in the coded picture. The isolated host stress
runner now passes all four stress checks and its parallel test suite.
