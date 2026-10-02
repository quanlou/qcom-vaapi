# Producer-side AV1 diagnostics

The C probes in this directory inspect AV1 packets using FFmpeg's CBS
interfaces. They are host-side diagnostics, not part of the VA driver. The
project's MIT license applies to these original probes.

The `ffmpeg-*.patch` files are diagnostic changes for an FFmpeg checkout and
are licensed GPL-2.0-or-later; see
[`LICENSE-GPL-2.0-or-later`](LICENSE-GPL-2.0-or-later). The repository-root
MIT license does not relicense those patches or FFmpeg source files. They are
experimental aids, not production dependencies.
