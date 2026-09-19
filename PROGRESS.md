# Progress / agent handoff

This file is the coordination point for concurrent or follow-up agents. Keep it
short and update it whenever a task starts, finishes, or gets blocked.

## Coordination rules

- Read this file before changing code.
- Put the current task under **Active task** before starting substantial work.
- Move completed work to **Completed recently** with the validation command that
  passed.
- Do not weaken the required verification matrix in `tools/verify-rust-driver.sh`:
  `sample-1`, `sample-30`, and `sample-full` must stay required.
- Probe order matters: keep the required matrix first, the working GStreamer
  export probe next, and optional/diagnostic stress probes (`ffmpeg hwmap`,
  `one-frame-eos`, `bframes-240p`) after that. These diagnostics can poison the
  next hardware session.
- If a change touches V4L2 queues, surface publication, export, or teardown,
  run both `tools/verify-rust-driver.sh` and `tools/verify-session-churn.sh`
  before marking it done.

## Active task

- Phase 2 CPU-copy compatibility (codex agent, 2026-09-19 latest loop): NOT COMPLETE. Current tree adds surface ownership to `ReplayChunk`, inserts FIFO before the post-QBUF pump, preserves published timestamps across rebuilds, avoids recording replay QBUFs back into replay history, and makes `vaSyncSurface` pump immediately after starting a sync STOP. Unit tests pass (`cargo test --manifest-path rust/Cargo.toml`, 91 tests). Hardware still fails the focused 30-frame FFmpeg CPU-copy probe with `rc=251`, timing out on `surface=0x40000003`. Latest artifact/log: `/tmp/libva-v4l2-rust-driver-phase2-clean-history-20260919`, `/tmp/phase2-clean-history-30.log`. Latest evidence: same-session STOP/START can publish a later reordered frame (`surface=0x40000008`, timestamp 300000) but loses earlier pending surface `0x40000003`; rebuild/replay no longer duplicates history, but rebuilt sessions still only produce source-change empty CAPTURE/EOS markers and then fill OUTPUT/stall. Delayed rebuild until 16 chunks did not satisfy the phase gate. Phase 2 remains blocked on a way to release the first frames without destroying reference continuity for the pending B-frame at timestamp 66667.
- Phase 2 CPU-copy compatibility (codex agent, 2026-09-19 current): NOT COMPLETE. Latest experimental tree still fails the required FFmpeg 30-frame CPU-copy decode with `rc=251`; do not run the full verifier/churn and claim success until this is fixed. Unit tests pass (`cargo test --manifest-path rust/Cargo.toml`, 91 tests). New evidence since the 15:47 note: event-time/source-change `DECODER_CMD_START` is not the root fix. In rebuilt replay sessions, accepted START tends either to make OUTPUT reject later QBUFs with EIO, or with a full OUTPUT queue to return only empty CAPTURE markers and then stall. Disabling START avoids EIO but fills all 16 OUTPUT buffers and stalls. Sync STOP is still the only operation that releases the first three frames, but repeated STOP+rebuild/replay loses the surface mapping for later drained frames: after the second STOP the submit path can rebuild before the sync loop publishes CAPTURE, and replay currently queues history without FIFO surface ownership. Current artifacts/logs: `/tmp/libva-v4l2-rust-driver-phase2-fullqueue-start-20260919`, `/tmp/phase2-fullqueue-start-30.log`; rejected earlier artifacts include `phase2-marker-start`, `phase2-no-sourcechange-start-stop`, `phase2-drain-before-rebuild`, and `phase2-no-stop-sourcechange`. Immediate next implementation direction: stop treating sync-drain rebuild as a transparent stateless replay. Either (a) keep the old drained CAPTURE publications alive and defer rebuild until the sync loop consumes them, with explicit state so submit cannot tear down an fd containing unpublished frames, or (b) attach replayed history to the correct VA surfaces/FIFO so a later STOP on the rebuilt session can publish surface 0x40000003+ instead of dropping replay CAPTURE as no-surface. Also remove temporary debug spam (`replay history record`, QBUF failure detail) after the final design is chosen.
- Phase 2 CPU-copy compatibility (codex agent, 2026-09-19 15:47 +0700): NOT COMPLETE. Required exit remains `tools/verify-rust-driver.sh` + `tools/verify-session-churn.sh` green. Current findings supersede older STOP/replay optimism: native `h264_v4l2m2m` ioctl trace sends `V4L2_DEC_CMD_START` immediately after same-dims `SOURCE_CHANGE`/`G_FMT` and ignores EBUSY, then keeps feeding OUTPUT; the Rust driver now mirrors the event-time START and keeps CAPTURE streaming. Clean no-STOP path still deadlocks FFmpeg VAAPI-copy because FFmpeg submits exactly 3 AUs then syncs the first surface, while iris withholds CAPTURE until a drain-like command. `V4L2_DEC_CMD_FLUSH` was tested and is ignored/failed. Sync `DECODER_CMD STOP` without replay releases the first frames but then either times out on surface `0x40000003` or enters repeated STOP-failed state; with EOS grace, debug log `/tmp/phase2-syncstop-grace-debug-30.log` shows first STOP publishes ts 0/33333/100000, START resumes, later empty CAPTURE/re-source-change appears, second sync STOP starts with pending=9/out_queued=9, then START happens without frames and repeated STOP attempts fail until FFmpeg times out. Earlier STOP+replay got sample-1 byte-exact but corrupted sample-30/full ordering/pixels. Conclusion: normal midstream STOP is not production-safe; a correct fix still needs a frame-preserving way to release the first displayable frames or an architecture change that lets feeding continue while vaSyncSurface waits. Artifacts: `/tmp/libva-v4l2-rust-driver-phase2-nostop-native-start-20260919`, `/tmp/libva-v4l2-rust-driver-phase2-flush-20260919`, `/tmp/libva-v4l2-rust-driver-phase2-syncstop-grace-20260919`, logs under `/tmp/phase2-*`.
- Phase 3 audit — OUTPUT-consume wedge in the export lane (claude agent,
  2026-09-19): ROOT-CAUSED from `/tmp/libva-v4l2-gl-roundtrip/gst-gl.log`
  (1941 lines). The gst-gl session fires a sync starvation drain every ~30
  frames (gst's GL upload sync pattern starves OUTPUT feeding, unlike
  ffmpeg): 6 drains total (log lines 236, 556, 877, 1198, 1521, 1842), each
  STOP+START+replay. Drains 1-5 recover. On drain #6 iris consumes two
  replay pairs then stops DQ-ing OUTPUT entirely; `replay_reference_history`
  hits its 2500-pump limit and BAILS MID-CHAIN
  (`rust/src/v4l2/submit.rs:56`, "replay output pacing stall", log 1898),
  leaving 2 replay AUs kernel-queued that the decoder will never eat. Every
  later submit joins the stuck queue (`out=3/16` frozen, fifo=1, 30 s of
  vaSyncSurface timeouts), and no recovery can arm because the wedge has NO
  abort signature (`aborted=false recov=0/ok`, CAPTURE stays streaming,
  cap=20/20). Conclusion: iris tolerates only a limited number of mid-stream
  STOP/START cycles per session (~5); the 6th wedges silently. Fix plan
  (NOT yet implemented — canary-gated hardware cycle required, matrix must
  stay green): (a) cap per-session sync drains and escalate to the proven
  full session rebuild after N (2-3); (b) the replay pacing bail must arm
  `aborted`/rebuild instead of returning mid-chain. Long ffmpeg plays are
  luckier (sample-full hit only 2 drains) but share the same latent bug.
- ~~DECODE-PATH OWNERSHIP CHANGE (claude agent, 2026-09-19 ~00:30)~~ RESOLVED
  2026-09-19 ~02:00: source-change/starvation fix implemented, hardware-validated,
  and MERGED into `rust/src` (12 files, trees verified identical to snapshot
  `/tmp/claude-scfix`). Full required matrix + churn + resolution-churn PASS on
  hardware. codex: HOLD LIFTED; see the top entry of **Completed recently** for
  the root-cause chain, evidence paths, and the remaining gst-gl export lane
  attribution before starting new `rust/src` work.
- CPU-copy compatibility (codex agent): the Rust driver now keeps FFmpeg and
  mpv `vaapi-copy` on the established queue-all CAPTURE path. Only a client
  that exports an Empty VA surface before decode switches the session into
  fixed surface-to-CAPTURE-slot mode. That queue-mode API now lives in
  `rust/src/v4l2/capture.rs`, keeping the CPU-copy/export split explicit.
  Host validation is complete. The current Phase 2 fix now gets past the
  original same-dimension source-change wedge far enough to publish the first
  frame byte-exactly (`sample-1` MD5 matches native), but FFmpeg still exits
  nonzero because a midstream STOP/START drain used to flush that first frame
  loses later reference continuity. Phase 2 remains open until repeated
  FFmpeg/mpv copy playback exits cleanly without corrupting or timing out
  later surfaces.
- Export importer audit (codex agent): the GL and GStreamer probes now require
  a successful `ExportSurfaceHandle` marker. The archived GL log contains
  callback entries without a successful driver export, so its former
  28-frame claim is callback-only and does not prove zero-copy export.
- Phase 3 BROWSER front: get a snap browser (Firefox/Chromium, aarch64 Wayland)
  to select VAAPI hardware decode through this driver instead of software.
  Selection happens at VAAPI-init/profile-query time (before frame streaming),
  so it is mostly decoupled from the firmware-poison flakiness. Diagnosing WHY
  Firefox picks software FFmpeg and whether the Chromium snap GPU process can be
  coaxed on; improving `tools/verify-browser-vaapi.sh` to report the precise
  fallback reason. Does NOT touch `rust/src` or the GL-importer/export-lifetime
  work. (claude/opus agent, in progress)
