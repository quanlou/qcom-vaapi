# 06 · The roadmap, explained for a learner

The repo's `ROADMAP.md` is the terse engineering list. This chapter
translates it: *why* each item exists, which real client forces it, and
what you learn by doing it. Read `ROADMAP.md` after this and it will
feel obvious.

---

## 1. Where we are

Already true (validated on the local sample):

- `vainfo` loads the driver; H264 Baseline/Main/High VLD reported.
- 30-frame and full-300-frame FFmpeg VA-API decodes are **byte-for-byte
  identical** to the native `h264_v4l2m2m` path (`framemd5` + `cmp`).
- SPS/PPS synthesis is pinned by unit tests against golden bytes.

That means the *hard middle* of the stack works end to end:

```plantuml
@startuml
skinparam defaultTextAlignment center
(*) --> "libva loads driver" as L
L --> "config/context/surfaces" as C
C --> "frame submit\n(synthesis + QBUF)" as S
S --> "decode + pump" as D
D --> "sync + get_image\n(CPU copy)" as G
G --> (*)
note right of D : you are here\n(correct, single stream,\ncontrolled clients)
@enduml
```

What separates "works in my FFmpeg command" from "survives a browser" is
everything below.

## 2. The three production blockers in plain words

| Blocker | Symptom today | Who forces it |
|---|---|---|
| **End-of-stream drain** | Last frames of a file can be delayed or lost; sync can time out on a frame the hardware never flushes | Every player at EOF; browsers at clip end |
| **No zero-copy export** | Pixels are memcpy'd out of CAPTURE buffers into CPU images | Browsers import GPU textures; the CPU copy is a bandwidth tax and often a blocker |
| **Lifecycle hardening** | Seek storms, resolution changes, repeated open/close can wedge the session | Browsers: seeking, ads (resolution change), tab churn |

## 3. The phases

```plantuml
@startuml
skinparam defaultTextAlignment left
rectangle "Phase 1\nH264 correctness\nEOS/drain parity\nframemd5 matrix" as p1 #C8E6C9
rectangle "Phase 2\nCPU-copy client polish\nimage lifecycle\nmpv vaapi-copy" as p2 #FFF9C4
rectangle "Phase 3\ndmabuf zero-copy\nvaExportSurfaceHandle\n(VIDIOC_EXPBUF)" as p3 #FFE0B2
rectangle "Phase 4\nlifecycle hardening\nseek/flush/reconfigure\nlong-run stability" as p4 #FFCCBC
rectangle "Phase 5\nmore codecs\nHEVC · VP9 · AV1" as p5 #E1BEE7

p1 -[#333]-> p2 -[#333]-> p3 -[#333]-> p4 -[#333]-> p5
note bottom of p3
  the browser gate
end note
@enduml
```

### Phase 1 — finish H.264 correctness

**What:** every frame of any H.264 file, including the last ones,
matches the native path. Track submitted/pending frames; send
`V4L2_DEC_CMD_STOP` exactly once; pump until EOS proves nothing more is
coming (the machinery exists — `maybe_start_drain`, `eos()`,
`pending_count()` — the accounting around it must become airtight).

**Why first:** everything else is built on "the pixels are right".
A zero-copy export of *wrong* pixels is still wrong.

**What you learn:** drain semantics, FIFO/timestamp matching, and why
"async pipeline" hardware needs explicit flush contracts.

### Phase 2 — CPU-copy client polish

**What:** image lifecycle (`vaCreateImage`/`vaDeriveImage`/`vaGetImage`/
`vaDestroyImage`) without leaks of CAPTURE buffers, fds or mmaps;
useful `vaQuerySurfaceError`; mpv `--hwdec=vaapi-copy` reliable over
repeated playback. The NV12 image layout/copy code now lives in
`rust/src/image.rs` with unit coverage, CPU-owned buffer metadata/handle
callbacks live in `rust/src/buffer/handles.rs` with direct host coverage,
and the queue-all CPU-copy path remains distinct from pre-decode PRIME
reservations. Repeated playback still needs a clean device run because the
latest verifier is blocked by `/dev/video16` firmware poison.

