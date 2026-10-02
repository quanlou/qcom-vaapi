# 06 · The roadmap, explained for a learner

The repo's `ROADMAP.md` is the terse engineering list. This chapter
translates it: *why* each item exists, which real client forces it, and
what you learn by doing it. Read `ROADMAP.md` after this and it will
feel obvious.

---

## 1. Where we are

The driver supports H.264 Baseline/Main/High, HEVC Main/Main10, and VP9 Profile
0. AV1 has an experimental implementation but remains unadvertised by default.
The latest strict hardware gate passed H.264 1/30/300 parity, exact 300-frame
GL output, 780-frame resolution churn, 3,600-frame playback, 30-frame parity
for HEVC/Main10/VP9, EOS, churn, and 24 ordinary seeks. It failed the mixed
seek phase on the supplied transport stream; an indexed Matroska remux passed
the same hardware seek checks, but does not change the failed gate result. A
fresh strict gate is running. Production qualification is not complete. See
[`PROGRESS.md`](../PROGRESS.md) and the [current hardware record](production-resumption-20261001.txt)
for the live status and exact evidence.

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
note right of D : implemented;\nrelease qualification\nin progress
@enduml
```

What separates "works in my FFmpeg command" from "survives a browser" is
everything below.

## 2. The three production blockers in plain words

| Blocker | Symptom today | Who forces it |
|---|---|---|
| **Release qualification** | Strict mixed seek gate is being rerun after a transport-stream parser/reference failure; sustained 4K and browser budgets remain separate | Deployment sessions |
| **PRIME and GL ownership** | Export/import works for qualified probes, but stable exported allocations currently need CPU copies; this does not prove a copy-free browser path | GPU importers and browsers |
| **Small-stream/kernel edge cases** | Recovery-v4 passed the supplied one-frame and B-frame probes; broader firmware reliability and persistent kernel deployment remain open | Short clips and deployment lifecycle |

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
reservations. The latest clean hardware gate passed churn 7/7 and the 3,600-frame
playback check; current release qualification is tracked separately below.

**Why:** this is the phase where *test hygiene* becomes the product:
leak checks under repetition.

**What you learn:** ownership across three resource kinds (V4L2 buffers,
fds, mappings) and how `Drop` ordering matters.

### Phase 3 — dmabuf zero-copy (the browser gate)

**Status:** PRIME export and GL import have been exercised by the strict
300-frame gate. Exported surfaces use stable reservations and CPU copies to
provide completed pixels; do not describe this as a copy-free zero-copy path.
The deployment browser playback/performance gate is still pending.

**What:** preserve export lifetime and layout correctness, measure the copy
and importer behavior, then verify the deployment browser's real decode, seek,
frame coverage, and performance budget.

**Why separate from phase 2:** different failure modes entirely — you
are now negotiating with the GPU importer, where a wrong stride is a
visibly corrupted texture and a premature requeue is a use-after-free in
another process.

**What you learn:** dmabuf lifetime rules, `vaExportSurfaceHandle`
semantics, and why "zero-copy" is a *lifetime* problem more than a
*copy* problem.

### Phase 4 — lifecycle hardening

**Status:** the latest clean gate passed 780 frames across four resolution
changes, 3,600-frame playback, churn 7/7, EOS checks, and 24 ordinary seeks.
The supplied mixed-resolution transport-stream seek case failed with parser and
reference errors also seen in a software control. An indexed Matroska remux
passed all 12 mixed seeks; the full strict gate is being rerun with that fixture.

**What:** flush on seek, drain after flush, safe destruction of pending
surfaces, real resolution changes, error recovery that never wedges the
device, deterministic `STREAMOFF`/release, and leak checks.

**Why:** browsers seek constantly and mixed-resolution playlists are
normal. A driver that hangs on seek storm #40 is dead on arrival.

**What you learn:** every state-machine corner you skipped in phase 1
comes back here with interest.

### Phase 5 — more codecs (HEVC, VP9, AV1)

**Status:** HEVC Main, HEVC Main10, and VP9 Profile 0 are implemented; the
latest strict hardware gate passed 30-frame parity for each. Main10 uses a
software HEVC reference converted to P010 because the native wrapper fails on
this fixture. AV1 OBU synthesis exists experimentally, but full-stream parity
and authoritative refresh/sequence metadata are missing, so AV1 stays hidden.

**What:** per codec: profile reporting, new VA buffer types, V4L2 format setup,
bitstream assembly changes, and native parity samples.

**Why last:** each codec multiplies the lifecycle surface area. Do it
once on a rock-solid H.264 base.

**What you learn:** what was H.264-specific in everything you built —
the honest answer usually surprises people.

## 4. Immediate next tasks (from `ROADMAP.md`), with the "why"

1. Complete the current strict qualification run; preserve the mixed-seek
   failure record even if the indexed-fixture rerun passes.
2. Run sustained 4K and deployment-browser checks with explicit FPS, memory,
   duration, seek, frame-coverage, kernel, and process-exit requirements.
3. Investigate the historical small-stream firmware failures and complete
   persistent kernel deployment qualification.
4. Keep AV1 unadvertised until the producer supplies authoritative metadata
   (or compressed headers) and full-stream parity passes.

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