- *** BLOCKER REFRAME — the "kernel-silent full-decode stall" is a DRIVER bug in
  the SOURCE_CHANGE handshake, NOT firmware poison (claude/opus agent,
  2026-09-18, intel for codex who now owns `source_change_flush`):***
  PROOF: on a healthy node (native `h264_v4l2m2m` decodes 30/30 byte-exact AND
  deterministic across runs, `journalctl -k` clean), our VAAPI driver
  INTERMITTENTLY fails the SAME 720p sample. When it fails, `V4L2_VA_DEBUG=1`
  shows: `SOURCE_CHANGE 1280x736 -> 1280x736` (same dims), then
  `CAP DQ bytes=0 last=true` (the empty buffer carries `V4L2_BUF_FLAG_LAST` —
  the spec flush marker), then a paired `V4L2_EVENT_EOS`. The old code misread
  BOTH as the small-stream abort -> false recovery -> "recovery limit reached"
  -> `output pacing stall` -> `VA_STATUS_ERROR_DECODING_ERROR`. Native handles
  the same event sequence and keeps decoding; the intermittency is just whether
  the firmware emits the initial same-dims SOURCE_CHANGE.
  WHAT WORKS / WHAT DOESN'T (tested on-device this session):
  1. Suppressing the false abort on LAST + paired EOS (the `source_change_flush`
     flag) is NECESSARY but NOT sufficient: the decoder does not resume on its
     own, so the pump stalls at `output pacing stall` (0 frames). Codex's
     current build reproduces this 0-frame stall on a healthy node.
  2. `DEC_CMD_START` alone does not resume it either.
  3. A full CAPTURE reinit on the flush (`release_queue_fd` + `capture_pool_setup`
     + `stream_on(false)` + `queue_all_capture`) DOES resume — decodes 30/30
     rc=0 — BUT CORRUPTS output: only ~19-21/30 frames match native (no dupes,
     11 genuinely-wrong-pixel frames). The realloc drops in-flight decode /
     disrupts reference state. So a full realloc is the WRONG resume.
  NEXT: the correct resume must be FRAME-PRESERVING — match native's exact
  same-dims SOURCE_CHANGE ioctl sequence (likely: dequeue CAPTURE to the LAST
  marker, then the minimal CAPTURE cycle WITHOUT REQBUFS/realloc, preserving the
  decoder's reference frames). Capture native's ioctl trace with
  `strace -f -e ioctl ffmpeg -c:v h264_v4l2m2m ...` and mirror it. This fix
  gates Phase 1/2/3 device validation AND browser hardware decode. Reproduce
  with: `tools/capture-iris-kernel-log.sh -- env LIBVA_DRIVERS_PATH=... \
  LIBVA_DRIVER_NAME=msm V4L2_VA_DEBUG=1 ffmpeg -hwaccel vaapi -i test_720p.mp4 \
  -frames:v 30 -f framemd5 -` and retry until the SOURCE_CHANGE path fires.
  (My scratch edits to poll.rs/v4l2.rs raced with codex's concurrent
  `source_change_flush` work; the tree currently builds + 89 tests pass with
  codex's version. I have stopped editing the decode path.)
- Phase 5 codec expansion (claude agent, parallel subagent tracks,
  2026-09-18): file-disjoint tracks (A) capability + profile reporting,
  (B) conformance samples + probe, and (C) the HEVC bitstream skeleton are
  all DONE (see Completed recently). The only `rust/src/lib.rs` change from
  (C) is the one-line `mod h265;` registration. Hardware decode validation
  for the new codecs stays deferred until the intermittent full-decode
  stall window clears.
- SOURCE_CHANGE resume DIAGNOSIS (claude agent, 2026-09-18, complementary
  lane to codex's `source_change_flush` ownership): this lane makes NO
  `rust/src` edits. Goal = the diagnosis ROADMAP immediate-next item 3 asks
  for: capture native `h264_v4l2m2m`'s exact ioctl flow across a
  same-dimension SOURCE_CHANGE (`strace -f -e ioctl` on the v4l2 node),
  diff it against the driver's current post-marker sequence (deferred
  CAPTURE dequeue + `DECODER_CMD START` nudge), and answer the open
  question — minimal CAPTURE cycle, drain handshake, or missing non-VCL
  NAL. Deliverable: evidence + concrete resume recipe posted here for
  codex. Bounded single hardware runs only; a wedged node is reported,
  never retried.
  **DIAGNOSIS RESULT (2026-09-18, done OFFLINE from codex's own strace dumps —
  `/tmp/native.strace` full-stream + `/tmp/libva-v4l2-native-sample1-strace-20260918232303.log`
  — plus code reading; ZERO hardware touched by this lane). Cross-checked
  against codex's eosgrace entry below (midstream STOP/START publishes early
  frames then misses reference-dependent surfaces) — the two agree:**
  - NATIVE gets the same-dims SOURCE_CHANGE on EVERY session (both traces
    show DQEVENT right after CAPTURE STREAMON) — it is NOT intermittent for
    native. Native's entire handling: `G_FMT CAP` dims check (we mirror
    this), then ONE `DECODER_CMD` attempt that returns **EBUSY and is
    IGNORED**, then keep feeding OUTPUT + pump CAPTURE immediately. NO
    successful STOP, NO START anywhere in the trace, NO REQBUFS/queue cycle,
    NO capture-dequeue deferral. `FLAG_LAST` + EOS event appear ONLY at true
    EOS after the explicit STOP drain — NEVER mid-stream.
  - OUR failing flow gets the same event PLUS the `FLAG_LAST`+EOS pair: iris
    completed the spec's decoder-initiated drain -> V4L2 "Stopped" state. Per
    dev-decoder.rst the only way out of Stopped is `V4L2_DEC_CMD_START` —
    but STOP-corrupted references (codex's eosgrace evidence) show a
    driver-initiated STOP must NOT be part of this path at all.
  - Code chain that blocks recovery today: (1) the only START sender
    `maybe_resume_after_drain` (submit.rs:24) is gated on
    `self.eos || self.draining` and called only from submit_frame
    (submit.rs:102); (2) the suppression branch for the paired EOS event
    (poll.rs EOS arm) deliberately does NOT set `self.eos` -> START
    unreachable; (3) `maybe_start_source_change_drain` either deadlocks
    (needs `out_queued()==0` + an OUT DQ — a stopped decoder returns neither;
    native's trace confirms OUTPUT stays held after STOP) or, when it does
    fire, sends the true-EOS STOP that corrupts references; (4) the
    CAPTURE-deferral gate blocks pumping any late real frame. Net: AUs
    swallowed, 0 published frames or bad references.
  - RECIPE for codex: (a) NEVER issue STOP for the same-dims source change —
    native's only STOP attempt EBUSY-fails and is ignored, and codex's
    eosgrace run proves a successful midstream STOP loses reference
    surfaces; (b) when the marker pair completes while `source_change_flush`
    is armed (empty LAST CAPTURE seen + paired EOS event), issue
    `V4L2_DEC_CMD_START` right there in poll.rs (idempotent, EBUSY-tolerant)
    and clear the flush state on success — do NOT wait for the next
    submission (sample-1 has none) and do NOT set `self.eos`; (c) drop the
    CAPTURE-deferral gate — pump CAPTURE continuously, requeue empties, like
    native; (d) keep the false-abort suppression. If START-after-markers
    still loses references, next suspect is per-AU header prepending (if we
    synthesize SPS/PPS onto EVERY AU, iris may re-parse mid-stream and
    auto-drain repeatedly; native feeds the original first-AU bytes once) —
    worth an audit of `assemble_access_unit` call sites. This diagnosis lane
    is DONE; `rust/src` follow-ups are codex's.
  **HEADER-PREPEND AUDIT (2026-09-18, same lane, CLEARS that suspect):**
  `submit_frame` does NOT prepend SPS/PPS to submitted AUs — decode.rs:333
  passes `syn.header_bytes()` but submit.rs:56-58 only STORES it for the
  recovery-replay path (recovery.rs:151-154 re-prepends to the first replayed
  chunk of a fresh V4L2 session, by design). The QBUF payload is
  `frame.bytes` alone, and `assemble_frame` emits headers only on the first
  AU and on `frame_num==0` (h264.rs test
  `frame_assembly_emits_headers_once_then_on_frame_num_zero`) — matching
  x264's on-wire structure (SPS/PPS on IDR only). So redundant-header reparse
  is NOT the auto-drain trigger; remaining first-AU delta vs native is just
  the x264 SEI (VA-API cannot deliver SEI for decode — accepted). Concrete
  byte-level check without new instrumentation: set `V4L2_VA_DUMP=<prefix>`
  (decode.rs:301) to dump each submitted AU and cmp against the native first
  AU extracted from the mp4. Net: the START-after-markers recipe stays the
  primary fix; the mid-stream auto-drain itself looks firmware-internal and
  with the recipe becomes transparent.
- Phase 3 CLOSURE ATTEMPT (claude agent, 2026-09-19, delegated by the user
  after claude/opus's "pause and let codex land it" handoff): node observed
  quiet after codex's `phase2-startmarkers-20260919` probe (that probe's rust
  leg produced 0 frames — empty rust.md5 vs native
  `a7ee14ad63331600445567e3572dab54` — so the START-markers build regressed
  vs eosgrace and the decode gate is NOT landed yet). Running ONE bounded
  `tools/verify-browser-vaapi.sh` (chromium, native mode) against a $HOME
  copy of the best-known build `phase2-eosgrace-20260918` (snap sandbox
  cannot see /tmp; a /tmp driver arg would silently rebuild from the live
  tree instead of the staged build). The verifier's pass bar is
  driver-loaded only — decode-level closure will be classified OFFLINE from
  the run log (EndPicture publishes, pacing stalls, DECODING_ERROR markers).
  Single run, no retries; result posted below when done.
  **RESULT — Phase 3 stays OPEN (2026-09-19 00:16, log
  `/home/mq/libva-v4l2-browser-eosgrace/run-1789751753-320690/chromium.log`):**
  verifier said `reached_driver` (its pass bar stops there), but the decode
  level shows: Chromium's `VaapiVideoDecoder` SELECTED (h264 main 1280x720),
  our driver negotiated OUTPUT H264 1280x720 / CAPTURE NV12 1280x736, frame
  pool init "up to 22 VideoFrames", then the FIRST `vaSyncSurface` failed
  `VA error: internal decoding error` ~7 ms after frame-pool init and
  `~VaapiVideoDecoder()` tore down. ZERO `EndPicture` publishes. The video
  element kept playing (timeline 0.4→9.4 s + loop) on a NON-hardware
  fallback. KEY NEW EVIDENCE for codex: in this BROWSER run the stall fired
  at frame 1 with NO `SOURCE_CHANGE` debug marker at all — Chromium uses the
  EXPORT session mode (VaapiVideoDecodeLinuxGL dmabuf import, 22-frame pool),
  which per this file switches the session into fixed surface-to-CAPTURE-slot
  mode; so the first-frame `DECODING_ERROR` there is the export-mode variant
  of the stall, not the same-dims marker path. The ffmpeg-side fix alone may
  not clear the browser leg — the export-mode session needs the same audit
  (does IT get markers? is CAPTURE-slot mode wedging before decode?). Phase 3
  closure therefore needs BOTH: the ffmpeg copy-path fix AND a browser-path
  decode audit. No further runs this window (single-run discipline; node left
  healthy — post-run state clean, browser exited on the 20 s bound).

## Last verified clean baseline

- `cargo test --manifest-path rust/Cargo.toml`: 29 passed.
- `./tools/verify-rust-driver.sh /tmp/libva-v4l2-rust-driver`: passed
  required 720p matrix (sample-1, sample-30, sample-full), passed
  `gst_export_probe=reached_driver`, ffmpeg hwmap export probe remained blocked
  before driver, `one-frame-eos` was skipped when native produced no frames,
  and `bframes-240p` remained an expected xfail in the latest run.
- `./tools/verify-session-churn.sh /tmp/libva-v4l2-rust-driver`:
  pass=7 fail=0.
- `tools/verify-resolution-churn.sh` passes one decoder through a repeated
  720x480 to 1280x720 to 720x480 to 1280x720 GStreamer probe and observes at
  least four `SOURCE_CHANGE` events; it is now part of the main verifier after
  the required matrix and export checks.

## Completed recently

- Independent verification of the decode fix + native ioctl trace (claude/opus
  agent, 2026-09-19): confirmed codex's fix on-device — our driver decodes
  test_720p 5/5 runs at 30 frames, all BYTE-EXACT vs freshly generated native
  refs, on a healthy node (native deterministic). Captured native's
  `strace -f -e ioctl` across the same-dims SOURCE_CHANGE, which validates the
  approach: native does NOT STREAMOFF/REQBUFS/realloc CAPTURE on the source
  change (a realloc corrupts ~11/30 frames — I tested it), it keeps up to ~19
  OUTPUT buffers in flight and DQBUFs empty CAPTURE buffers (bytesused=0) as
  normal, matching codex's `output_inflight_limit(source_change_flush)=16` +
  flush-suppression. Trace saved at `/tmp/native_strace.txt`.
  REMAINING for Phase 3 browser (for codex): snap Chromium native-mode still
  hits `vaSyncSurface: internal decoding error` — but it fails FAST (~35ms,
  before the 500ms starvation drain can fire), at Chromium's initial
  `ApplyResolutionChange` resolution-detect decode, with driver debug showing
  only OUTPUT/CAPTURE fmt (no REQBUFS/BeginPicture). Needs a check of whether
  the starvation drain is armed on the very first submitted frame in the
  allocate-mode/stable-capture path Chromium uses. Reproduce:
  `V4L2_VA_BROWSER=chromium V4L2_VA_BROWSER_CHROMIUM_MODE=native
  tools/verify-browser-vaapi.sh <driver>`.
- ***Source-change/starvation decode fix LANDED (claude agent, 2026-09-19,
  resolves the takeover + ROADMAP item 3's "STOP drain loses reference
  continuity"): the required H.264 framemd5 matrix is byte-exact vs native on
  hardware for the first time — sample-1, sample-30, and sample-full all
  rc=0 with `cmp` equal to freshly generated native refs, including a
  full-length run that exercised two starvation drains and recovered
  cleanly.***
  Root causes and fixes (all in the merged `rust/src`):
  1. vaSyncSurface input-starvation deadlock: iris defers releasing decoded
     frame N until AU N+1 is consumed, but a VA-API client blocks in
     vaSyncSurface instead of feeding one — deadlocked by construction
     (native v4l2m2m escapes only because its feeding thread runs ahead).
     Fix: bounded starvation drain in `rust/src/v4l2/poll.rs`
     (`starvation_check`, 100 no-progress pumps ≈ 500 ms) fires the spec
     DECODER_CMD STOP flush; pending frames publish by timestamp
     (`maybe_start_sync_drain` in `rust/src/v4l2/submit.rs`, now returning
     bool and wired into the pump).
  2. Iris drops its H.264 reference chain across STOP: bare P-AUs after a
     midstream START decode to nothing (empty-CAPTURE abort). Fix: the next
     submission after a sync drain replays SPS/PPS + keyframe history
     (`replay_after_drain`); replayed frames whose surface was already
     published drop harmlessly by timestamp.
  3. CAPTURE-slot aliasing on late reads: CAPTURE buffers are requeued at
     DQBUF and recycled immediately, but vaGetImage/vaDeriveImage read the
     slot's live mmap at call time, so published-but-unread surfaces showed
     later frames (observed as reordering + duplication at frame 3+ of the
     30-frame run). Fix: pixels are snapshotted at dequeue time
     (`SurfaceFrame` in `rust/src/state.rs`, stored on the surface at
     publish in `rust/src/sync.rs`); both image read paths
     (`rust/src/image.rs`) consume the snapshot.
  Validation (canary gate before every hardware run; no retries on wedge):
  staging .so `/tmp/libva-v4l2-rust-driver-claude-scfix`; md5s + V4L2_VA_DEBUG
  logs `/tmp/claude-scfix-verify/scfix4-{1,30,full}.md5/.log` vs
  `/tmp/libva-v4l2-verify/native-sample-*.md5`; `tools/verify-session-churn.sh`
  pass=7 fail=0; `tools/verify-resolution-churn.sh` passed (6 SOURCE_CHANGEs);
  one-frame-eos/bframes-240p skip on native-produces-no-frames (known xfail
  lane) and the ffmpeg hwmap probe stays blocked_before_driver (drm-derive
  fails before any driver call — pre-existing). Offline on the merged live
  tree: `cargo fmt --check` clean, 91 tests pass, clippy 0; merged build
  `/tmp/libva-v4l2-rust-driver-scfix-merged` sample-1 rc=0 cmp-equal.
  Attribution notes for follow-up agents: (a) the earlier "native-start
  build = 0 frames" scare was same-session device poison from back-to-back
  probes, not code — hence the now-mandatory canary gate; (b) the gst-gl leg
  of `tools/verify-gl-roundtrip.sh` still fails, but in the PRE-EXISTING
  export lane: ~180 frames publish fine, then iris stops consuming OUTPUT
  with 3/16 kernel-queued (`out=3/16 streaming=true` stall; the starvation
  drain correctly does not fire because out_queued != 0) — this fix touches
  only the CPU read path. Control run on codex's live-baseline build fails
  EARLIER (reference decode rc=251), so the merged tree strictly dominates.
  Phase 2 CPU-copy may now be closed pending repeat mpv/gst-copy runs; the
  Phase 3 export wedge is the next distinct blocker.

- Phase 2 native-handshake narrowing (codex agent, 2026-09-18): matched more of native `h264_v4l2m2m`'s ioctl behavior and proved the remaining failure is queue/drain semantics, not H.264 synthesis. Native strace showed OUTPUT is queued/streamed before CAPTURE allocation/STREAMON and that native's post-`SOURCE_CHANGE` `VIDIOC_DECODER_CMD` returns `EBUSY`; the Rust driver now lazily brings up CAPTURE after the first OUTPUT QBUF, normalizes VA POC timestamps to the first POC (`0`, `100000`, `33333`, matching native), removes source-change START nudges, defers CAPTURE marker dequeue until OUTPUT progress, and suppresses stale source-change drain EOS/empty markers. Host checks pass: `cargo test` (92 tests), strict Clippy, `bash -n tools/*.sh`, `python3 -m py_compile tools/*.py`, and release builds. Hardware evidence: `/tmp/libva-v4l2-rust-driver-phase2-eosgrace-20260918` writes a `sample-1` framemd5 that `cmp`s equal to native, but FFmpeg still exits 251 after decoding ahead and timing out on surface `0x40000003`; debug log `/tmp/libva-v4l2-phase2-eosgrace-debug-sample1.log` shows midstream STOP/START publishes early frames then misses later reference-dependent surfaces. Do not mark Phase 2 complete; next fix must avoid using STOP as a normal midstream resume, or rebuild/replay in a way that preserves H.264 references.

- Phase 2 source-change/SPS narrowing (codex agent, 2026-09-18): fixed the
  false-abort half of the same-dimension `SOURCE_CHANGE` path and narrowed the
  remaining short-decode stall. The V4L2 session now tracks a
  `source_change_flush` marker so the empty CAPTURE marker and paired EOS are
  not mistaken for firmware aborts; OUTPUT pacing temporarily expands to the
  full OUTPUT queue while that marker is active; and source-change submissions
  send a `DECODER_CMD START` resume nudge after queuing OUTPUT. H.264 SPS
  synthesis now derives size-appropriate levels, sets Main's constraint flag,
  and writes VUI bytes matching the 720p sample's original SPS exactly
  (`674d401feca02802dd8088000003000800000301e078c18cb0`; the remaining
  first-AU difference is the original x264 user-data SEI, which VA decode does
  not provide). Host validation: `cargo fmt`, 90 Rust tests, strict Clippy,
  `bash -n tools/*.sh`, `python3 -m py_compile tools/*.py`, and release build
  all pass. Hardware: staged `/tmp/libva-v4l2-rust-driver-phase2-fix-20260918`;
  `tools/verify-rust-driver.sh` still fails required `sample-1` with
  `vaSyncSurface` timeout after three OUTPUT QBUFs and the same-dims
  source-change marker (`out=3/16 cap=32/32 pending(fifo=3,ready=0)`, no
  abort armed). `tools/verify-session-churn.sh` improved from the prior
  `pass=1 fail=6` to `pass=4 fail=3`: reference full decode and all three
  mpv-cut followed by GStreamer legs pass; the remaining failures are the three
  forced SIGKILL/SIGTERM parity legs producing no output. Logs:
  `/tmp/libva-v4l2-phase2-fix-verify-nodebug.log`,
  `/tmp/libva-v4l2-phase2-fix-verify6.log`, and
  `/tmp/libva-v4l2-phase2-fix-churn.log`.

- Phase 5 track C — HEVC (H.265) bitstream plumbing skeleton (claude
  subagent, 2026-09-18): new `rust/src/h265.rs` +
  `rust/src/h265/bitstream.rs`, parsing/assembly ONLY (no VA callback, no
  V4L2 wiring). Covers NAL unit header parse + classification (VPS 32 /
  SPS 33 / PPS 34 / IDR_W_RADL 19 / IDR_N_LP 20 / TRAIL_R 1 / TRAIL_N 0 /
  CRA 21; unknown types pass through as `Other(t)`; set
  `forbidden_zero_bit` and `nuh_temporal_id_plus1==0` rejected with clean
  errors), full `profile_tier_level()` incl. the per-sub-layer present-flag
  loops, minimal-but-real SPS (chroma_format_idc, picture size, bit depths,
  poc log2, the sub-layer-ordering loop bounds, max_dec_pic_buffering /
  max_num_reorder_pics arrays; stops after the SAO flag), minimal VPS
  (IDs, PTL, ordering info, layer sets), minimal PPS (IDs, init_qp_minus26,
  slice/tiles/sign_data_hiding flags; stops after the tile block),
  EBSP->RBSP and RBSP->EBSP (`00 00 03` insertion) helpers, Annex-B
  assembly of `[VPS?][SPS][PPS][slice]` with 3-byte start codes and role
  validation (escaping preserved byte-for-byte), and an Annex-B splitter
  used by the round-trip tests. Bit writer/reader/EBSP primitives are
  deliberately duplicated from `h264::bitstream` in a commented block:
  those items are `pub(super)` to `h264` and `h264.rs` is outside this
  track's ownership. 19 new tests (13 in h265.rs, 6 in h265/bitstream.rs);
  `cargo test` = 87 passed on the concurrent tree. Files added:
  `rust/src/h265.rs`, `rust/src/h265/bitstream.rs`; the ONLY lib.rs change
  is the one-line registration `mod h265;`. Validated: `cargo fmt --check`,
  `cargo test --manifest-path rust/Cargo.toml`,
  `cargo clippy --all-targets -- -D warnings`. No hardware interaction;
  decode validation stays deferred per track rules.
- Phase 5 track A — codec capability + VA profile reporting (claude agent,
  2026-09-18): `/dev/video16` OUTPUT enumerates H264, HEVC, VP90, AV01
  (read-only v4l2-ctl + python fcntl ENUM_FMT only; the HEVC fourcc is
  'HEVC', not 'H265'), so vainfo now advertises H264 x3 + HEVCMain,
  HEVCMain10, VP9Profile0, AV1Profile0 (all VAEntrypointVLD), gated per codec
  on the live ENUM_FMT result. Gating lives in `rust/src/config.rs`
  (`advertised_profiles()`, fed by a new read-only
  `rust/src/v4l2.rs::enumerate_output_fourccs` probe, cached once per
  process); VP9Profile2 deliberately out of scope (no 10-bit render targets
  yet), and any config/entrypoint request for an un-advertised codec still
  fails `VA_STATUS_ERROR_UNSUPPORTED_PROFILE`. Pointing `V4L2_VA_DEVICE` at
  a node without coded formats falls back to the historical H264-only table
  (verified via vainfo). Files: `rust/src/config.rs` (gating + 3 tests),
  `rust/src/v4l2/abi.rs` (HEVC/VP9/AV1 fourcc constants),
  `rust/src/v4l2.rs` (probe + fourcc re-export), `rust/src/lib.rs`
  (one-line `max_profiles` wiring to the gated table — allowed lib.rs
  exception). Validation: `cargo fmt --check`, 68 tests,
  `cargo clippy --all-targets -- -D warnings`, and vainfo on fresh staging
  `/tmp/libva-v4l2-rust-driver-codec5` (both init symbols at one address).
  No hardware decode or STREAMON was performed.
- Phase 5 track B — codec-expansion samples + probe (claude agent,
  2026-09-18): all four host encoders present (libx265, libvpx-vp9,
  libsvtav1, libaom-av1); generated three 10 s 1280x720 300-frame transcodes
  of `test_720p.mp4` in `/home/mq/tmp/vaatest/codec5/` (hevc-main-720p.mp4
  Main crf28 ultrafast, vp9-720p.webm Profile 0 crf32 realtime, av1-720p.mp4
  Main crf30 via libsvtav1), all ffprobe-verified, exact command lines and
  facts in `/home/mq/tmp/vaatest/codec5/MANIFEST.txt` (note: this ffmpeg's
  native `av1` decoder wrapper refuses CPU decode without a hwaccel; the file
  is fine — libdav1d decodes it, 30-frame framemd5 extracted). Added
  `tools/verify-codec-expansion.sh`: per-codec skip-77
  (`missing_sample` / `profile_not_advertised`), 1-frame self-reference +
  N-frame (`V4L2_VA_CODEC_FRAMES`, default 30) framemd5 legs through the
  driver under `capture-iris-kernel-log.sh` with `V4L2_VA_DEBUG=1`,
  eos-drain-style kernel classification (fail system-fatal>0, degraded
  session-fatal>0), no retries; final `codec_expansion=pass|fail` line,
  exit 77 when nothing was verified. Offline validation only (no hardware
  decode): `bash -n` clean; `V4L2_VA_CODEC5_DIR=/nonexistent` → three
  `missing_sample` skips, exit 77, 0 s; real codec5 dir +
  `/nonexistent-dir` driver → three `profile_not_advertised` skips
  (vainfo status 3), exit 77, 0 s; offline pattern/logic checks pass
  (Main10 no false-match, framemd5 frame-1 parity detection,
  kernel_counts NA fallback). One pre-hardware bug caught by validation:
  per-codec skip returns needed `|| rc=$?` under the probe's set -e.
  No `rust/src` changes; source sample untouched.

- Phase 3 BROWSER — Chromium now drives the driver (claude/opus agent,
  2026-09-18): FIRST time a browser reaches this driver through the full VAAPI
  decode path. Two fixes:
  1. GPU-process launch: the browser verifier's forced
     `--use-gl=egl-angle --use-angle=opengles` was itself killing the snap
     Chromium GPU process (`gl=none` -> `Exiting GPU process`). New
     `V4L2_VA_BROWSER_CHROMIUM_MODE=native` passes NO forced GL flags, so
     Chromium keeps its working default path and the GPU process survives.
  2. Driver surface-attribute fix (`rust/src/surface.rs`,
     `validate_surface_creation_attributes`): Chromium's `VaapiVideoDecoder`
     calls `vaCreateSurfaces` (allocate mode) with a SETTABLE
     `VASurfaceAttribUsageHint` (= DECODER). We rejected it with
     `ATTRIBUTE_NOT_SUPPORTED`, forcing software decode. Now accepted+ignored
     (advisory hint, as iHD/gallium do); unknown settable attributes still
     rejected and now logged under `V4L2_VA_DEBUG`.
  Result: `browser_vaapi_probe=reached_driver`; Chromium creates the decoder,
  our driver negotiates OUTPUT=H264/CAPTURE=NV12 and `REQBUFS count=32`, and the
  pipeline runs to `vaSyncSurface`, which returns `internal decoding error`
  (`VA_STATUS_ERROR_DECODING_ERROR`) — the SAME decode-failure class every
  client (FFmpeg/mpv/GStreamer sample-full) hits on the currently-poisoned node,
  not a browser-compat bug. Validated: 65 Rust tests, `cargo fmt --check`,
  strict Clippy, and two live snap-Chromium probe runs. Next: confirm end-to-end
  browser decode on a CLEAN node window; Chromium's allocate-mode surface/decode
  flow may still need driver work distinct from the FFmpeg path.
  Tooling: `tools/verify-browser-vaapi.sh` gained `native`/`vulkan` modes and
  precise `hw_decode_gate_off`/`gpu_gl_init_failed` classification;
  `docs/09-browser-vaapi.md` documents it. (Note: this scoped `rust/src/surface.rs`
  fix is the one exception to the browser task's "no rust/src" boundary; it is
  additive and isolated to attribute validation.)
- EOS/teardown drain regression probe (claude agent, 2026-09-18): added
  `tools/verify-eos-drain.sh` for the ROADMAP item "cover normal EOS and
  teardown drain behavior with a dedicated regression sample". Legs: (A)
  natural-EOS full decode whose driver log must contain NO recovery-armed
  markers (`anomalous EOS without drain` / `empty CAPTURE without drain`);
  (B) mpv mid-stream cut (`--frames=45`) asserting the bounded teardown
  drain; (C) the cross-session assertion — the IMMEDIATELY following full
  decode must match leg A byte-for-byte, the next-session CAPTURE STREAMON
  EIO wedge the drain was built to prevent. Kernel-log wrapper per leg, a
  pre/post single-frame sanity bracket, `bash -n` + both skip paths
  validated offline. Single bounded hardware run against
  `/tmp/libva-v4l2-rust-driver-syncdbg`: the TEARDOWN DRAIN IS PROVEN —
  `DECODER_CMD STOP drain started (pending=3)` → `teardown flush done
  pending=0 out_queued=0 eos=false`, kernel clean, mpv exit 0. The EOS and
  parity legs are BLOCKED by the pre-existing intermittent full-decode
  stall, now characterized as the kernel-silent variant (see Known
  blockers) — a drain-behavior-independent failure. The probe is ready
  unchanged; `V4L2_VA_SAMPLE=/home/mq/tmp/vaatest/one-frame.mp4` gives a
  true-EOS low-stress variant for the next healthy window. Drain-then-
  resubmit remains covered by the seek storm (submit.rs resets `draining`).
  Logs: `/tmp/libva-v4l2-eos-drain/`.

- Buffer metadata/handle callback split (codex agent, 2026-09-18): moved
  `vaBufferInfo`, `vaAcquireBufferHandle`, `vaReleaseBufferHandle`, and
  `vaSyncBuffer` into `rust/src/buffer/handles.rs`, with direct regressions for
  CPU-owned buffer metadata, unsupported external-handle acquisition, and
  release/sync handle validation. The parent `buffer.rs` is now 404 lines and
  focuses on allocation, resize, map/unmap, and image-backing protection. Host
  validation: `cargo fmt --check`, 65 Rust tests, strict Clippy,
  `bash -n tools/*.sh`, `python3 -m py_compile tools/*.py`, and release build
  all pass. Hardware decode was not rerun for this host-only buffer ABI split;
  the latest current51 matrix/churn blocker below still applies.

- Vtable unsupported callback split (codex agent, 2026-09-18): moved the
  exact-signature unsupported core/VPP rejection callbacks into
  `rust/src/vtable/unsupported.rs`. The parent `vtable.rs` is now 272 lines
  and focuses on driver termination plus callback installation. Host
  validation: `cargo fmt`, 62 Rust tests including vtable installation and
  CPU/display rejection checks, strict Clippy, `bash -n tools/*.sh`,
  `python3 -m py_compile tools/*.py`, and release build all pass. Hardware
  decode was not rerun for this pure vtable refactor; the latest current51
  matrix/churn blocker below still applies.

- Image layout test relocation and derive guard (codex agent, 2026-09-18):
  moved pure NV12 layout/copy/rectangle tests into `rust/src/image/layout.rs`
  beside the code they cover, and added a direct `vaDeriveImage` null-output
  regression before it can sync or touch surface state. The parent `image.rs`
  is now 372 lines. Host validation: `cargo fmt --check`, 62 Rust tests,
  strict Clippy, `bash -n tools/*.sh`, `python3 -m py_compile tools/*.py`,
  and release build all pass. Hardware decode was not rerun for this host-only
  image lifecycle/coverage change; the latest current51 matrix/churn blocker
  below still applies.

- H.264 bitstream helper split (codex agent, 2026-09-18): moved the pure
  `BitWriter`, Exp-Golomb, RBSP-to-EBSP escaping, and NAL wrapping helpers into
  `rust/src/h264/bitstream.rs`. The parent `h264.rs` is now 483 lines and
  remains focused on SPS/PPS synthesis policy plus frame assembly. Host
  validation: `cargo fmt`, 61 Rust tests including the golden SPS/PPS byte
  tests, strict Clippy, `bash -n tools/*.sh`,
  `python3 -m py_compile tools/*.py`, and release build all pass. Hardware
  decode was not rerun for this pure refactor; the latest current51
  matrix/churn blocker below still applies.

- Surface status/error coverage and module split (codex agent, 2026-09-18):
  moved `vaQuerySurfaceStatus` / `vaQuerySurfaceError` into
  `rust/src/surface/status.rs` and added direct callback regressions for
  Ready vs Rendering status plus Dead-surface decode-error reporting. The
  parent `surface.rs` is now 400 lines. Host validation: `cargo fmt --check`,
  61 Rust tests, strict Clippy, `bash -n tools/*.sh`,
  `python3 -m py_compile tools/*.py`, and release build all pass. Hardware
  decode was not rerun for this host-only status/error split; the latest
  current51 matrix/churn blocker below still applies.

- Seek-storm probe, lifecycle/reconfig item (claude agent, 2026-09-18):
  `tools/verify-seek-storm.sh` + `tools/mpv_seek_drive.py` drive REAL seeks
  through mpv's JSON IPC (`--hwdec=vaapi-copy --loop=inf`): phase 720p = 24
  absolute keyframe seeks, phase mixed = 12 seeks over a 720x480+1280x720
  mpegts concat so seeks cross the resolution boundary. Each phase runs under
  `tools/capture-iris-kernel-log.sh` (journal cursor delta), and the storm is
  bracketed by pre/post single-frame framemd5 sanity decodes through the
  driver; the probe never retries a wedged node. VA-API has no flush callback,
  so these sync/drop/resubmit cycles are what a seek actually looks like to
  the driver (`draining` resets on new submissions, `rust/src/v4l2/submit.rs`).
  Offline validation caught three probe bugs pre-hardware: mpv 0.41 rejects
  the bare-string IPC command form AND the singular `absolute+keyframe` flag
  (array form + `absolute+keyframes` required), and `grep -q` under
  `set -o pipefail` SIGPIPEs the large `mpv --list-options` producer, making
  the preflight falsely skip. Single bounded hardware run (staging
  `/tmp/libva-v4l2-rust-driver-syncdbg`): `720p=ok` (24 seeks, kernel
  session=0 system=0), `mixed=degraded reason=session_abort_rescued` (12
  seeks; FIVE session-fatal 0x4000003 aborts inside one ~750 us burst at the
  resolution-crossing teardown, 0 system-fatal, 0 power-cycles; mpv exited 0
  and playback survived), `seek_storm=pass sanity=ok`, post-storm framemd5
  byte-identical to pre-storm. Verdict vs ROADMAP: seek storms do NOT wedge
  or deadlock the node and do not trigger the system-fatal poison; the open
  residual is the per-session abort burst on seek-driven reconfiguration,
  which the driver's recovery rescues (same small-stream/bframes signature).
  Logs: `/tmp/libva-v4l2-seek/{720p,mixed}{,-mpv}.log`.

- V4L2 CAPTURE mode module split (codex agent, 2026-09-18): moved the
  CPU-copy queue-all vs pre-decode PRIME reservation logic from the parent
  session file into `rust/src/v4l2/capture.rs`. The parent `v4l2.rs` is down
  to 343 lines, and the reservation regression now lives beside the queue-mode
  API it covers. Host validation: `cargo fmt`, 59 Rust tests, strict Clippy,
  `bash -n tools/*.sh`, `python3 -m py_compile tools/*.py`, and release build
  all pass. Required hardware validation with
  `/tmp/libva-v4l2-rust-driver-current51-20260918` still fails at `sample-1`
  with VA status 23 / FFmpeg EIO, before sample-30 or sample-full. Mandatory
  churn still fails its reference leg with no output, so repeated playback
  legs cannot run while `/dev/video16` is in the documented poisoned state.

- Phase 3 BROWSER diagnosis (claude/opus agent, 2026-09-18): pinned WHY no
  browser reaches this driver. Both blockers are snap confinement, NOT the
  driver — proven because the whole non-browser stack works on this host:
  `vainfo` loads the driver, Vulkan = Adreno X1-85 turnip 1.4.311, GL/EGL =
  freedreno Adreno OpenGL 4.6 / GLES 3.2 Mesa 25.1.4, and FFmpeg/mpv/GStreamer
  decode 720p byte-exact.
  - Firefox (snap): RDD decoder order is `FFmpeg(FFVPX)/FFmpeg(OS)/Agnostic`
    with NO VA-API module; `IsHardwareAccelerated=0`, zero `vaapi` log lines,
    `msm_drv_video_rs` never loaded. The `CanUseHardwareVideoDecoding` gate is
    off and `media.hardware-video-decoding.force-enabled=true` does not override
    it. Classified `hw_decode_gate_off`.
  - Chromium (snap): GPU process dies at `Requested GL implementation
    (gl=none,angle=none) not found in allowed implementations: [egl-angle...]`
    -> `Exiting GPU process`. No GPU process => `VaapiVideoDecoder` can never
    run. Classified `gpu_gl_init_failed`. The host GL/Vulkan both work, so this
    is confinement (and possibly our own forced `--use-gl` flags killing it).
  - The real unblock is an UNCONFINED (non-snap) browser; on aarch64 the
    practical one is Mozilla's official Firefox aarch64 tarball (no ARM64 Chrome
    build exists). Needs a user install decision.
  - Deliverables (no `rust/src`, no hardware decode run): improved
    `tools/verify-browser-vaapi.sh` with `native`/`vulkan` Chromium modes and
    precise `hw_decode_gate_off` / `gpu_gl_init_failed` classification, plus
    `docs/09-browser-vaapi.md`. Validated `bash -n`; findings come from the
    existing captured browser logs (no new browser launch).

- CPU-copy queue compatibility boundary (codex agent, 2026-09-18): normal
  sessions retain queue-all CAPTURE behavior and timestamp matching; only
  pre-decode PRIME exports reserve and bind individual CAPTURE slots. Exported
  surface backing is preserved across publication, and a reservation mode
  regression covers the transition and release. Host validation: 59 Rust
  tests, formatting, and strict Clippy pass. Build
  `/tmp/libva-v4l2-rust-driver-current50-20260918` was produced, but the
  required matrix failed at sample-1 with VA status 23 / FFmpeg EIO, and the
  mandated churn suite failed its reference leg with no output because the
  known `/dev/video16` firmware poison is still active.

- Bounded VA input lists (codex agent, 2026-09-18): surface creation and
  destruction reject counts above the fixed surface table before forming FFI
  slices, and config/surface creation reject attribute lists above 64 entries.
  Added an excessive surface-count regression. Host-independent validation:
  57 Rust tests, formatting, strict Clippy, release build, and shell syntax
  checks pass. Required current47 hardware validation reached sample-1 and
  sample-30 successfully; sample-full and churn remain blocked by the known
  `/dev/video16` firmware wedge.

- Bounded PRIME export retention (codex agent, 2026-09-18):
  `vaExportSurfaceHandle` now rejects a 65th tracked duplicate on one live
  surface with `VA_STATUS_ERROR_MAX_NUM_EXCEEDED`; the guard prevents
  driver-owned fd bookkeeping from growing without bound when the VA API does
  not notify the driver when a client closes an exported fd. The 56-test suite,
  strict Clippy, formatting, release build, required sample-1/sample-30
  checks, and the mandated churn run were executed. The current node still
  fails sample-full with status 38 and churn with `pass=1 fail=6` as recorded
  in Known blockers.

- VA decode-boundary hardening (codex agent, 2026-09-18):
  - `vaBeginPicture` now checks for an already-open picture before retiring
    the target's CAPTURE slot or tracked export state, so a rejected nested
    begin is side-effect free.
  - `vaRenderPicture` rejects more than 256 client-supplied buffer IDs before
    allocation and uses checked arithmetic for slice parameter/data ranges.
  - Added regressions for nested begin preservation and oversized render lists.
  - Host-independent validation: 55 Rust tests, `cargo fmt --check`, strict
    Clippy, release build, and shell syntax checks pass.
- Required hardware verification for the preceding surface-retirement fix
  (`/tmp/libva-v4l2-rust-driver-current44`, 2026-09-18): unit tests, sample-1,
  and sample-30 passed; sample-full hit the known `/dev/video16`
  `vaSyncSurface` status 38 / FFmpeg EIO failure. Session churn completed the
  reference leg but failed the three mpv-cut GStreamer legs and all three
  forced-kill/SIGTERM parity legs (`pass=1 fail=6`) while the node remained
  poisoned. This does not invalidate the host-independent regression.

- GL-importer NV12 layout validation (claude agent, 2026-09-18, closes the
  ROADMAP "verify NV12 planes/offsets/strides with an importer" item for
  planes/offsets/strides):
  - Added `tools/gst_gl_roundtrip.py` (rewritten gi-free) and
    `tools/verify-gl-roundtrip.sh`. The original PyGObject appsink design was
    abandoned: the installed python3-gst bindings crash with heap corruption
    inside `GstVideo` boxed types (even `GstVideo.VideoInfo()` aborts). The
    new design dumps both views of the decoded stream to raw I420 files and
    compares them stride-aware: reference = ffmpeg vaapi decode + CPU copy
    (`-f rawvideo`, alignment 1, stride == width) through the same Rust
    driver; GL path = `gst-launch vah264dec ! glupload ! gldownload !
    videoconvert ! video/x-raw,format=I420 ! filesink`. Strides are derived
    from the file sizes (bytes = frames * stride * height * 3 / 2) with
    ambiguous candidates disambiguated by matching frame 0 hashes across the
    two files; `--stride/--ref-stride` overrides exist.
  - Offline validation: synthetic padded-stride (68) vs tight (64) dumps
    with an ambiguous reference candidate set pass; a single flipped V-plane
    pixel in frame 2 is detected; exit codes are 0/1. `bash -n`,
    `py_compile` clean.
  - Single bounded hardware run against
    `/tmp/libva-v4l2-rust-driver-syncdbg`: the GL path downloaded 28 frames
    and EVERY one is byte-identical to the CPU-copy reference
    (`gl_roundtrip=pass frames=28 gl_stride=1280 ref_stride=1280`; hashes
    archived in `/tmp/libva-v4l2-gl-roundtrip/layout-compare.txt`). The
    exported descriptor's plane offsets, strides, and sizes are correct as
    sampled through EGL import + GL download. Modifiers are unexercised
    (linear DRM PRIME, no modifier attribute).
    CORRECTION (codex audit, see Active task): the archived log has export
    callback entries but NO `ExportSurfaceHandle succeeded` marker, so the
    28-frame result proves the fallback (CPU) path layout only; re-run with a
    driver build carrying the success marker before claiming zero-copy
    descriptor correctness.
  - The pipeline itself aborted at ~frame 29 ("Failed to upload buffer" from
    glupload, then decoder stall and sync timeouts) — recorded as a precise
    repro for the export-lifetime work in Active task / blockers; NOT a
    layout problem. Node state around the run: the earlier background
    verifier run on this tree PASSED the full required matrix (sample-1/30/
    full), gst export probe, resolution probe, and churn 7/0 (node had
    recovered), a later run re-hit the sample-full error 38, and this probe
    added its own error-stop teardown. No further hardware attempts this
    session per the no-loop rule; raw dumps deleted, logs kept.
- Unprivileged Iris firmware-error tracing (claude/opus agent, 2026-09-18):
  - The `bframes-240p` and cross-session "poison" failures were treated as
    needing root `dmesg`/venus HFI traces. They are partly observable WITHOUT
    root: `dmesg` is blocked (`kernel.dmesg_restrict=1`) but `journalctl -k`
    reads the same kernel ring from the journal. HFI-level detail is still
    root-gated (`qcom_iris` has no module params; dynamic_debug/debugfs need
    root), so we can see the firmware error CLASS but not the provoking command.
  - Kernel evidence, driver is `qcom-iris aa00000.video-codec` (NOT venus):
    `session error received 0x4000003: fatal error` = per-session firmware abort
    (recoverable; the small-stream/bframes signature). `received system error of
    type 0x5000003` = device-wide firmware crash -> `video hw is power on`
    reload + ~90s poison, and it trips a WARN at `videobuf2-core.c:1821`
    (`vb2_start_streaming`) because iris fails STREAMON without returning
    buffers to vb2.
  - Client-agnostic confirmation: this boot's two device-wide crashes were
    raised by DIFFERENT clients (`dec0:0:h264_v4l` = native ffmpeg, `queue0:src`
    = GStreamer via this driver). Native decode trips the identical crash, so
    the abort/poison is firmware-side, not a bug in this VAAPI driver. This is
    the kernel-side proof of the existing behavioral hypothesis; it validates
    `MAX_SESSION_RECOVERIES=1` and keeping `bframes-240p` as xfail.
  - Added `tools/capture-iris-kernel-log.sh` (`summary` mode + wrapped-command
    cursor-delta mode) and `docs/08-iris-firmware-errors.md`. No `rust/src`,
    verifier-matrix, or hardware-node changes; validated with `bash -n` and a
    live `summary` run (session-fatal=250, system-fatal crashes=3 this boot).
- Node-recovery probe + recovery-path review (claude agent, 2026-09-18):
  - Review verdict on the `pump` recovery-on-timeout change and the
    sync-timeout `DECODER_CMD STOP` fallback removal: both are sound. The
    silent abort variant can leave the firmware silent after arming, so
    waiting for another poll readiness event would stall recovery forever,
    and the STOP fallback never fired in passing runs while risking dropped
    in-flight work mid-stream. Recovery limit still latches; the
    `in_recover` guard is intact.
  - Node state after the long-hold wedge: `vainfo` loads the current-tree
    build (`/tmp/libva-v4l2-rust-driver-syncdbg`, staged from a green
    29-test tree) and all three H.264 VLD profiles appear, sample-1 and
    sample-30 framemd5 pass, but the full 300-frame decode died with
    `vaSyncSurface` error 38 (OPERATION_FAILED) -> ffmpeg EIO, aborting the
    verifier before its summary. Consistent with the wedge below; no retries
    attempted per the no-loop rule. Re-run the required matrix after a
    longer idle window.
- Teardown leak checks (claude agent, closes the ROADMAP "Add leak checks for
  fds and mmap regions" item): both plane-munmap sites in `rust/src/v4l2.rs`
  now funnel through `release_mapping` with a test-only counter, and
  `teardown_unmaps_planes_and_closes_fd` drops a synthetic session (real
  anonymous-mapped planes in OUTPUT/CAPTURE/legacy pools, /dev/null fd)
  asserting each plane is unmapped exactly once, queue bookkeeping is reset,
  and the session fd is closed. No behavior change on the device path.
  Validated: 29 unit tests, `cargo fmt --check`, `cargo clippy --all-targets
  -- -D warnings`, `./tools/verify-rust-driver.sh
  /tmp/libva-v4l2-rust-driver-syncdbg` (required matrix + gst export probe +
  resolution probe green), and `./tools/verify-session-churn.sh
  /tmp/libva-v4l2-rust-driver-syncdbg` pass=7 fail=0.
- `bframes-240p` gap diagnosis (claude agent, findings below; no fix possible
  without kernel-side evidence):
  - Failure signature under `V4L2_VA_DEBUG=1`: after 1-2 OUTPUT QBUFs the
    firmware fires `SOURCE_CHANGE` (same dims), returns ONE EMPTY CAPTURE
    (bytes=0, ts=0.0), then raises a spontaneous `V4L2_EVENT_EOS` without any
    `DECODER_CMD STOP`; no OUTPUT DQBUF ever happens, so every later submit
    hits the `output pacing stall` and all surfaces die with
    `VA_STATUS_ERROR_DECODING_ERROR`.
  - NOT the synthesized bitstream: x264 Main re-encodes of the same content
    have byte-equivalent SPS/PPS fields (refs=4, CABAC, weighted pred,
    poc_type 0) at all sizes; the 1280x720 re-encode passes 300-frame-class
    parity while the 640x480 re-encode fails.
  - NOT B-frames, level, or the small-picture level pick: B-free
    Constrained-Baseline 320x240 also fails; 640x480 and 1280x480 both
    synthesize level 4.2 and fail.
  - It is a PER-SESSION probabilistic firmware failure correlated with small
    picture size (<=~512 px height fails most sessions, 544-576 flaky,
    >=640 solid; 720p never observed failing), not a deterministic rule: the
    same 640x480 clip passed 6/6 frames and even passed the 15-frame drain
    path once.
  - Ruled out userspace triggers: `V4L2_DEC_CMD_START` on same-dim
    SOURCE_CHANGE (A/B tested), first-submit pacing delay (30 ms tested).
    Native `h264_v4l2m2m` on the same /dev/video16 node decodes the failing
    clips reliably and never sends DECODER_CMD; kernel `dmesg`/HFI logs are
    required to identify the firmware-side condition.
  - Kept additive diagnostics: OUTPUT/CAPTURE negotiated-format debug lines in
    `setup_output`; experiment knobs were reverted.
- Sync-timeout diagnostics: `V4l2Session::debug_snapshot` (pure-formatted
  OUTPUT/CAPTURE queue summary in `rust/src/v4l2.rs`, unit-tested) and a
  debug-gated `vaSyncSurface` timeout log in `rust/src/sync.rs` that reports
  surface state, cap_idx, elapsed/timeout, drain state, and the session
  snapshot. Behavior otherwise unchanged.
- Split NV12 image layout/copy helpers into `rust/src/image.rs`.
- Split CAPTURE-to-surface publication into `rust/src/sync.rs`.
- Split export-state and driver-owned duplicate fd bookkeeping into
  `rust/src/surface_export.rs`.
- Added `tools/verify-gst-export.sh` and wired it into the main verifier as a
  runtime GStreamer importer/export probe.
- Added `tools/verify-resolution-churn.sh`, which keeps one `vah264dec` alive
  while a concatenated stream changes 720x480 to 1280x720 twice, and wired it
  into the main verifier.
- Hardened export fd bookkeeping: `vaExportSurfaceHandle` now fails if the
  driver cannot duplicate the exported dma-buf fd for tracking, closes the
  untracked descriptor on that failure, and retires driver-owned export fds
  before requeueing a CAPTURE buffer on surface reuse/destroy.
- Converted tracked export fds from raw `i32` values to Rust `OwnedFd`, so
  duplicated dma-buf fds are also closed automatically if a surface is dropped
  during teardown.
- Added `tools/verify-browser-vaapi.sh`, which serves a local sample with a fresh
  profile and classifies page-load, GPU-process, software-decoder, and driver-call
  failures.
- Added the legacy `__vaDriverInit_1_0` entry point alongside
  `__vaDriverInit_1_24`; Chromium's bundled libva can now initialize the Rust
  driver. The Chromium snap still disables its GPU process before VAAPI decode
  and its in-process variant crashes; Firefox reaches the page but selects
  software FFmpeg H.264 decoding.
- Moved `query_image_formats`, `create_image`, `destroy_image`, `get_image`, and
  `derive_image` into `rust/src/image.rs` beside the NV12 helpers. The split is
  formatted and validated by the 26-test suite and the full required verifier.
- Moved VA buffer allocation and map/unmap callbacks into
  `rust/src/buffer.rs`; metadata, external-handle, and sync callbacks now live
  in `rust/src/buffer/handles.rs`. The 26-test suite, required frame matrix, GStreamer
  export probe, and session-churn suite all pass after this split.
- Moved H.264 `vaBeginPicture`, `vaRenderPicture`, and `vaEndPicture` into
  `rust/src/decode.rs`; the same 29-test, required-matrix, export, and churn
  verification remains green, and `lib.rs` is now about 1,000 lines.
- Moved surface attribute negotiation, allocation, status, destruction, and
  CAPTURE retirement into `rust/src/surface.rs`, adding focused NV12 and
  VA/DRM-PRIME attribute tests. The 29-test suite, required matrix, export
  probe, and session churn all pass.
- Added the generated-binding Clippy boundary and fixed handwritten lint findings
  in the new modules and V4L2 recovery path. `cargo clippy --all-targets --
  -D warnings` now passes, alongside `cargo fmt --check` and the 29-test suite.
- Release sanity checks pass after the split: both `__vaDriverInit_1_0` and
  `__vaDriverInit_1_24` are exported at the same address, and the Firefox
  browser probe still classifies the installed snap as software-decoder
  fallback rather than reporting a driver failure.
- Added unit coverage for automatic `OwnedFd` cleanup on surface drop and an
  opt-in `V4L2_VA_GST_EXPORT_BUFFERS`/`V4L2_VA_GST_EXPORT_HOLD_MS` stress
  knobs for `tools/verify-gst-export.sh`; the latter queues imported buffers
  while later frames are decoded, and timed-out diagnostics now request a
  graceful GStreamer interrupt before force-killing.
- Added teardown leak checks in `rust/src/v4l2.rs`: both plane-unmap paths use
  one helper, and a synthetic session test verifies every plane is unmapped
  once and its fd is closed. The 29-test suite, strict Clippy, and hardware
  verifier matrix pass.
- Reordered optional verifier probes after the required framemd5 matrix.
- Added a bounded native-reference retry in `tools/verify-rust-driver.sh` for
  transient native `h264_v4l2m2m` POLLERR/abort storms; the Rust matrix remains
  required.
- Added bounded V4L2 recovery for spontaneous EOS with pending output: pending
  chunks are replayed on a fresh session, old CAPTURE mappings remain available
  to published surfaces, and recovery state is included in timeout diagnostics.
  The latest verification passed 29 unit tests, the required 720p matrix, and
  session churn with this path enabled.
- Recovery hardening follow-up (claude agent, complements the entry above):
  - The abort has TWO firmware variants: the spontaneous-EOS one AND a silent
    one (SOURCE_CHANGE + empty CAPTURE, then permanent silence, no EOS event).
    Detection now arms on either: EOS-without-drain or an empty CAPTURE while
    work is pending and `draining=false`.
  - The rebuilt session receives the last synthesized SPS/PPS in front of the
    first replayed chunk (`H264Synth::header_bytes`, unit-tested), and old
    CAPTURE pools stay readable through a legacy index space (live pool
    indices are offset by the legacy total), so already-published surfaces
    survive the rebuild. Exports from legacy pools fail cleanly (the old fd
    is gone); copies keep working.
  - CASCADE POISONING confirmed: rapid rebuild loops (4 aborted sessions in a
    row) degraded the firmware so far that NATIVE `h264_v4l2m2m` failed the
    bframes clip (1 frame, `capture: driver decode error`) while 720p stayed
    byte-exact; the device recovered on its own after ~90 s. This is why
    `MAX_SESSION_RECOVERIES` is 1: a single rebuild rescues a one-off abort
    on a healthy device, and a second abort means the device is wedged, so
    more opens only dig deeper.
  - bframes-240p currently fails NATIVE too (2/2 attempts, 1 frame each)
    while the 720p sample passes natively, so the probe stays xfail; the
    rebuilt sessions abort with the identical signature, which points at the
    stream/firmware interaction rather than the Rust session setup.
- Vtable callback audit:
  - 41 of 60 vtable entries are real implementations; the 19 unsupported
    core callbacks now have exact typed rejection functions in
    `rust/src/vtable.rs` and return `VA_STATUS_ERROR_UNIMPLEMENTED` without
    incompatible function-pointer transmutation. The three VPP callbacks use
    the same exact-signature treatment.
  - Clean precise rejections (not generic stubs): `vaAcquireBufferHandle`
    returns `UNSUPPORTED_MEMORY_TYPE` after handle validation
    (`rust/src/buffer/handles.rs`), `vaReleaseBufferHandle` validates then returns
    success, display-attribute query reports 0 attributes and get/set are
    no-op successes, `vaQuerySubpictureFormats` reports 0 formats.
  - Browser decode hot path is fully covered: profiles/configs, surface
    negotiation/allocation/status/error, context, buffers, begin/render/end,
    sync (`vaSyncSurface`+`vaSyncSurface2`), and DRM PRIME export are all
    implemented, so the remaining 19 explicitly unsupported entries do NOT explain browser
    software-decoder fallback. `vaSyncBuffer` now validates the CPU-owned
    buffer and returns success; MF family, `vaCopy`, subpictures, and put/lock
    paths are not on the Linux browser decode path.

- Added a real `vaSyncBuffer` callback for CPU-owned decode buffers and renamed
  the implemented surface sync functions to remove stale `unimplemented`
  names. Formatting, strict Clippy, and the 29-test suite remain green.
- Updated `V4l2Session::pump` to attempt an armed abort recovery even when the
  following poll times out, and removed the sync-timeout `DECODER_CMD STOP`
  fallback so client backpressure cannot race later submissions. The explicit
  STOP path remains in teardown/recovery.
- Isolated VA vtable installation in `rust/src/vtable.rs`, reducing `lib.rs` to
  84 lines. Replaced the old incompatible generic stub function pointer with
  exact-signature rejection callbacks for all unsupported core and VPP entries.
  Added a contract test that requires every core vtable slot to be populated.
  Validated with 30 unit tests, `cargo fmt --check`, strict Clippy, release
  build, `vainfo`, and matching `__vaDriverInit_1_0`/`__vaDriverInit_1_24`
  symbols in `/tmp/libva-v4l2-rust-driver-typed-stubs`.
- Hardened all VA object-ID decoders in `rust/src/state.rs` with checked
  subtraction. Added tests for lower-bound, upper-bound, and cross-kind IDs;
  this fixes invalid lower IDs previously reaching a debug-build arithmetic
  underflow. The current static suite is 32 tests, with format, strict Clippy,
  and release build green. `/tmp/libva-v4l2-rust-driver-current7` also loads
  through `vainfo` and exports both libva init symbols at one address.
- Extracted the typed V4L2 `BufferState`, `V4l2Buffer`, and `V4l2Queue`
  bookkeeping into `rust/src/v4l2/queue.rs`; session orchestration remains in
  `rust/src/v4l2.rs` (1,537 lines before the later setup/recovery splits). The 32-test suite, strict
  Clippy, release build, `vainfo`, and both init symbols remain green in
  `/tmp/libva-v4l2-rust-driver-current8`.
- Extracted V4L2 capability discovery, format negotiation, queue allocation,
  CAPTURE STREAMON retry, and pending OUTPUT snapshots into
  `rust/src/v4l2/setup.rs`; `v4l2.rs` is now 1,261 lines. The 32-test suite,
  strict Clippy, release build, and `vainfo` pass in
  `/tmp/libva-v4l2-rust-driver-current9`. The required hardware matrix again
  passed sample-1/sample-30 and failed sample-full with VA sync error 38; the
  required churn run again ended pass=4 fail=3 on forced-kill parity legs.
- Extracted bounded firmware-session rebuild and OUTPUT replay into
  `rust/src/v4l2/recovery.rs`; `v4l2.rs` is now 1,064 lines. The 32-test suite,
  strict Clippy, release build, `vainfo`, required matrix, and churn suite were
  rerun with `/tmp/libva-v4l2-rust-driver-current10`; the matrix and churn
  reproduced the same VA error 38 and pass=4 fail=3 host-state signature.
- Extracted bounded drain, streamoff, queue release, legacy-pool unmapping,
  and `Drop` into `rust/src/v4l2/teardown.rs`; `v4l2.rs` is now 965 lines.
  The 32-test suite, strict Clippy, release build, `vainfo`, required matrix,
  and churn suite were rerun with `/tmp/libva-v4l2-rust-driver-current11`.
  The matrix again passed sample-1/sample-30 and failed sample-full with VA
  sync error 38; churn again ended pass=4 fail=3 on forced-kill parity legs.
- Hardened `vaGetImage` region validation in `rust/src/image.rs`: checked
  coordinate arithmetic now rejects negative, overflowing, surface-out-of-range,
  and destination-image-out-of-range requests before copying. Added a focused
  test; 33 unit tests, strict Clippy, release build, `vainfo`, and both init
  symbols pass in `/tmp/libva-v4l2-rust-driver-current12`.
- Added explicit unrecoverable-session propagation: once V4L2 recovery latches
  `abandoned`, `vaSyncSurface`, `vaQuerySurfaceStatus`, and
  `vaQuerySurfaceError` mark pending surfaces dead or return a decoding error
  instead of waiting for a generic sync timeout. Validated with 33 tests,
  strict Clippy, release build, `vainfo`, and the required current13 matrix
  and churn runs; the hardware results remain the known VA error 38 / pass=4
  fail=3 host-state signature.
- Hardened mapped VA buffer lifetime in `rust/src/buffer.rs` and
  `rust/src/image.rs`: mapped buffers cannot be resized or destroyed, repeated
  maps return the stable pointer, image destruction refuses a mapped backing
  buffer, and storage-size multiplication is checked. Added an API-level
  regression test for the map/resize/destroy/unmap sequence. The suite is now
  35 tests; format, strict Clippy, release build, `vainfo`, and both init
  symbols pass in `/tmp/libva-v4l2-rust-driver-current15`.
- Closed the image/buffer ownership hole in `rust/src/buffer.rs`: a generic
  buffer resize or destroy now rejects an image backing buffer, preserving the
  `VAImage.buf` reference until `vaDestroyImage`. Added an API-level regression
  test; the 36-test suite, format, strict Clippy, release build, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current16`.
- Prevented `vaGetImage` from writing through a client-mapped destination
  buffer in `rust/src/image.rs`; the callback now returns an operation error
  until the client unmaps it. The 36-test suite, format, strict Clippy,
  release build, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current17`.
- Rechecked the external compatibility probes with current17: the standalone
  PRIME verifier exits 77 because the host still lacks the libva/libav headers
  and development `.so` links, and the 8-second Chromium probe remains
  `browser_vaapi_probe=blocked_gpu_process` with timeout status 124. No driver
  code was changed by either probe.
- Moved read-only V4L2 queue diagnostics and their three formatting tests into
  `rust/src/v4l2/debug.rs`, reducing `v4l2.rs` to 838 lines without changing
  queue behavior. The 36-test suite, format, strict Clippy, release build, and
  shell checks pass; current18 loads through `vainfo` with both init symbols.
  The required matrix reproduces the existing sample-full VA error 38, and
  session churn reproduces pass=4 fail=3 on the forced-kill parity legs.
- Isolated raw ioctl numbers, libc declarations, `PollFd`, and zeroed/ioctl
  helpers in `rust/src/v4l2/abi.rs`; `v4l2.rs` now has 332 lines after the
  follow-up OUTPUT submission and polling splits. `submit.rs` owns pacing,
  OUTPUT QBUF construction, and explicit drain initiation; `poll.rs` owns
  readiness, DQBUF processing, CAPTURE lookup, and export lookup. The 36-test
  suite, strict Clippy, release build, required current21 matrix, and churn
  run pass through their normal checks; hardware remains at sample-full VA
  error 38 and churn pass=4 fail=3 on forced-kill parity legs.
- Hardened context/config ownership in `rust/src/context.rs` and
  `rust/src/config.rs`: destroying a live config is rejected, and destroying
  a context first detaches and marks its owned surfaces dead before the
  V4L2 session drops its mmap regions. Added two lifecycle regression tests;
  the 38-test suite, strict Clippy, release build, required current22 matrix,
  and churn run pass through their normal checks. Hardware remains at the
  known sample-full VA error 38 / churn pass=4 fail=3 signature.
- Added VA context dimension validation in `rust/src/context.rs`; invalid or
  oversized dimensions now fail before device setup. Added a regression test;
  the 39-test suite, strict Clippy, release build, shell checks, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current23`.
- Hardened `vaCreateConfig` negotiation in `rust/src/config.rs`: only the
  supported YUV420 render target, normal slice mode, and no decode-processing
  mode are accepted; unsupported attributes fail before a config slot is
  allocated. Added two focused tests; the 41-test suite, format, strict
  Clippy, release build, shell checks, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current24`.
- Hardened `vaCreateContext` argument validation in `rust/src/context.rs`:
  negative render-target counts and nonzero counts with null render-target
  lists now fail before device setup. The 41-test suite, format, strict
  Clippy, release build, shell checks, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current25`.
- Hardened `vaCreateSurfaces2` attribute validation and `vaBeginPicture`
  ordering: malformed surface attribute lists now fail before allocation, and
  an invalid context cannot retire a surface. Added two regression tests; the
  43-test suite, format, strict Clippy, release build, shell checks, `vainfo`,
  and both init symbols pass in `/tmp/libva-v4l2-rust-driver-current26`.
  The required matrix still reaches sample-1 and sample-30 before the known
  sample-full VA error 38 / FFmpeg EIO; GStreamer export did not reach the
  driver and churn is currently pass=1 fail=6 while `/dev/video16` remains
  wedged.
- `vaCreateContext` now validates and retains nonempty render-target lists,
  and `vaBeginPicture` rejects targets outside the context list. The 43-test
  suite, format, strict Clippy, release build, shell checks, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current27`. The
  required matrix and export/churn probes retain the current hardware limits:
  sample-full VA error 38 / FFmpeg EIO, no GStreamer driver call, and churn
  pass=1 fail=6 on the wedged node.
- Hardened `vaDestroyImage` so it verifies the linked backing buffer still
  exists before taking ownership and keeps the image alive while that buffer
  is mapped. Added an image lifecycle regression test; the 44-test suite,
  format, strict Clippy, release build, shell checks, `vainfo`, and both init
  symbols pass in `/tmp/libva-v4l2-rust-driver-current28`.
- Hardened `vaDestroySurfaces` to validate every ID before changing state,
  deduplicate repeated IDs, and reject destruction of a surface used by an
  open picture. Added a no-partial-mutation regression test; the 45-test
  suite, format, strict Clippy, release build, shell checks, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current29`. The
  required matrix still fails at sample-full with VA error 38 / FFmpeg EIO,
  GStreamer export does not reach the driver, and churn remains pass=1 fail=6
  on `/dev/video16`.
- `vaDestroyContext` now rejects teardown while a picture is open, preserving
  the active session and surface mappings until the client closes the frame.
  Added a lifecycle regression test; the 46-test suite, format, strict
  Clippy, release build, shell checks, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current30`. The hardware result remains the
  known sample-full VA error 38 / FFmpeg EIO, no GStreamer driver call, and
  churn pass=1 fail=6.
- Added explicit decode-buffer ownership in `state.rs`: buffers retain the
  context that created them, `vaRenderPicture` rejects cross-context buffer
  use, and context teardown reclaims owned unmapped buffers while rejecting
  mapped ones. Added a lifecycle regression test; the 47-test suite, format,
  strict Clippy, release build, shell checks, `vainfo`, and both init symbols
  pass in `/tmp/libva-v4l2-rust-driver-current31`. Hardware remains at the
  known sample-full VA error 38 / FFmpeg EIO and churn pass=1 fail=6.
- Made the current hardware-session policy explicit: `vaCreateContext` rejects
  a second active context before opening V4L2, instead of allowing an
  unverified multi-session path to fail nondeterministically. The 47-test
  suite, format, strict Clippy, release build, shell checks, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current32`.
- `vaGetImage` now synchronizes a pending source surface before copying its
  NV12 data, matching the existing `vaDeriveImage` and export behavior. The
  47-test suite, format, strict Clippy, release build, shell checks, `vainfo`,
  and both init symbols pass in `/tmp/libva-v4l2-rust-driver-current33`.
  The required matrix still passes sample-1/sample-30 and reaches the known
  sample-full VA error 38 / FFmpeg EIO; export does not reach the driver and
  churn remains pass=1 fail=6.
- Split pure NV12 layout, plane-offset, rectangle, and CPU-copy helpers into
  `rust/src/image/layout.rs`; image callbacks remain in `image.rs`. The 47-test
  suite, format, strict Clippy, release build, shell checks, `vainfo`, and
  both init symbols pass in `/tmp/libva-v4l2-rust-driver-current34`.
- Split surface attribute query/get negotiation into
  `rust/src/surface/attributes.rs`; `surface.rs` now keeps allocation,
  publication, status, export handoff, and teardown together. The 47-test
  suite, format, strict Clippy, release build, shell checks, `vainfo`, both
  init symbols, and module-size check pass in
  `/tmp/libva-v4l2-rust-driver-current36` (`image.rs` 402 lines, `surface.rs`
  409 lines).
- Added API coverage proving `vaCreateBuffer` records the creating context as
  the buffer owner. The 48-test suite, format, strict Clippy, release build,
  shell checks, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current37`.
- Added explicit tests for the typed optional callback boundary: `vaPutImage`,
  `vaPutSurface`, `vaLockSurface`, and `vaUnlockSurface` return
  `VA_STATUS_ERROR_UNIMPLEMENTED` cleanly. The 49-test suite, format, strict
  Clippy, release build, shell checks, `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current38`.
- Added direct FFI coverage for `vaCreateSurfaces2`: a nonzero attribute count
  with a null list fails before a surface ID is allocated. The 50-test suite,
  format, strict Clippy, release build, shell checks, `vainfo`, and both init
  symbols pass in `/tmp/libva-v4l2-rust-driver-current39`. The new GL
  round-trip parser also passes a synthetic padded-I420 two-frame comparison;
  a live importer run remains queued behind the wedged hardware node.
- Hardened NV12 copy bounds in `image/layout.rs`: stride and plane offsets now
  use checked additions, and aligned pitch handles u32 saturation safely. The
  51-test suite, format, strict Clippy, release build, shell checks, `vainfo`,
  and both init symbols pass in `/tmp/libva-v4l2-rust-driver-current40`.
- Hardened H.264 bitstream assembly in `h264.rs`: capacity arithmetic no longer
  overflows, Exp-Golomb writers reject unrepresentable values, trailing-bit
  handling cannot spin after overflow, and reference-count clamping avoids
  u8 wraparound. Added direct bit-writer coverage; the 52-test suite, format,
  strict Clippy, release build, shell checks, `vainfo`, and both init symbols
  pass in `/tmp/libva-v4l2-rust-driver-current41`.
- Added a 64 MiB allocation ceiling for client VA buffers and enforced it at
  creation and resize boundaries. Added an excessive-allocation regression
  test; the 53-test suite, format, strict Clippy, release build, shell checks,
  `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current42`.
- Added a checked 64 MiB ceiling for aggregate H.264 frame assembly, so many
  individually valid slice buffers cannot create an oversized OUTPUT packet.
  The 53-test suite, format, strict Clippy, release build, shell checks,
  `vainfo`, and both init symbols pass in
  `/tmp/libva-v4l2-rust-driver-current43`.
- Split profile/configuration negotiation and display/subpicture capability
  callbacks into `rust/src/config.rs`, reducing `lib.rs` to 487 lines. Format
  and strict Clippy checks pass; hardware verification is pending node recovery.
- Split decode-context creation and teardown into `rust/src/context.rs`,
  reducing `lib.rs` to 411 lines while leaving the V4L2 lifecycle unchanged.
  The 29-test suite, format check, and strict Clippy pass.
- Moved `vaSyncSurface` and `vaSyncSurface2`, including timeout diagnostics,
  into `rust/src/sync.rs`; `lib.rs` is now 312 lines and the 29-test suite,
  format check, and strict Clippy remain green.
- Moved `vaExportSurfaceHandle` validation, synchronization, error mapping, and
  descriptor publication into `rust/src/surface_export.rs`; `lib.rs` is now
  258 lines. The 29-test suite, format check, and strict Clippy remain green.
- Release build `/tmp/libva-v4l2-rust-driver-current2` loads through `vainfo`
  and reports the three H.264 VLD profiles; both libva init ABI symbols remain
  exported at the same address. Full decode verification still waits for the
  V4L2 node to recover from the long-hold diagnostic.
- Release build `/tmp/libva-v4l2-rust-driver-current5` loads through `vainfo`
  after the sync split and still exports both libva init ABI symbols at one
  address. Hardware decode validation remains pending node recovery.
- Release build `/tmp/libva-v4l2-rust-driver-current6` loads through `vainfo`
  after the export split, reports all three H.264 VLD profiles, and keeps both
  init ABI symbols at one address. Hardware decode validation remains pending.
- Added the export/reuse tracing described in the active task. It is gated by
  `V4L2_VA_DEBUG` and does not change queue or surface behavior; the 29-test
  suite, format check, and strict Clippy pass.
- Reworked the GStreamer hold diagnostic to retain imported buffers on a
  bounded leaky tee branch while the main branch continues decoder input. This
  separates importer lifetime pressure from the prior artificial input-starvation
  deadlock; shell syntax still passes, with hardware validation pending.

## Known blockers

- Phase 2 re-verification (claude/opus agent, 2026-09-18): with a fresh
  release build `/tmp/libva-v4l2-rust-driver-phase2-20260918` (65 host tests
  pass; `vainfo` loads all three H.264 VLD profiles), the Phase 2 exit
  criterion (reliable repeated FFmpeg + mpv copy playback) is NOT met.
  `verify-session-churn.sh` returned `pass=1 fail=6`: the reference full decode
  produced 300 frames via our driver, but the very next session (mpv
  `--frames=60` mid-stream cut) failed, and every later session failed too.
  Sharper repro than the older current51/current8 notes: after the churn,
  native `h264_v4l2m2m` decodes 300 frames on `/dev/video16` while OUR driver
  fails on the SAME node — `V4L2_VA_DEBUG` shows setup OK, OUTPUT QBUF, then
  `SOURCE_CHANGE` immediately followed by `CAP DQ idx=0 bytes=0` ("empty
  CAPTURE without drain") → session-fatal abort; the bounded rebuild replays 2
  chunks and aborts identically → VA 23 / ffmpeg EIO rc 251. Kernel log:
  `qcom-iris` `0x4000003` session-fatal bursts plus one `0x5000003` device
  power-cycle (21:21, `vb2_streamon` stack trace). NEW evidence narrowing the
  gap: `strace` of native full decode shows native ALSO keeps CAPTURE streaming
  across the initial `SOURCE_CHANGE` (585 `DQBUF`s, no CAPTURE reconfig), so the
  difference is NOT ioctl/source-change ordering. Remaining suspects: our
  re-synthesized H.264 bitstream vs native's original stream, and the
  `V4L2_DEC_CMD_START` we send on a same-dimension `SOURCE_CHANGE`
  (`poll.rs:196`) that native never issues. Our driver poisons the firmware on
  aborted/mid-stream teardown AND cannot bring up a session in the degraded
  state that native survives; a fresh (rebooted) device is needed to re-baseline
  the clean matrix, but the "reliable over repeated playback" bar stays open.

- Kernel-SILENT full-decode stall (eos-drain probe, 2026-09-18): two
  full-sample 720p framemd5 decodes within one probe run died with the SAME
  signature — `vaSyncSurface timed out state=Pending cap_idx=None
  elapsed_ms=10004`, session snapshot `out=0/16 cap=19/32
  pending(fifo=4,ready=0) eos=false draining=false aborted=false`, VA error
  38 → ffmpeg EIO rc 251 — while the kernel-log wrapper classified ZERO
  iris/vb2 messages in both windows. Distinct from both abort variants: no
  `0x4000003`, and no empty-CAPTURE/spontaneous-EOS event to arm recovery.
  Single-frame sanity decodes before AND after were byte-identical, so the
  node itself stays healthy; the stall is per-session and kernel-invisible.
  Same class as the sample-full error-38 failures other agents recorded
  today, now with the datum that the kernel sees nothing at all. Logs:
  `/tmp/libva-v4l2-eos-drain/{eos-full,post-cut}.log`.

- Latest CPU-copy compatibility validation with
  `/tmp/libva-v4l2-rust-driver-current51-20260918`: the required verifier
  failed at `sample-1` with VA status 23 (`internal decoding error`) and
  FFmpeg EIO, before sample-30 or sample-full. The mandated churn verifier
  then failed its initial reference decode with no output, so no repeated
  playback legs were run. This is the already observed poisoned
  `/dev/video16` firmware state; a clean device run is still required before
  Phase 2 can be marked complete.

- Latest required post-change hardware run with
  `/tmp/libva-v4l2-rust-driver-current8`: the 32-test suite passed and the
  required `sample-1`/`sample-30` framemd5 checks passed, but `sample-full`
  failed at `vaSyncSurface` with VA error 38 / ffmpeg EIO. The required
  session-churn run completed `pass=4 fail=3`; the three failures were the
  forced SIGKILL/SIGTERM parity legs. This matches the existing poisoned
  `/dev/video16` state and is not evidence against the queue-module split.

- `bframes-240p.mp4` exposes a firmware-side small-stream session failure
  (spontaneous EOS or silent empty CAPTURE before any decoded frame). Userspace
  triggers ruled out and one session rebuild does not rescue it; in its current
  episodes the clip also fails native `h264_v4l2m2m` (1 frame each attempt)
  while the 720p sample stays byte-exact, so the probe stays xfail, not fail.
  Kernel-confirmed as `qcom-iris` session-fatal `0x4000003` via `journalctl -k`
  (`tools/capture-iris-kernel-log.sh`, `docs/08-iris-firmware-errors.md`).
  Further HFI-level detail needs root (dynamic_debug); the error class is now
  known and matches native decode, so no more unprivileged tracing will help.
- Repeated aborted-session teardowns poison the firmware transiently: after a
  4-rebuild loop even native failed the small-stream clip, and the device
  self-recovered in ~90 s. Never loop session opens/rebuilds on aborts;
  `MAX_SESSION_RECOVERIES=1` in `rust/src/v4l2.rs` encodes this.
- The local ffmpeg `hwmap=derive_device=drm` probe fails before reaching
  `vaExportSurfaceHandle`, but GStreamer `vah264dec ! glupload` does reach it.
- NEW export-lifetime repro (GL roundtrip probe, single run): continuous
  `vah264dec ! glupload ! gldownload ! videoconvert ! filesink` on the 720p
  sample imports and downloads 28 frames byte-correct, then `glupload` fails
  with "Failed to upload buffer" (~frame 29), qtdemux/queue propagate -5, and
  the session drains via `DECODER_CMD STOP`. Timing snapshot before the
  failure: `out=0/16 cap=25/32` and repeated `vaSyncSurface timed out ...
  state=Pending cap_idx=None`. Only 7 `ExportSurfaceHandle` calls for 31
  decoded frames — gst-va appears to export each decoder-pool surface once
  and recycle surfaces, so the failure coincides with the driver retiring
  export fds on CAPTURE requeue while an imported buffer may still be in
  flight downstream. Candidate cause to verify: requeue must not retire (or
  invalidate) an export while the imported dmabuf can still be re-imported by
  the client; consider skipping retirement while a surface is referenced by
  an outstanding export, or exporting per-buffer instead of per-surface.
  Log: `/tmp/libva-v4l2-gl-roundtrip/gst-gl.log`. Full GL playback of the
  sample is blocked on this; short (28-frame) GL imports are proven correct.
- The GStreamer export hold diagnostic passes 4 buffers held for 100 ms. With
  16 buffers held for 50 ms also passes after the V4L2 pump was fixed to run
  abort recovery when poll times out. The former linear 250 ms hold stalled
  after seven submissions; the hold pipeline now uses a leaky tee and needs a
  clean hardware rerun before this is classified as an export-lifetime bug.
- Manual `V4L2_VA_GST_EXPORT_BUFFERS=8 tools/verify-gst-export.sh ...` timed
  out once with a pending surface after EOS; `1`, `4`, and a later `16`-buffer
  process completed. One post-recovery main-verifier run also hit four repeated
  spontaneous-EOS rebuilds and timed out, while the immediate rerun and the
  current 16-buffer run pass. Keep the main verifier at the stable default while
  this hardware-level intermittency is investigated.
- After the 250 ms hold timeout, the node stayed poisoned through the expected
  recovery window: the next churn run passed four setup/playback legs but the
  three forced SIGKILL/SIGTERM parity legs failed. Treat those failures as the
  current host firmware state; the last clean required verifier remains the
  serial run with the 29-test suite and four-source-change resolution probe.
- A later stable probe with `/tmp/libva-v4l2-rust-driver-current4` still timed
  out with status 137 after the node was idle, so the leaky-tee hold change has
  not yet received hardware validation. A device reset or equivalent kernel
  recovery is required before making another decode attempt.
- `tools/verify-export-prime.sh` exits 77 until libva/libav development headers
  and unversioned development `.so` links are installed.
- Firefox remains a snap browser-launch limitation: its RDD capability gate
  disables VAAPI and selects software FFmpeg H.264. Chromium is no longer
  blocked at launch when the verifier uses `native` mode; its GPU process stays
  alive and `VaapiVideoDecoder` reaches this driver. The remaining Chromium
  proof is clean-node end-to-end decode, followed by checking whether its
  allocate-mode surface flow exposes any additional driver requirements.

## Next safe steps

1. Rerun `tools/verify-browser-vaapi.sh` in Chromium `native` mode during a
   clean device window and capture successful end-to-end browser decode. An
   unconfined Firefox build remains useful for a second browser implementation,
   but is no longer required to prove that Chromium can reach this driver.
2. Install the libva/libav development packages needed by
   `tools/verify-export-prime.sh` when possible, or add another no-compile
   importer path.
3. Extend the resolution probe to repeated changes, seeks, and longer
   mixed-resolution playlists while keeping the current two-clip check green.
4. `bframes-240p`: the firmware error CLASS is now captured unprivileged with
   `tools/capture-iris-kernel-log.sh` (session-fatal `0x4000003`, escalating to
   device-wide `0x5000003` + power-cycle; see `docs/08-iris-firmware-errors.md`).
   To go deeper needs root: enable `qcom_iris` dynamic_debug, then diff the HFI
   command sequence of a failing small session vs a passing 720p session. Treat
   small-stream decode failures as retryable in clients. Do NOT automate retries:
   repeated aborted sessions poison the firmware for ~90 s (see blockers).