**Why:** this is the phase where *test hygiene* becomes the product:
leak checks under repetition.

**What you learn:** ownership across three resource kinds (V4L2 buffers,
fds, mappings) and how `Drop` ordering matters.

### Phase 3 — dmabuf zero-copy (the browser gate)

**Status:** groundwork has landed — `VIDIOC_EXPBUF` on CAPTURE buffers
(`v4l2.rs`) plus a first `vaExportSurfaceHandle` returning read-only
DRM PRIME 2 NV12 descriptors (`lib.rs` + `va_drm.rs`, composed or
separate layers). What remains is exactly the hard part: export
lifetime tracking (CAPTURE buffers must not be requeued while a GPU
process still holds their fds), modifier/format verification on real
importers, and browser validation.

**What:** export CAPTURE buffers as dmabufs (`VIDIOC_EXPBUF`), implement
`vaExportSurfaceHandle`, keep CAPTURE buffers alive while exported,
requeue them only after handles are released, verify strides/offsets/
modifiers. Then: browser VAAPI logs show hardware decode, no fallback
to software.

**Why separate from phase 2:** different failure modes entirely — you
are now negotiating with the GPU importer, where a wrong stride is a
visibly corrupted texture and a premature requeue is a use-after-free in
another process.

**What you learn:** dmabuf lifetime rules, `vaExportSurfaceHandle`
semantics, and why "zero-copy" is a *lifetime* problem more than a
*copy* problem.

### Phase 4 — lifecycle hardening

**What:** flush on seek, drain after flush, safe destruction of pending
surfaces, real resolution changes (CAPTURE cycle + surface
invalidation + SPS/PPS re-emit — the `SOURCE_CHANGE` handler currently
only resumes same-size changes), error recovery that never wedges the
device, deterministic `STREAMOFF`/release, leak checks.

**Why:** browsers seek constantly and mixed-resolution playlists are
normal. A driver that hangs on seek storm #40 is dead on arrival.

**What you learn:** every state-machine corner you skipped in phase 1
comes back here with interest.

### Phase 5 — more codecs (HEVC, VP9, AV1)

**What:** per codec: profile reporting, new VA buffer types, V4L2
format setup, bitstream assembly changes (HEVC also needs
synthesized headers; VP9/AV1 differ again), conformance samples.

**Why last:** each codec multiplies the lifecycle surface area. Do it
once on a rock-solid H.264 base.

**What you learn:** what was H.264-specific in everything you built —
the honest answer usually surprises people.

## 4. Immediate next tasks (from `ROADMAP.md`), with the "why"

1. Keep EOS drain covered by the framemd5 matrix and fix the tracked B-frame xfail -> *phase 1 guardrail*
2. Image lifecycle polish + surface error reporting → *phase 2*
3. `vaExportSurfaceHandle` via `VIDIOC_EXPBUF` → *phase 3 groundwork*
4. mpv `--hwdec=vaapi-copy` testing → *phase 2 exit check*
5. Chromium/Firefox with VAAPI logging → *phase 3 requirement discovery*:
   run them, read which `va*` calls they make, and implement exactly
   those — the fastest way to find out which stubs matter.

## 5. How to work on the roadmap as a learner

- **Never skip the harness.** Every change: build, `vainfo`,
  framemd5 30-frame, then full-file. `cmp` silence is your unit test.
- **Turn each roadmap item into a reproducible failure first** (e.g. a
  seek pattern that wedges), then fix it, then keep the pattern as a
  regression command.
- **Use the debug dials** ([chapter 4 §7](04-code-tour.md#7-build-test-and-debug)):
  `V4L2_VA_DEBUG=1` for the queue-level story, `V4L2_VA_DUMP` to diff
  the exact bytes reaching the decoder against a known-good file.

---

*Next: [chapter 7 — the recipe for writing your own driver](07-write-your-own-driver.md).*
