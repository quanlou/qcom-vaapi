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
AV1 PRODUCER COPY-ON-WRITE HEADER OWNERSHIP HOST PASS (2026-10-02):
New reusable CBS helper clones parsed content through standard CBS writable-unit
API, retaining tile buffer and writer-held sequence lifetimes. Original parsed
headers are checked byte-for-byte unchanged after serialization; show-existing
visibility/reference state remains original. Host five samples including10bit
PASS690coded/684display exact tiles/pixels/order, Werror build and13 focused tests
PASS. Evidence av1-producer-ownership-host.1ugsj9kg. No decoder/device opens,
installed changes or operator step. This is producer ownership groundwork, NOT
actual VA transport/driver ownership/Chromium qualification. AV1 stays disabled.
Next wire helper into isolated actual FFmpeg VA producer plus validated full-OBU
standard slice-data driver path; freeze paired experiment before hardware.
Baseline remains user-deferred/incomplete; every prior failure retained.

AV1 VA PRODUCER OWNERSHIP INTEGRATION ACTIVE (2026-10-02): implement CBS
copy-on-write normalization of actual parsed headers, preserving original parser
visibility/reference state and compressed tiles. Host corpus checks first; no
hardware or advertised AV1 changes. Baseline stays explicitly deferred.

AV1 NATIVE NORMALIZED DIVERSE PASS / VA INTEGRATION NEXT (2026-10-02):
User explicitly deferred fresh baseline; interrupted baseline retained incomplete.
Aom99coded/96display p6yso_fj PASS; corrected active-sequence original300/300
skv3j30x PASS; rav1e96/96 c9puqkke PASS; SVT96/96 dkg8np7f PASS. All591coded
and588original-display NV12 pixels/order byteexact, full clean kernel windows,
exit0/no linger/safe final idle/current3loaded identities unchanged. Summary
av1-native-normalized-diverse-summary.json. Original300 truncated139 failure
preserved; corrected input removes superseded differing MP4 extradata sequence
before actual inband header, full changed run now PASS. Original faulted4f13
never opens again, operator normal restart currente56c1f3a verified. No extra
sudo/boot/module edits. Host5samples including10bit/690coded/tileproof/13tests
PASS; native10bit untested. This establishes normalized native hidden-reference
pixel path for tested corpus, NOT AV1 VAAPI/Chromium or full production. Next
implement actual FFmpeg VA producer original/CBS headers+standard slice-data
transport with validated driver ownership, then strict VA diversity/lifecycle/
Chromium. AV1 remains advertised disabled; active-playback sleep/streaming
removal still unqualified. Preserve all failures; no unchanged retries.

AV1 DIVERSE FIRST FAILURE / ACTIVE SEQUENCE HOST FIX (2026-10-02):
Original300 sample av1-coded-diverse.e56c1f3a.0.op_jii4c FAIL139/300frames,
clean complete observer/exit0/no linger/ref0. Offline exact hash audit matches all
139returned frames to originalcoded161..299; no emptyERROR completions. Serial
runner stopped, rav1e/SVT packets never attempted. Preserve full failure.
Input has differing MP4-extradata and immediate inband sequence headers before
frame0; active inband header recurs at frame161. Packetizer now omits superseded
sequence headers before a coded frame (no intervening frame consumes old header),
retains last active header and all compressed tiles.13 focused host tests PASS;
five corpus active-sequence software proof running/completing. Firmware cause
unproven until changed hardware proof, no unchanged failure retry. Baseline stays
user-deferred/incomplete; AV1 advertised support remains disabled/unqualified.

AV1 DIRECT CHANGED NATIVE PASS (2026-10-02): av1-coded-native.e56c1f3a.p6yso_fj
single99coded normalized libaom frames ALL NV12 byteexact/order PASS; original
96display alias projection PASS. One decoder session(no probe decode), one coded
frame per packet, passthrough output; complete clean kernel observer/process-tree
exit0/no linger, safe final idle and3loaded identities unchanged. This proves
actual hidden reference pixels for tested normalized path, not raw hidden payload
or VAAPI/Chromium integration. Old system fault/81drop failures remain FAILED;
boot4f13 permanently excluded. No general firmware root-cause claim. Baseline
explicitly deferred by user, interrupted incomplete. Next native original300/
rav1e96/SVT96 diverse checks serial stop first failure, then actual VA producer/
driver header/reference integration. No extra sudo/reboot/unload/sleep needed.

USER DEFERRED BASELINE / DIRECT CHANGED AV1 EXPERIMENT (2026-10-02): user
explicitly requested skip baseline to conserve agent bandwidth. Owned baseline
process tree terminated, no lingering children; user-interruption.json preserves
incomplete result, never qualification PASS. Currentboot e56c1f3a exact3loaded
identities/selectedSHA/ref0/suspendedusage0/auto and wholebootclean verified after
cleanup. Fresh av1-coded-native.e56c1f3a.p6yso_fj frozen99coded libaom packets,
unique coded timestamps/no probe decode/passthrough output, full99NV12 software
reference and96original display alias projection. Host five corpus/12tests PASS.
Explicit baseline defer recorded in frozen plan, remaining safety/parity/timeout/
cleanup unchanged. Single changed experiment starting, not old failure retry.
No extra sudo/reboot/module operations. Stop first fault; AV1 remains disabled.

AV1 HOST CODED-FRAME CORPUS PASS / FRESH SUPPORTED BASELINE ACTIVE:
av1-coded-frame-host.bjt63i9i/corpus all five samples PASS690coded frames,
684original displays, exact compressed tiles and pixels/order, including10bit.
One complete FRAME OBU per IVF packet and coded-frame monotonic timestamps;
no probe decode and output passthrough.12 focused AV1 tests PASS. Firmware fault
cause remains unproven; no hardware validation of changed framing yet.
Newboot e56c1f3a operator normal restart, actual3loaded builds/selectedSHA/idle/
wholeboot journal clean read-only PASS. Current-boot persistent activation evidence
saved. Fresh av1-prereq-boot.e56c1f3a.yi5595k4 exact unchanged v15 source/harness/
driver hashes PASS; full supported correctness/lifecycle baseline starting before
new changed AV1 experiment. Never use faulted4f13 for hardware, preserve old FAIL.
No privileged writes/reboot/unload/sleep/sudo requested. AV1 remains disabled.

AV1 HOST DIAGNOSIS CONTINUES AFTER OPERATOR NORMAL RESTART: currentboot
e56c1f3a normal chosen restart confirmed; no hardware opens. Actual private FFmpeg
host strace with ordinary-file mock (no video-node opens) shows old probe3opens,
-nofind_stream_info1open. Evidence av1-no-probe-host-r2.bn83mtsn; preliminary
probe-session fault cause still unproven. Changed host diagnostic uses one complete
FRAME OBU per IVF packet, monotonic coded-frame timestamps, nofind_stream_info
and output fps_mode passthrough; unsupported OBU layouts refused. New host corpus
check active av1-coded-frame-host.bjt63i9i, never hardware. Old fault/drop evidence
kept, no unchanged hardware retry. Framework fault guard rejects4f13/b86c.

AV1 CHANGED NATIVE EXPERIMENT FAILED / HARDWARE STOP (2026-10-02):
av1-visible-native.4f13b3c1.0oensoo1 result FAIL, actual Iris system error0x5000003
and vb2_start_streaming WARN; current4f13b3c1 is FAULTED, never decoder-open again.
Process monitor exited1 in0.595s, no linger/signal denial/unresolved children;
private FFmpeg terminated, module ref0. All raw logs/failed packet retained.
Host normalization five samples684display/690coded/tile parity remains host only.
Native reported99decoded but muxed81 with18timestamp drops; no pixel pass inferred.
Producer normalization/header packaging/probe-session fault cause UNPROVEN.
A later boot now e56c1f3a-108d-4e3e-9440-f0c397aafa6b detected; operator restart
reason pending. No decoder opens on newboot until identities/clean/prerequisites
and changed experiment reviewed. No repeat unchanged candidate, sudo, agent
reboot/sleep/unload. Continue HOST diagnosis and producer/reference integration.
AV1 remains disabled/unqualified. Prior supported sleep/decode passes retained.

AV1 VISIBILITY HOST CORPUS PASS / CHANGED NATIVE EXPERIMENT (2026-10-02):
Actual CBS visibility normalization only hidden INTER, zero-refresh show-existing
commands omitted; hidden KEY/timed decoder models refused. Five software samples
(original/libaom/rav1e/SVT/10bit)684 displayed projected byteexact from690 decoded
frames, all690 compressed tile groups unchanged. Host failures for raw-container
identification retained; explicit IVF packet framing corrects harness. No VAAPI
support claim. Evidence av1-visible-reference-host.ug562ias/corpus-r3/result.json.
Fresh frozen av1-visible-native.4f13b3c1.0oensoo1 checks sameboot activation/three
actual loaded identities/selectedSHA/sleep counter/source+harness+native hashes,
shared lease and wholeboot clean. Readonly preflight PASS. One changed normalized
99-frame libaom native experiment next; bounded45s cleanup/kernel monitoring,
full99coded byte parity plus96original display projection, no retry on failure.
No new module install/sudo/reboot/sleep/removal. AV1 VAAPI remains disabled.

Host AV1 visibility/reference experiment ACTIVE: use actual CBS parsed headers
to request hidden INTER pictures as visible decoded outputs, omit show-existing
alias commands only with zero refresh, and verify original display projection
byteexact in software before any hardware or VA integration. Reject hidden KEY
and timed decoder models initially; retain original headers/index and tile bytes.
This experiment changes visibility and is not unchanged-bitstream transport.

Observer settling fix completed: tools/qualify-iris-av1-completion-trace.py now
waits bounded15s read-only after successful process cleanup, checks journal every
poll and refuses identity/fault errors immediately. Seven focused host tests and
py_compile PASS; no new hardware run. Independent subsequent-idle-witness.json
binds exact sameboot/build/selectedSHA/ref0/suspendedusage0/auto/clean journal.
Original failed packet untouched. Continue AV1 header/reference implementation.

AV1 LIVE TRACE ACTUAL RESULTS (2026-10-02): client-open normal removal refusal
PASS on current4f13b3c1. Live BPF attached and stopped cleanly; worker96 displayed
frames ordered byteexact PASS,197 raw completions including42 NOSHOW, all42 raw
sizes0 and timestamps nonzero. Complete kernel observer/process-tree exit0/no
linger PASS. Overall packet remains FAIL: immediate final idle check preceded
runtime autosuspend. Independent subsequent idle suspended/usage0/auto/ref0;
no new kernel fault. Preserve result.json and all old failures; never rerun this
packet. Fix host observer settling with bounded read-only wait and fault/identity
checks, then continue actual-header/hidden-reference AV1 integration. Raw hidden
payload absent means ERROR-bit clearing cannot supply hidden decoded pixels.
AV1 VAAPI/Chromium still unqualified; active-playback sleep/streaming removal
remain unqualified. No new sudo/reboot/removal requested.

AV1 FASTER LIVE TRACE PREPARED (2026-10-02): user prioritizes completing AV1.
Frozen av1-live-trace.4f13b3c1.l_9qb4rh/run-next-reviewed.py ready. Exact installed
module SHA and DWARF-derived inst.codec/session/HFI/frame-info offsets bound;
manual read-only BPF kprobe/kretprobe snapshot at actual OUTPUT function, no BTF
module needed, no device addresses logged, no module/boot/PM writes. Trace startup
must attach before decoder opens; runtime verifier/attachment UNPROVEN as root.
Unprivileged codegen failed solely tracefs permission, preserved, no attachment.
Four focused classifier tests PASS corruption/overflow/rejected/incomplete/nohidden
and per-codec complete verified-control pairs. Prior strict control4vs2 failure
remains failed; exact old offline reanalysis proves2contexts each valid pair, new
classifier rejects omitted/unverified/duplicate pairs, not arbitrary count relax.
Readonly identity/idle/ref0/clean/shared-lease preflight PASS. Copies frozen native
producer/reference fixture, requires full96 displayed ordered byte parity, complete
global clean kernel observer/45s process-tree timeout/no linger. Kernel checks
while decode runs; any error stops first experiment. Raw NOSHOW size/timestamp
only observation, never hidden-pixel validity or AV1 VAAPI/Chromium support.
Operator wrapper first completes pending client-open removal refusal once if not
attempted, refuses existing failed/incomplete evidence; then conditional traced
AV1 experiment. No forced removal/reload/install/reboot/sleep. Agent no sudo,
trace attachment or decoder opens. Necessary new combined operator command:
sudo python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/av1-live-trace.4f13b3c1.l_9qb4rh/run-next-reviewed.py
Do not repeat request unchanged or retry failure. Canonical diagnostic source
 tools/qualify-iris-av1-completion-trace.py. AV1 transport/hidden lifetimes remain
incomplete; inspect actual evidence next, then implement evidence-supported fix.

HOST AV1 OBSERVATIONAL TRACE PREPARED (2026-10-02): isolated candidate
av1-raw-completion-trace-host.26bikaen W=1 build PASS against exact installed kernel
headers. Compiler Ubuntu15.3.0-3 vs installed15.3.0-4/pahole absent warnings and
BTF skip retained; no runtime compatibility claim. Repo diagnostic patch
kernel/diagnostics/iris-av1-raw-completion-trace.patch adds AV1-only dev_dbg after
buffer identity/queued validation and before firmware metadata copy/flag mapping.
Records session/index/raw size/offset/timestamp/HFI flags/picture/no-output/
corrupt/overflow, no device addresses. Dynamic debug default disabled. Removing
only trace block restores exact prior source; NOSHOW ERROR/payload/ownership and
all PM behavior unchanged. Not in production patch stack, not installed, no
hardware opens or protected/module writes. Binary/source identities preserved.
Future evidence can distinguish raw firmware completion from VB2 metadata
clearing; nonzero size alone never proves valid hidden pixels. Existing module
results never qualify this diagnostic candidate. Do not deploy/start hardware or
repeat privileged requests while existing operator removal-refusal remains pending.
AV1 producer transport and hidden surface lifetime remain incomplete.

HOST AV1 ORIGINAL REFERENCE INDEX COMPLETE (2026-10-02):
Existing CBS original-OBU probe now records resolved refresh masks, frame type and
show-existing slot index. tools/inspect-av1-reference-index.py follows original
header reference aliases, not VA surface/DMA ownership. Fresh isolated -Wall
-Wextra -Werror build and five actual software ordered pixel roundtrips PASS
684 displayed frames including 10bit; original/aom/rav1e/SVT/10bit show-existing
aliases149/39/45/45/39 all refer to previously hidden frame headers in this corpus.
Evidence av1-original-reference-host.r_ti3dbd/reference-summary.json; frozen sources
and all inputs retained. An evidence script named inspect.py shadowed Python
stdlib and failed; traceback/source retained, renamed evidence script fixes import.
No kernel/module/VA capability changes, no hardware opens. This is host reference
metadata audit only, not complete AV1 conformance, VA transport or hidden pixels.
Original producer transport source anchors also retained in
av1-va-transport-source-audit.b5w2x0im/audit.json. Current FFmpeg start_frame sees
original frame bytes and retains sequence bytes, but decode_slice supplies tile
payload; Rust keeps only selected ranges. Paired bounded prefix/tile-offset
transport and real hidden surface lifetime remain required. Operator removal
refusal still pending; do not repeat the already presented command.

POSTWAKE FULL/SCOPED PASS / CLIENT REMOVAL REFUSAL PREPARED (2026-10-02):
exec34519 exited0. Sameboot4f13b3c1 genuine idle deep sleep preceding then full
production.SmVk4F strict headless matrix/parity/churn/EOS/seeks PASS. Real-use
run.jaca_p3g PASS4K3000byteexact65.13s46.0617FPS483252KiB actual FFmpeg RSS;
Chromium755/25.0004s30.1995FPS779hardware completions/seek/cleanexit. RuntimePM
before/after PASS; null600frames cleanexit/complete clean window PASS; final3
loaded IDs unchanged and sleep_success remains1. All runners exited. This proves
one idle deep wake followed by required decoding, not active-playback suspend.
Next frozen active-removal-refusal.4f13b3c1.gy64n0o4 ready;2focused hostclassifier
tests PASS (busy pin success, permission/wrongID/noPin/exit0 fail). Readonly
preflight PASS idle/ref0/3loadedIDs/selectedSHA/currentclean. Operator-only worker
opens idle client, requires positive module ref, plain rmmod once must refuse
specifically busy with same loaded module/pin retained. No forced removal/reload/
reboot/sleep/retry; bounded process-tree/global kernel observer; closes ownfd then
verifies idle ref0. Scope open idle client, active streaming remains unqualified.
Necessary operator sudo python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/active-removal-refusal.4f13b3c1.gy64n0o4/run-reviewed.py
Agent did not open decoder or attempt removal for this packet. Wait quietly pending.
AV1 VAAPI producer/hidden-reference ownership/Chromium remains incomplete; native
AV1 passes alone never enable advertised support. Preserve all older failures.

SAMEBOOT DEEP SLEEP PASS / POSTWAKE GATES ACTIVE (2026-10-02): operator
idle-sleep-postdecode.4f13b3c1.ecvqq5_f genuine deep witness PASS boot4f13b3c1,
sleep_success0->1, PMnone, complete clean kernel window; actual3patched identities
unchanged. Fresh frozen postwake-qrtr.4f13b3c1.4v5a85g1 binds preserved before/after/sleepresult, exact
v15 driver/source/harness and current-boot activation. Preflight idle/clean/lease/
selectedSHA/loaded builds PASS. verify-loaded also refuses additional sleep cycles.
run-all.sh ACTIVE exec34519 full strict then conditional4K/Chromium/runtimePM/null
regression; before-gate PM PASS. Inspect without competing; no repeated tests.
No operator action needed while running. No agent sleep/reboot/unload/privileged
writes. Previous failed boots/evidence retained; AV1 VAAPI and active-client
removal-refusal still incomplete. Do not call expanded production complete.

FRESH PATCHED BOOT ALL SCOPED GATES PASS (2026-10-02): exec57728 exited0.
qrtr-installed-boot.4f13b3c1.fo17v1gt production.uDTPWA full strict correctness/
lifecycle PASS; real-use run._2skxs22 PASS4K3180byteexact70.25s45.2669FPS491320KiB
actual FFmpeg RSS (whole observer tree553012KiB separately). Chromium757frames/
25.0003s30.2796FPS780hardware completions/seek/cleanexit PASS. Before/after runtime
PM PASS;600frame null regression cleanexit/complete clean kernel window PASS.
Final three loaded identities PASS; no runners remain or competing hardware.
Sameboot postwake decoding still pending because operator chose normal restart
between prior deep witness23b4 and these freshboot4f13b3c1 results.
Fresh observer idle-sleep-postdecode.4f13b3c1.ecvqq5_f prepared read-only after
sameboot strict/scoped prerequisites and idle/clean state; no sleep transition.
Necessary operator command systemctl suspend once, wake normally DO NOT RESTART,
then python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/idle-sleep-postdecode.4f13b3c1.ecvqq5_f/observe-reviewed.py verify
After PASS run required fresh full strict/scoped decode gates under lease on this
sameboot and preserve before/after sleep evidence. No repeat on failure/hang.
This required cycle follows prior PASS, not retrying unchanged failed experiment.
AV1 VAAPI and active-client removal-refusal still incomplete. No completion claim.

FRESH PATCHED BOOT FULL GATE PASS (2026-10-02): qrtr-installed-boot.4f13b3c1.fo17v1gt/
production.uDTPWA strict full headless gate PASS pixel parity/matrix/churn/EOS/seek
clean kernel windows and final frozen driver identity. Run-all exec57728 remains
ACTIVE scoped4K/Chromium/runtimePM/null regression; before-real-use PM PASS.
Do not start competing tests. These are fresh boot4f13b3c1, not postwake on23b4.
Actual deep sleep PASS preserved; user normal restart confirmed. Need genuine
sameboot postwake decode qualification after runner completion. No extra sudo or
restart requested; no new agent sleep/module operations.

DEEP SLEEP PASS / FRESH RESTART GATES ACTIVE (2026-10-02): actual idle deep
sleep witness PASS on23b4aa5c, sleep_success3->4, PMnone, clean completed kernel
window and three actual patched module identities checked. User chose normal
restart afterward; independent journal orderly shutdown confirms, now boot
4f13b3c1-dddf-4b86-9667-b104b7fad629. Cannot label fresh boot tests post-wake on oldboot.
Current cold3loaded identities and selected exact pair PASS; journal clean,
Irisref0/suspendedusage0/auto. Fresh frozen qrtr-installed-boot.4f13b3c1.fo17v1gt
copies prior exact v15 source/harness/driver, fresh current-boot activation evidence.
run-all.sh ACTIVE exec57728 full strict gate then conditional scoped4K/Chromium/
runtimePM/null regression, stops first failure. Before-gate runtime PM PASS.
Inspect runner/logs without competing hardware or repeated tests. No new sleep/
module/reboot by agent. Next genuine sameboot post-wake decode still pending;
AV1 VAAPI and active-client removal-refusal still incomplete. Old failures kept.

PATCHED QRTR STAGED PM PASS / IDLE SLEEP OBSERVER PREPARED (2026-10-02):
Currentboot23b4aa5c pm-qrtr-fixed.23b4aa5c.pffv4qkb actual freezer5.504s/devices6.554s/
platform8.058s PASS, exit0/no linger/complete clean global kernel windows; all
controls restorednone/0/0, Iris ref0/suspendedusage0/auto. Three actual loaded IDs
verified before/after wrapper. Previously failed Wi-Fi platform symptom absent in
changed candidate run, not proof of earlier true deep hang cause. Idle sleep
observer frozen idle-sleep-qrtr-fixed.23b4aa5c.avk4woal,7witness hosttests PASS; agent prepared before journal
cursor/sleep_success snapshot only, no sleep or privileged/module operation.
Operator next save work then select Sleep manually, let it sleep then wake normally;
verify command python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/idle-sleep-qrtr-fixed.23b4aa5c.avk4woal/observe-reviewed.py verify
Wrapper verifies actual3loaded identities/selected QRTR hashes and sameboot staged
PM pass; observer requires genuine PMnone, increasing sleep_success, completed
entry/exit/no fault and idle Iris. User authorization for operator-led tests persists.
Do not repeat if failure/restart/hang; preserve evidence and inspect journal. After
successful actual sleep witness full required fresh postwake decode gates remain.
AV1 VAAPI and active-client removal-refusal remain incomplete. Stay quiet pending.

QRTR COLD BOOT VERIFIED / PATH BUG FIXED (2026-10-02): user verifier failed
selection comparison because /lib symlink resolves /usr/lib on actual path only.
Original verifier and failure JSON preserved; resolve both sides, mismatch still
rejected. Corrected verifier PASS boot23b4aa5c-c050-4c60-bee9-e6d7bdfdd52f actual
QRTRa3742ff58316450e2f4cbc77f799d29f567277d4/MHI2a9b093bf2b5cfcddd4a3b31f5b3ad8cb3757de4/Iris231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2.
Exact selected pair SHA PASS; independent kernel journal no matching fault/WARN.
Fresh operator-only staged packet pm-qrtr-fixed.23b4aa5c.pffv4qkb; readonly preflight PASS idle/ref0/
suspendedusage0/auto and PMnone/0/0. Frozen wrapper binds current activation,
three loaded build IDs and selected QRTR pair before/after each stage; shared
lease/strict observer/restoration in tools; freezer then devices then platform,
stops first failure, exclusive attempt witness prevents unchanged retry. No real
platform machine sleep, module ops/reboot/decoder opens by agent. Necessary
operator command sudo python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/pm-qrtr-fixed.23b4aa5c.pffv4qkb/run-reviewed.py
Save work/screen may dark briefly. Wait quietly while pending. Genuine system
sleep/postwake, active-client removal-refusal and AV1 VAAPI still unqualified.

QRTR PACKAGE LOCATION CORRECTED (2026-10-02): user requested repository ownership,
not Chromium Snap storage. Upstream patch now kernel/0006-qrtr-resend-hello-on-mhi-resume.patch.
Both failed r1 and installed r2 deployment bundles moved to qualification workspace
resume-20261002/deployment/qrtr-resume-r{1,2}; old Snap paths are compatibility
symlinks only, no duplicated payloads. R2 resolves BUNDLE to canonical location;
INSTALL.txt current commands updated, pre-relocation identities preserved separately.
Installed boot files/root rollback backups untouched; no reinstallation needed.
Next after operator normal restart use canonical r2 verify-cold-boot.py.
QRTR patch is an upstream platform dependency, separate from Iris patch stack;
never apply automatically to Iris-only source. No hardware/PM operations.

QRTR R2 OPERATOR INSTALL SUCCESS (2026-10-02): user reports
prior_rollback_verification=pass paired_boot_files=installed live_modules=unchanged.
Independent readonly selection+SHA checks confirm exact qrtr/qrtr-mhi overrides
and corrected hook. Current boot8f19b5c6 still original QRTR builds0f5c6c68820ae6e5cda0a4299b8413d4c3c750c1/
4c0b03365c6835ac0adbca717157a24bb3e903f4; Iris v15 unchanged. No decoder/sleep/module
operations by agent. Operator normal restart then r2 verify-cold-boot.py required.
Do not request further sudo/install retries while waiting. No sleep before actual
changed identity checks and fresh reviewed diagnostic packet. Failed r1 retained.

QRTR R1 INSTALL FAILED / R2 CORRECTED (2026-10-02): operator r1 install rejected
unexpected qrtr in initramfs; automatic rollback paths ran, original selection
restored/override absent/hook absent. Root backup state/exact initrd+metadata
restoration must be verified by r2 privileged preflight before changes. Do not
rerun r1. Actual distro auto_add_modules =net/qrtr copies original subtree before
hooks; real staged directory+explicit module copy reproduced original+candidate
duplicates. Initial rehearsal missed this directory-copy path. Failed r1 bundle
and root backup preserved. R2 hook calls apply_add_modules, checks exact both
candidate image hashes, removes only original qrtr/qrtr-mhi variants from DESTDIR,
never host distro modules. Final image still strictly rejects any wrong copy.
Rehearsal-r2 actual generated depmod selection/exact pair/hash/wrong-copy refusal
PASS;9transaction/rollback/prior restoration tests PASS; readonly preflight PASS
with root rollback verification deferred. Corrected bundle:
/home/mq/snap/chromium/common/libva-v4l2-production/qrtr-resume-r2/INSTALL.txt.
Necessary corrected operator install step presented; only after success normal
operator restart and r2 verify-cold-boot.py. Never live unload QRTR/Wi-Fi; no sleep
retry before new reviewed packet with actual changed loaded identities. No r2
privileged writes, reboot, sleep or decoder opens by agent. Currentboot8f19b5c6.

QRTR REVERSIBLE BOOT PACKAGE READY (2026-10-02): bundle
/home/mq/snap/chromium/common/libva-v4l2-production/qrtr-resume-r1/INSTALL.txt.
Paired qrtr/qrtr-mhi overrides and version-gated explicit initramfs hook; retains
Iris v15 and distro modules; snapshots baseline initramfs/module metadata;
completed failure automatic restore, unfinished writer refuses competing rollback.
6mocked transaction tests PASS, readonly preflight PASS, staged actual depmod/
dracut-install copied exact pair+Iris PASS. Cold verifier prepared checks all3
actual loaded build IDs and selections after new operator boot. No privileged
writes/module operations/sleep/reboot. Operator install command presented once,
then only on successful installation operator normal restart/cold verifier.
Do not repeat unchanged requests while pending; no sleep until changed identities
and a NEW reviewed diagnostic packet. Existing failed PM evidence stays failed.

QRTR UPSTREAM CANDIDATE HOST VERIFIED (2026-10-02): exact upstream patch applied
without edits to authenticated distro QRTR source in qrtr-resume-review/candidate.
Exact running kernel headers W=1 modules PASS; actual-function UBSan host model
4resume/error paths +10000handshake resets PASS (not concurrency/hardware proof).
ABI imports PASS qrtr127/qrtr-mhi24; added qrtr_endpoint_hello resolved against
candidate qrtr export. SHA qrtr0981df0407fe42db0064a9d47a97f7836d23042cd25365914552a9bfc38f8485;
qrtr-mhi4fe0acdbf99364f87bcf0a67e21dcf7045473016e0a8ff978e081a8a9ba39715.
Review/identities/build/model evidence saved; no deployment or live module change.
Next prepare paired reversible future-boot deployment; never live unload QRTR/Wi-Fi.
Only changed candidate after operator installation/boot can receive controlled PM
retest. Do not retry existing platform packet or claim system sleep qualified.

LATEST PLATFORM FAILURE / QRTR REVIEW (2026-10-02): operator platform diagnostic
FAIL on b86c3104: ath12k_wifi7 WCN7850 restart timeout -110, two mac80211 WARNs;
Iris suspend/resume callbacks returned0. Runner exited0/no lingering, controls
restored none/0/0; strict global kernel observer correctly failed. Failed evidence
and SHA manifest preserved in pm-platform-packet.b86c3104.zlgf9wpy. No further
hardware or PM tests on this warned boot; no unchanged retry. Host independently
restarted to 8f19b5c6-686e-4201-ac31-3d75443ddf72; persistent v15 cold identity PASS,
no new decode qualification. Agent performed no reboot/sleep/module operation.
Exact authenticated distro archive extracted QRTR source: handshake-at-register
present, resume handshake absent. Upstream fix6a5719cc3ef2e4d9857cc4ae18e6db09d59a8cc9
net: qrtr: resend HELLO on MHI resume matches exact WCN7850/ath12k_wifi7 symptom.
Prepare isolated credited upstream backport and exact-header build before any
operator deployment/test. This does not prove the cause of earlier deep hang.
AV1 VAAPI/Chromium remains gated, real sleep and active-client removal-refusal
remain incomplete. Old results remain bound to their original boots.

LATEST PLATFORM PM PREPARATION (2026-10-02): operator-only platform dry-run
packet pm-platform-packet.b86c3104.zlgf9wpy prepared. Exact 7.3 distro suspend.c
SHA71c4a16ad3db8cac349b7d8b52cdfddae26a8918216f18ec54a68c1761077576 extracted
from cached package archiveSHA2519439d088772d8dfe298d5c1c4bcff7c2744abef41183c93e0bd1e97614813
matching .dsc. Actual suspend_enter host model6cases PASS: TEST_PLATFORM bypasses
CPU offlining/syscore/platform machine sleep,5callback error rollback paths covered;
not hardware/concurrency proof. Initial host compile missingerrno failed evidence
platform-boundary-host preserved; corrected platform-boundary-host-r2 PASS.
Operator runner adds platform stage only, requires preceding PASS devices sameboot/
build/restorednone, refuses unavailable stages, explicitly witnesses restoration
of debug/print along with pm_test.8focused witness/restoration/prerequisite tests
and174tooling tests PASS51.966s; py_compile/diffcheck PASS. Frozen wrapper verifies
all packet identities before action; shared hardware lease/exactloaded/idle/clean
kernel gates remain. Read-only --check PASS sameb86c3104/build/ref0/suspendedusage0
andnone/0/0; NO transition or privileged write run. Necessary operator step:
sudo python3 /home/mq/.cache/libva-v4l2-qualification/resume-20261002/pm-platform-packet.b86c3104.zlgf9wpy/run-reviewed-platform.py
Save work; screen may dark briefly. No module/reboot/CPU offline/actual deep sleep.
Stop and inspect failure; do not retry. Inspect pending evidence quietly, no repeated
operator request. Devices/freezer actual PASS remain diagnostic-only; real sleep/
postwake and active-client removal-refusal still incomplete. AV1 native diverse
passes retained; VA hidden-reference ownership/Chromium integration remain gated.

LATEST AV1 EXPLICIT DECODE-ORDER DIAGNOSTIC (2026-10-02): fresh isolated FFmpeg
source/build av1-order-diagnostic-source/build; standard display-delay0/enable1
S_CTRL+G_CTRL verified before streaming, AV1-only opt-in, capture trace only;
receive-core discard model PASS. Installed VA/kernel unchanged. Single bounded
av1-decode-order.b86c3104.8lk29q48 hardware experiment:96displayed full ordered
pixel hashes exact/exit0/no linger/kernelclean. Strict runner result FAIL preserved:
checker assumed exactly2control messages, FFmpeg probe and real decode initialized
2distinct contexts=>4messages. Offline capture-review.json proves both contexts
set/read exactcontrol10029965=0/10029966=1. No hardware retry or failed-result rewrite.
139capture completions:96nonempty,42empty ERROR alltimestamp0,1emptyEOS. Explicit
mode does not make hidden reference surfaces available through current kernel.
Raw HFI payload/hidden pixel validity NOT observed; do not clear error flags on
assumption. Need further verified hidden-reference transport/firmware semantics
before VAAPI/Chromium AV1 support. Original source patch and identities frozen.
All runners exited/refcnt0. Device-stage operator PM PASS6.6598s and temporary
pm_test/debug/print restorednone/0/0; genuine system sleep still unqualified.
Diverse native AV1 aom/rav1e/SVT96 each PASS after device PM, clean windows.
Next: continue paired producer/hidden-reference ownership work and staged operator
PM diagnostics; no repeated deep sleep or unchanged hardware failure.

LATEST AV1 DIVERSE HARDWARE PASS (2026-10-02): after operator device-stage PM
PASS, serial exact prepared native cases aom-720p/rav1e-720p/svt-720p all PASS96
ordered full pixel hashes each, exit0/no lingering/complete clean kernel windows.
Evidence av1-diverse-prepared.b86c3104.4qj9mp36/{aom,rav1e,svt}-720p. Current same
boot b86c3104/exact persistent build/refcnt0. Installed VA/module unchanged;
AV1 profile gated. Native display output proof does NOT establish VA hidden-surface
ownership or Chromium support, nor genuine system sleep. Next isolated diagnostic:
request existing explicit decode-order controls in native AV1 and record capture
completion payload/flags/timestamps, with strict unchanged display parity and
no retry. Host build/identity precedes any bounded hardware run.

LATEST OPERATOR DEVICE PM PASS (2026-10-02): pm-devices-packet.b86c3104.7krx481j
result PASS devices dry run,6.6598s exit0/no lingering/timed_out=false. Same boot
b86c3104/build231cb9f3, refcnt0/runtime suspended/usage0/auto and pm_test none
restored. This does NOT qualify platform sleep/post-wake. AV1 guarded independent
encoder hardware corpus now eligible; run prepared cases serially, stop first
failure, frozen identities/current reload/actual build/shared lease/clean kernel
required. Installed driver/module unchanged, AV1 profile still gated.

HOST AV1 HIDDEN COMPLETION AUDIT (2026-10-02): actual unchanged installed-source
HFI flag mapping + output handler + VB2 completion extracted into sanitized C host
model.8normal/NOSHOW/corrupt/overflow cases PASS. Even NOSHOW without corruption
and nonzero incoming payload loses both payload and timestamp, advances no capture
sequence, completes ERROR. Original source hashes match post-reload frozen kernel.
Tool tools/verify-iris-av1-noshow.py; evidence av1-noshow-completion-host with full
actual functions/identities. No candidate fix, no firmware valid-hidden-pixel proof.
This concretely blocks publishing hidden VA reference surfaces from original AV1
headers; native software-display DISCARD fix cannot supply those surfaces. Need
verified firmware decode-order hidden completion semantics before changing error
classification; actual corrupt/overflow errors must stay strict. No kernel/VA change
or hardware opens. Device PM operator result still pending; no repeated request.

HOST AV1 ORIGINAL TILE SPANS (2026-10-02): Extended actual CBS original-byte
probe to require tile-group pointers/sizes strictly within original OBU spans and
record OBU-relative offsets; verifier rejects missing/out-of-bounds tile spans.
Fresh av1-original-tile-host.ggbm0h1q PASS5software roundtrips: original MP4300,
aom/rav1e/SVT96each and 10bit aom96; all ordered full pixel hashes exact.
Original tile groups300/99/96/96/99, hidden/showexisting retained. Strict C build,
py_compile and diff-check PASS. Sources, exact linked libraries and manifest frozen.
This validates producer offset feasibility only: VA transport, hidden-surface
publication and Chromium AV1 remain unfinished/gated. Installed binaries unchanged.
Device-stage PM result still absent on same b86c3104/refcnt0; no hardware opens,
no repeated operator request. Continue host integration while pending.

HOST AV1 DEPTH CONTINUATION (2026-10-02): Original OBU verifier now preserves
source 10/12-bit comparison formats and refuses unknown formats instead of always
converting to 8-bit NV12. 12-bit acceptance is NOT tested/qualified. New host-only
libaom Main10 fixture av1-original-depth-host.d70ni99w PASS96 ordered full 10-bit
pixel hashes, original hidden42/showexisting39 preserved, parsed high_bitdepth=1.
Same verifier 8-bit libaom96 regression PASS unchanged NV12 hashes. py_compile PASS.
Frozen source/probe/fixture/results/trace manifest retained. No hardware decoder
opens or VA/module changes. This is host transport feasibility only, not AV1
hardware/Chromium support. Device-stage PM operator result still pending; no
unchanged request/retry. Continue actual producer/VA hidden ownership integration
and guarded hardware corpus after successful operator PM diagnostic.

HOST AV1 PRODUCER CONTINUATION (2026-10-02): Exact CBS original OBU retention
probe implemented producers/av1-cbs-original-obu-probe.c and reproducible verifier
tools/verify-av1-original-obus-host.py. No hardware decoder open/find_stream_info/VA
call. Requires entire packet unit coverage/contiguity and bounded original spans.
Host software roundtrip av1-original-obu-host-r2 PASS original MP4300 + libaom/
rav1e/SVT96each, pixel hashes and order exact, hidden/show-existing retained.
MP4 absent temporal-delimiter raw OBU demux attempts FAILED and preserved at
av1-original-obu-host; corrected host adapter uses IVF framing with original packet
units+original sequence extradata and original PTS, no sequence/frame synthesis.
Timing NOT qualified. This proves compression-header preservation feasibility,
NOT final VA transport/hidden-surface ownership or Chromium integration. Installed
VA/module unchanged, AV1 profile gated. Four host successes do not qualify hardware.
No new hardware corpus opens while operator device PM result pending. Existing
necessary operator command remains presented, do not repeat it unchanged.

Expanded AV1/system-sleep/controlled-live-removal scope INCOMPLETE. Firefox deferred.
Boot b86c3104-05ca-4e40-a913-9226ea801cfb runs exact persistent build
231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2; installed VA/module unchanged.
Idle operator module removal/reload and fresh post-reload full strict matrix,
churn/EOS/seeks/4K/Chromium/runtime-PM/null-output all PASS, all windows clean.
real-use run.u738ry1i:3120byteexact70.98s43.956FPS467568KiBRSS. Active-client
normal-removal refusal still pending; no force-removal support.
Operator freezer dry run PASS5.4915s/cleanexit/clean kernel/settings restored.
Device-stage operator packet pm-devices-packet.b86c3104.7krx481j is ready;
necessary command already presented; do NOT repeat unchanged sudo request.
No result yet. No agent sleep/unload/reboot. Prior ec980588 deep sleep FAIL/no
recorded resume/forced shutdown; cause unproven, no unchanged deep retry.
AV1 native fullstream300/300 pixel parity IN ORDER PASS with receive-core
DISCARD correction in av1-discard-core.b86c3104.7ky_w9qp, clean kernel/teardown.
Original299-frame and first-empty-recycle/internal-error failures preserved.
Actual-function host discard model PASS through10000empties/error propagation.
Native test alone is NOT Chromium/VAAPI production support; profile remains gated.
New HOST-ONLY diverse corpus av1-corpus-host.cgwv9g1s generated libaom/rav1e/SVT
8-bit720p96displayed frames each. Traced hidden42/45/45 and showexisting39/45/45,
varied refresh flags. No grain/superres/10bit/sequence-change coverage yet.
Frozen candidates/references av1-diverse-prepared.b86c3104.4qj9mp36; identities
and pending-PM guards PASS. No hardware corpus run; waits successful device-PM
result on same boot, then verify loaded identity/kernel/shared lease before each
case; stop at first failure. Inspect operator/test runners without competing.
FFmpeg parsed original sequence bytes are retained in seq_data_ref, current OBU
bytes are passed to start_frame; vaapi_av1 ignores originals. Need actual producer
and driver transport/hidden-surface ownership integration plus lifecycle/diverse
hardware proof before enabling AV1. No unused driver API or reserved-ABI abuse.
Host tests never qualify production. Stop hardware on any new kernel/firmware
fault; never open on three known faulted boots. No4K60/battery/zero-copy/kernel
physical-memory claims. Preserve all user changes/failures. All runners exited,
refcnt0; stay quiet while operator pending unless meaningful new progress.

## Completed recently
SCOPED INSTALLED PRODUCTION QUALIFICATION COMPLETE (2026-10-02).
Boot ec980588-64dd-447d-a29a-00e44e786ae8 automatically loaded persistent
candidate build231cb9f3a0141c3ddfa7b8df87df0889eff2f5f2, module SHAa604eda3...230aa,
VA unchanged v15 SHAa1cbbb6b...50ff. Read-only cold identity PASS.
Fresh installed-routing.ec980588.fdr663c_/production.69Uych full strict gate
PASS matrix/graphics/resolution/long-codecs/churn/EOS/seeks, clean kernel.
real-use/run.mpwcqztq scoped PASS:4K3180byteexact77.47s41.048FPS491152KiB
(<512MiB), Chromium playback/seek/cleanexit PASS. Ordinary runtime PM before/
after PASS active then suspended usage0/auto. Null-output regression PASS600
frames/exit0/no lingering children/complete clean kernel window.
Actual installed launch-chromium.py persistent profile smoke r2 PASS759frames
25.0005s30.359FPS781hardwarecompletions/seek/cleanexit/clean kernel. All runners
exited, refcnt0, hardware lease free. Release evidence saved bundle/v15/
installed-qualification.json. Kernel/device/VA identities verified; no more
privileged actions needed. Root installer state remains its original successful
installed_pending_cold_boot transaction label; independent user-readable cold
verification/qualification records establish actual completion, no root state edit.
Early boot renumbered Iris decoder video16->video0; launcher now resolves unique
sysfs qcom-iris-decoder with hash-checked selector and exports V4L2_VA_DEVICE.
Headless clients must set same selector-derived override as documented. Original
wrong-node vainfo gate failure installed-boot.ec980588.z_e5zt05 retained clean;
launcher initial smoke blocked missing execute permission retained; chmod fixed
and fresh r2 real smoke PASS. Driver and kernel binaries unchanged.
Supported scope ONLY opt-in Chromium and headless H264/HEVC/VP9. Firefox,
AV1, system sleep and live module removal remain unsupported/unqualified.
No4K60/battery/end-to-endzero-copy/separately-measured-kernel-memory claim.
Backup/rollback remains documented; original distro module/GRUB/default profiles
preserved. Qualification heartbeat stopped after scoped completion.

Historical installed/preparation statuses below are superseded by the above.

Installer R2 operator completed. Independent read-only checks confirm persistent
module selection is updates/libva-v4l2-production/qcom-iris.ko with exact SHA
 a604eda3918b0f8f8d3422d8a5dd53268792a93b59bb5ef9c9bcd9cf228230aa,
and candidate path is present in current initramfs; explicit hook exists.
Current boot5d252b15 still runs original buildd32b38c1...da8a6b. No live unload
or decoder open. Operator normal restart and read-only verify-cold-boot.py next;
installed-boot hardware qualification remains pending. No further sudo requested.

- Installer r1 operator FAIL: candidate missing from initramfs because generic
  image builder excludes media drivers; omitted explicit Iris hook. Root restore
  returned selection to original; override absent, original initrd SHA7ff09fa2...
  eaae5141. Full root backup/state unreadable unprivileged. R2 verifies prior
  rolled_back state plus exact backup/current image+metadata before mutation,
  retains r1 backup, uses fresh backup v15-a1cbbb6b-r2. Adds version-gated
  manual_add_modules Iris hook; rollback removes owned hook.6host tests PASS,
  real staged depmod/dracut-install candidate/dependency copy PASS exact SHA
  (initramfs-r2-rehearsal._yb4c8ft). No r2 privileged writes: sudo-n refused
  interactive authentication. Necessary terminal retry presented; no restart
  until corrected installer succeeds. No decoder opens/live module changes.


- Scoped persistent installation bundle PREPARED at
  /home/mq/snap/chromium/common/libva-v4l2-production/v15/INSTALL.txt.
  No privileged install or boot writes yet. Boot changed to5d252b15-eafd-43a6-
  8d71-85303f9673de; actual original buildd32b38c1...da8a6b loaded/refcnt0.
  No decoder opens on this new boot. Candidate v15 successes remain bound to
  prior8558e0e1. Installer preflight PASS;4transaction fixture tests PASS
  (failed-initrd rollback, unfinished-writer refusal, success/rollback,
  corrupt-backup refusal). Boot override preserves original module; backs up
  initrd/metadata/original, verifies candidate inside new initrd. Never live
  loads/unloads/reboots. Opt-in Chromium launcher uses separate profile and
  refuses unexpected loaded module. Operator install/normal restart then
  read-only cold identity verification required before persistent qualification.


- V13 ACTUAL scoped runtime PASS: boot8558e0e1, loadedbuild231cb9f3...2f5f2,
  cold-boot.Zl7IlH activation PASS. Gate production.NKVdIq full strict PASS;
  run.txbs4s6m real-use PASS scope=chromium-headless.4K3540byteexact69.88s,
  50.658FPS/479708KiBRSS PASS; Chromium30.318FPS/seek/cleanexit PASS.
  Runtime PM witnesses qualification.htORNl PASS: suspended before, active
  then suspended after; active102952->225840ms/suspended73148->78045ms,
  usage0/auto, same boot/build. All kernel windows clean. Operator exited.
  Runtime PM fix observed working; system sleep/live unload and persistent
  deployment remain unqualified. Do not rerun cold activation while live.
  User asks why4K below60FPS: compare identical600frame CPU-download with/
  without framemd5; diagnostic only, no4K60playback claim. Evidence
  4k-hash-cost._scxn3nv. Hash arm600frames13.35s44.94FPS; no-hash null
  output arm FAILED with10s vaSyncSurface timeout at212frames and IO errors,
  experiment observer terminated45s. Processes exited/refcnt0, independent
  journal no kernel/firmware fault; no completed observer qualification window.
  Cannot isolate hashing cost or claim60FPS. Investigate host lifecycle/drain
  before further diagnostic hardware; preserve all failures and strict gates.

- V13 kernel preparation COMPLETED and superseded by actual runtime results
  above: patch0005 in recovery-v13/packet.20vamb24. Original suspend deadlock
  reproduced;28source-model scenarios/10000cycles,152tooling,W=1build,
  153importCRCs and same-path reproducibility PASS. Module SHAa604eda3...230aa,
  buildID231cb9f3...2f5f2; VA SHA8d85fbfc...4cd69. No operator runner active;
  no additional activation or reboot requested. Persistent installation remains
  pending. Full details: docs/production-kernel-pm-fix-20261002.txt.
- ACTIVE v15 natural-GOP grace candidate: actual-submit-frame host model
  with delayed20ms completion reproduces v14 premature STOP at11.176ms,
  fifo1/STOPfailure1. Candidate100ms elapsed grace waits20.641ms until fifo0,
  sends no STOP. Evidence keyframe-grace-model.eoy2kvrg baseline FAIL/candidate
  PASS; injected pump completion model, not hardware/firmware proof.
  Only natural-keyframe grace changed; ownership and compatibility seek drain
  unchanged.228Rust/strictClippy/frozen identity PASS. Fresh v15 packet
  recovery-v15/packet.8mtqzw6u VAa1cbbb6b...50ff reuses exact live v13 kernel,
  current-boot activation/build checked. Full strict gate PASS production.jn0UWB,
  all required matrix/churn/EOS/seeks/clean windows, exec83144 exited0.
  V15 scoped real-use PASS run.x3kheaho:4K3180byteexact86.93s36.581FPS490544KiB;
  Chromium757frames25.0004s30.2795FPS781completions/seek/cleanexit.
  Before/after PM PASS sameboot/build active then suspended usage0/auto.
  Null-output regression7qeohvdy PASS600frames/7.664s, exit0/no lingering,
  complete kernel window clean; module refcnt0. Exec83394 exited0.
  Natural-GOP grace candidate resolves this reproduced regression; no general
  firmware-cause/4K60/battery claim. Persistent deployment and system-sleep/
  live-unload qualification still pending; prepare reversible scoped install.
- ACTIVE userspace pacing fix, not hardware qualified: actual submit_frame
  host reproducer on permanently writable /dev/null exhausts2500polls in
  2.477656ms (output-pacing-host.k9jgjefw/actual-function.log). Ordinary
  OUTPUT waits now share an elapsed5sdeadline and yield1ms on no progress;
  queue limits/allocations and drain semantics unchanged.228Rust tests,
  232integrated host tests,strictClippy/release/frozen identity PASS. Actual
  candidate submit_frame reproducer waits5.000077s instead of2.477656ms.
  Frozen v14 recovery-v14/packet.sgawj23n VA60d19057...f3081 reuses unchanged
  live v13 kernel and successful same-boot activation. Full strict gate now
  PASS production.e7ss6l: required matrix,session churn,EOS/drain,seeks and
  complete clean kernel windows; final identity PASS. Exec session17492 exited0.
  Scoped real-use PASS run.jnj6owfo:4K3060byteexact69.5s44.0288FPS486832KiB;
  Chromium759frames/25.0004s30.3595FPS780completions/seek/cleanexit;
  all complete kernel windows clean. Before/after runtime PM PASS sameboot/build,
  after active then suspended usage0/auto. Exec38158 exited0.
  Bounded null-output regression FAIL exec44866 exited1: same212frames,
  OUTPUT pacing stall after natural keyframe STOP/START, inner timeout124.
  Completed observer window clean, but child FFmpeg remained after timeout;
  private PID69657 required explicit TERM cleanup; then exited, refcnt0 verified.
  observer completion alone does not prove teardown.
  Evidence v14-null-output-regression.45yi30c_/result.json and no-hash.log.
  The elapsed wait fix does NOT resolve the post-START stall. Do not rerun this
  unchanged failure or deploy persistently. Investigate host drain/resume and
  firmware/kernel buffer lifetime next; no new kernel fault or restart needed.
- Host-only null-output failure analysis: hash arm has0compatibility STOP/START;
  failed arm has2STOP/1START/2pacing stalls. First STOP at natural keyframe
  seq210; LAST dequeued before START. Five OUTPUT submissions then3OUTPUT and
  3CAPTURE completions precede seq215 pacing stall (out2/4,cap6/20,fifo2).
  Investigate GOP drain/resume and poll-call pacing bounds; cause UNPROVEN.
  Machine-readable evidence:4k-hash-cost._scxn3nv/drain-sequence-analysis.json.
  No allocation/driver change or hardware open made for this host analysis.

- User scope decision (2026-10-02): defer Firefox for this release. Supported
  qualification scope is Chromium and headless H.264/HEVC/VP9 decoding; AV1
  remains unadvertised. Firefox is unsupported/deferred, with its failed
  evidence preserved. Do not relabel the original full real-use FAIL as PASS.
  Continue kernel PM/lifecycle assessment and reversible deployment preparation;
  retain all correctness, memory, performance and kernel safety requirements.

- Current root owner (2026-10-02): v12 runtime investigation on fresh
  boot b234b7a2-eb8f-4b27-a7fe-4d3a1d7552ab, production UNQUALIFIED.
  Frozen identity and activation/current loaded build PASS. Operator owns the
  hardware lease during its completed run. Gate production.9ogeIz, wrapper
  evidence qualification.XCnWZU. Full headless gate now PASS, clean kernels.
  Real-use run.b9pg99be sustained4K3180byteexact69.61s/45.68FPS/478680KiBRSS
  PASS60s/30FPS/512MiB. Chromium PASS30.27FPS/cleanexit. Firefox cleanexit
  but FAILdrops9/742=1.21%; identical diagnostic repeat11/744=1.48%, all11
  before5s. No kernel faults. Full real-use stays FAILED. Software-decode
  private control also11/745drops,allbefore5s,IsHardwareAccelerated=false
  and0driver messages. Shared Firefox startup issue suspected, not proven.
  Software timeline diagnostic6/761 drops, all by0.702s, visible page and
  sampled animation gaps17ms; no driver messages. Evidence
  firefox-startup-timeline.70j0g6wh/findings.txt. Variable startup drops remain
  unexplained; diagnostic success cannot replace failed hardware qualification.
  All runners now exited; kernel windows clean. Firefox investigation deferred
  by the user; continue kernel lifecycle and deployment requirements.
  Boot311d78af remains forbidden after v9 emitted5SESSION-FATAL in VA sample30;
  c5ace5e4 and4492e975 also excluded (full IDs in cold runner).
- Historical pre-v13 preparation: boot8558e0e1 initially had Iris absent.
  Superseded by the matching successful v13 activation/runtime results above.
- Scoped kernel PM review: host concurrency model reproduced a lock cycle
  using actual iris_pm_suspend/iris_hfi_pm_suspend functions from the matching
  candidate source. Suspend holds core->lock; power-off waits in disable_irq;
  a queued IRQ handler needs that lock. Evidence
  resume-20261002/scoped-pm-review.ne8c5e23/review.txt. This is a modeled race,
  not a new hardware fault. Removal patch0003 does not address this PM path.
  Prepare an IRQ/PM synchronization fix and host race tests before considering
  persistent kernel deployment; do not simply drop IRQ synchronization or
  release the mutex around hardware teardown without handling races.
- v12 packet: resume-20261002/recovery-v12/packet.wlhanl45. VA SHA256
  4a7a39bff1e146143390f5b10ca94246fa345a004e7568c8dd502243640d2d26.
  Changes: lazy CAPTURE mappings; Arc<Vec<u8>> immutable snapshot aliases;
  CPU pool queries firmware minimum only AFTER initial SOURCE_CHANGE event,
  targets max(20, minimum+6). Invalid/failed controls fail setup. Predecode
  exports and recovery preserve CAP32; independently passed OUT4 retained.
  Export pressure appends4 via CREATE_BUFS while preserving six working slots,
  existing mappings/reservations/export accounting. No shrinking live exports.
  Runtime evidence: full headless and sustained 4K gates PASS; process RSS
  fell from v8 803260KiB to 478680KiB. Reserved kernel memory unmeasured.
  Scoped production remains UNQUALIFIED pending kernel PM/lifecycle and
  persistent deployment. Firefox is explicitly deferred, not passed.
- Initial DMA snapshots and stable export publication copies remain necessary:
  firmware selects working targets and late readers can retain old frames.
  Arc aliases share pixels and survive owner replacement/drop; writable images
  remain independent. Lazy mappings reduce process accounting, not kernel
  reservation; pool reduction may reduce allocation if hardware proves it safe.
- Host:226Rust,230integratedstress,149tooling,strictClippy/fmt/release PASS.
  Native Linux headers confirm G_CTRL/CREATE_BUFS ABI. Mapping-tail failure
  preserves queued state, pointer identity and held exports. Frozen source,
  driver and kernel artifact identities PASS. Host preparation performed no
  hardware opens; subsequent v12 hardware results are recorded above.
- Same qualify-cold-order-reviewed.sh now points to v12; original v11 wrappers
  saved. Current faulted-boot rejection actually PASS before sudo. Full next
  step and rollback in docs/production-next-cold-20261002.txt. Cold restart and
  operator activation now complete; matching boot/build independently verified.
- Historical actual v7/v8 strict full gates PASS; v8 Chromium PASS. v8 4K300
  byteexact44.64FPS/803260KiBRSS FAIL512MiB. Firefox correct seek/hardware but
  timeout124: sessionstore proves extra Mozilla first-run privacy tab. Private
  firstRunURL suppressed; user closed old test window. Browser thresholds strict.
  v9 CAP8/growth failed and source reverted; immutable failed evidence preserved.
  Historical gates do not qualify v12. Its own pixel/lifecycle/kernel/4K gates
  now PASS; Firefox remains FAILED. Acquire the hardware lease for further probes.
- Reviewed temporary kernel SHA10e15278...f3af1/buildID372e5204...9b883 unchanged.
  Installed original module SHA9836e09e...dc4c unchanged. No live unload, forced
  removal, persistent install, boot edit or agent reboot. Cold rollback via
  ordinary reboot WITHOUT blacklist restores original (known defects retained).
- Heartbeat finish-libva-production-qualification ACTIVE every10minutes, updated
  to v12; quiet until meaningful progress or required operator action. Inspect
  an active operator runner without competing hardware. Disable only on genuine
  completion. Sole ownership confirmed; preserve all existing user changes.

## Historical recovery notes (superseded by the current task above)

- HARDWARE STOP: initial packet.hxix0_32/production.oeTqVg passed CPU1/30/300,
  full GL300 and resolution780, then FAILED rc141 before long decode; outer
  window reports both Iris index32 UBSAN reads. Original results preserved.
- User-started activation17:25 deadlocked removing the original module.
  On the old boot Python38033/modprobe38044 and IRQ712 were blocked/live;
  Iris was Unloading/refcnt-1. Candidate NEVER inserted. User reboot recovered
  that state; do not repeat live activation/rollback or forced removal.
  Hung-task stacks prove disable_irq/core-lock cycle. GDM's17:27:32 Mesa abort
  is separately recorded; causality remains unproven.
- Prepared bounds+removal candidate (0001+0003, excluding PSC0002): build/model
  PASS, all152imported CRCs PASS. SHA256 b333a5b6...a29bc, build ID
  de5d3d265214e6c64edddf31a4040983abc46288, srcversion8057268C3554915BE16E951.
  Removal source model reproduces original deadlock and patched4scenarios PASS.
  General PM power-off lock ordering remains unresolved; no runtime proof.
- Host fixes: per-session kernel/transition fail-fast, ffprobe producer drain,
  boot/module/fixture provenance, generated distinct-ID seek fixture, activation
  phase journal and bounded cleanup with unfinished-PID evidence/no rollback race.
  Latest tooling147/147PASS (24activation regressions), source models PASS.
  Logs under ~/.cache/libva-v4l2-qualification/resume-20261001/; latest tests
  recovery-v4-host-tests.log. Shell syntax and git diff --check PASS.
- Latest immutable packet: resume-20261001/recovery-v4/packet.cuishazj.
  Identity PASS; same Rust driver SHA5f79f6...eab98. Reviewed module artifacts
  copied into kernel-artifacts/ with SHA manifest. Current hardware gate in progress; qualification not yet established.
  Prior recovery-v3/final-v2/final packets predate guards/journal and remain historical.
- Cold wrapper: resume-20261001/activate-cold-reviewed.sh. Faulted-boot rejection
  PASS before sudo. User asked to save work/reboot once with
  modprobe.blacklist=qcom_iris, then run wrapper. It requires absent Iris and
  interactive sudo. Guide: docs/production-cold-boot-20261001.txt.
  Activation now PASS with exact identity, boot and unchanged installed module
  verified. Frozen strict gate running; only clean pass permits real-use runner.
  Do not treat source/ABI checks as hardware qualification. Remaining runtime
  matrix/lifecycle/sustained4K/browser and persistent deployment pending.
  AV1 remains disabled pending authoritative producer metadata/full parity.
  Full report: docs/production-resumption-20261001.txt.

## Previous handoffs (historical)

- Production resumption (root, 2026-10-01 17:00 local): user requested all
  pending work through completion. Current decoder/DRM nodes available, lease
  free, no qualification/decode processes observed. Freeze current source with
  stage-production-qualification.py; run strict gate on immutable identity.
  Preserve old failures, ownership guards, required parity and performance
  budgets. Coordinate with any external editors before shared Rust changes.
  No system installation, module activation or reboot authorized by this run.

- CPU capture fallback audit finished (test_4k child, 2026-10-01): repaired
  external6-slot candidate now passes CPU1/30/300 per root; owner continues
  GL/resolution gate. Isolated31+1 fallback built/host219/Clippy/fmt pass but
  NEVER hardware executed, no shared Rust changes. Preserve unrun artifacts
  ~/.cache/libva-v4l2-qualification/cpu-export-window-candidate; patch/provenance
  and actual failure/pass chronology in docs/pending-lane-performance.txt.
  No fallback test while current owner lease/gate is active; latest replenishment
  repair supersedes the hypothesis that6slots necessarily require enlargement.

- Seek/EOS integration update to review root (2026-10-01): submit.rs now
  replenishes the full bounded CPU working pool on every streaming submission,
  instead of only requeueing the recycled target's old slot. This addresses
  the queue-policy interaction; no capture.rs edits in this lane. H264 now
  preserves slice PPS IDs and recognizes IDR25/45/65, rejecting05. Host checks
  pass. packet.dojf_84g failed HOST checks only: it captured a transient syntax
  error in test_4k_thread_scopes.py (already corrected by its owner) and an old
  diagnostic assertion in test_decode_quality.py (updated here to assert the
  new exact fail-fast status9/expected0 result). No hardware opened for that
  packet. eufa1nd_/xrSLy6 still failed CPU30 clean: diagnostic cpu30-debug.e8sxkg
  traced CAPTURE starvation during pre-IDR drain/replay, whose top-up was still
  stable-capture-only. Extended both to streaming CPU queues; retain ownership
  and replay failure guards. Rust219/fmt and strict Clippy pass. Current gate:
  igmkrc1v CPU1/30/300 pass; GL CPU reference times out, clean kernel. CPU
  polling now replenishes working CAPTURE while a sync/download blocks its
  submitter; it latches a QBUF failure rather than publishing success. Current:
  packet.9_hlndl8 SHA5f79f6e4e7e430f162658746f3f349520af3b50f0af2ed44d8a344340f1eab98.
  Supplied distinct-ID low fixture; required counts and stronger controller
  unchanged. Software-only seek trace proves original colliding SPS/PPS IDs
  cause37 syntax errors versus0 with distinct IDs at the same3seek targets.
  No default fixture change until hardware proves the new ID support. Preserve
  strong seek/kernel validators; do not start another lane while gate owns lease.

- Integration regression handoff (review root, 2026-10-01): inspected external
  packet.377t6a0p / production.ZIYIfL: new dc9739...ca63 candidate passes
  sample1 but FAILS required sample30 with status251; all kernel counters0.
  Root export agent now auditing bounded legacy CAPTURE policy against the
  previously clean b2e7ce candidate. External owner retains submit/H264 fixes;
  no competing edits there. Do not claim final build qualified. Earlier root
  baseline small-stream STOP was followed by external 15:27-15:28 clean full
  matrix/Bframes150/churn/EOS on b2e7ce; it is baseline-specific evidence.
  Root does not repeat crashing baseline fixture. New source needs full gate.

- 4K memory source audit HOST FIX COMPLETE, hardware measurement PENDING
  (performance child, 2026-10-01): changed only
  `tools/verify-4k-decode.sh`, a focused argv regression under `tools/tests`,
  and performance report updates. Existing logs show17 published surfaces in
  the1-frame leg; output-only `-threads:v1` does not bound input decoder
  threading. Independent input/output limits now covered by a before-failing,
  after-passing host argv regression across all4codecs/1,30,full legs;
  preserve512MiB budget. Allocation/lifetime model and bounded smaps follow-up
  recorded in `docs/pending-lane-performance.txt`; no inferred RSS/leak claim.
  No capture/export/codec/recovery edits and no hardware opens after STOP.

- Firefox export ownership host fix complete; hardware qualification PENDING
  (test_4k child, 2026-10-01): only `rust/src/v4l2/capture.rs` changed.
  Keep CPU working queue bounded before late exports, restore valid snapshots
  when their old slot is a foreign reservation, and reuse existing reservations
  idempotently. Three new regressions fail before/pass after; Rust212, host
  stress216, strict Clippy and fmt pass. No codec/submit/recovery/replay edits,
  no hardware opens after STOP. CPU/GL/churn/Firefox qualification of the changed
  queue policy still required; details `docs/pending-lane-performance.txt`.

- HARDWARE STOP handoff (review root, 2026-10-01 15:26 local (UTC+7)): VA small
  B-frame probe on baseline fc082 produced0 output and five session-fatal
  0x4000003 messages, exit251. Root STOPPED further hardware opens. Evidence
  ~/.cache/libva-v4l2-qualification/smallstream-va-20261001-root-1/driver-small.log.
  Preceding native/VA720p checkpoints passed clean; native-small independently
  produced one zero frame with no kernel fault. Do not automatically retry
  this fixture or open another decoder after this fault. External seek owner
  must account for this shared-device state before any further hardware test.

- Six-lane root hardware evidence (2026-10-01): exact baseline fc082 binary
  one-frame native and VA both pass byte parity with clean kernel windows
  (`~/.cache/libva-v4l2-qualification/eos-20261001-root-1`). Native small
  B-frame fixture fails: one all-zero output instead of150, command0 and
  all seven kernel counters0; 720p native/VA baselines pass. Separate VA
  small-stream diagnostic now scheduled. Performance run.O6rYy2 completed:
  4K1/30/300 byte parity passes, 512MiB budget fails, sustained skipped;
  Chromium video not ready, Firefox timeout124. These are baseline-only
  results, not qualification of external owner's new seek/restart changes.
  Matching kernel review artifact compiled; no module installed or loaded.

- Seek peer-audit handoff (root's six-agent lane1, 2026-10-01): read-only
  findings for external Seek/EOS owner; no competing Rust edits. In
  `codec/h264.rs::finish_picture`, exact Annex-B header0x65 recognition misses
  reference-IDR0x25/0x45. Add finish_picture regressions for all3nonzero ref-idc
  headers (and non-IDR0x41/invalid0x05), use NAL type/ref-idc masking; preserve
  true random-access requirements. `mpv_seek_drive.py` accepts constant finite
  time-pos1.25 after every accepted seek: add stuck-position/stuck-seeking
  regressions, require bounded seek completion plus real hardware playback
  progress without assuming exact keyframe landing. Details and root-only
  frozen candidate executor: `docs/pending-lane-seek.txt` and
  `/tmp/run-frozen-seek-20261001.sh`. Current52V4L2/12IPC tests pass;
  historical fc082seek failure remains failure, fresh candidate outcome pending.

- Six-agent pending-work sweep (review root, 2026-10-01, explicitly requested):
  reactivated six scoped workers in waves: gl=seek peer review (external active
  thread retains Rust edit ownership), qualification=one-frame EOS,
  recovery=small B-frame streams, test_4k=4K/browser performance,
  kernel=matching source/boot qualification preparation, test_production=final
  immutable gate. Older child namespaces retain masked devices; root now has
  direct hardware/full access and centrally executes their bounded scripts.
  All probes use the shared lease and disk-backed outputs; stop first kernel
  fault. Do not duplicate the active Seek/EOS fix owner's Rust edits. Child
  reports go in docs/pending-lane-*.txt; root updates this handoff. No kernel
  installation or reboot in this sweep.

- Seek/EOS fix (root, 2026-10-01, user authorized): claiming H.264 codec
  random-access/timestamp handling and V4L2 recovery as evidence requires.
  Preserve concurrent edits, fail-closed ownership guards and required parity
  matrix. Hardware directly accessible; serialize every probe. New frozen
  frozen source /tmp/libva-seek-small-source-20261001 now includes bounded
  pending-surface reuse, unique completion cookies, pre-IDR drain, explicit
  errors for drain-discarded owners and CAPTURE-cycle DRC for CPU sessions.
  One-frame isolated VA parity passes against complete software output.
  Fx2fKe passed CPU/GL but failed resolution 63/780 after repeated STOP/START,
  kernel/sanity clean. Added bounded completion wait before pre-IDR STOP;
  Rust209/fmt/strict Clippy pass. New disk-backed immutable candidate:
  /home/mq/.cache/libva-v4l2-qualification/seek-eos/packet.z5bonbls
  SHA256 b2e7ce849994a5b44deb10962a2c577697d6ae6f790e5df1d2d41370f810ba0c.
  b2e7ce gate passed CPU/GL/resolution780/long3600/codecs, both strict small
  edges, churn7/7 and EOS, all kernel counters zero. This later new-binary
  evidence accounts for the baseline small-stream STOP handoff above; the
  failing old-binary small probe was not retried. Short IPC path fixed the
  disk-backed seek setup; 24 seeks pass, mixed seek still fails with frontend
  parser corruption before driver calls. New PPS-ID preservation and masked
  IDR recognition added; concurrent CPU queue bound also required submit-side
  full working-pool replenishment. New immutable packet.dojf_84g SHA5cb030...77e
  is running the complete gate with a supplied distinct-ID960x640 fixture,
  unchanged resolution/seek counts and strengthened peer-owned seek verifier.
  Other hardware lanes must wait for the common lease.
  Previous V5QP6a attempt stopped at hardware_in_use (no hardware tests),
  fresh attempt launched after lease recheck. Details:
  docs/production-seek-eos-fixes-20261001.txt. Mixed seek still unqualified.

- Production continuation BLOCKERS (root, 2026-10-01): hardware now accessible;
  direct exact-binary runs completed. e1tXqa passes CPU1/30/300, strict GL300,
  resolution780, long3600, HEVC/Main10/VP9 with clean kernel; production FAILS
  native one-frame EOS (zero frames). Independent jezSwc churn7/7 and EOS pass.
  Seek startup race fixed/tested; corrected seek PIvRqk and debug svZYKG FAIL:
  empty CAPTURE with pending owners, recovery fifo mismatch, then hwdec-current
  unavailable. Clean kernels/sanity, no complete seek storm; mixed seek unrun.
  Preserve fail-closed guard. Next: timestamp/reference reset and recovery of
  consumed OUTPUT dependencies; future Rust changes need new binary/full gates.
  All runs used fc08226...a6e10; lease free. Report:
  `docs/production-host-continuation-20261001.txt`. Earlier absent-device notes
  describe previous environments and no longer block direct execution here.

- Post-GL qualification delegation (root, 2026-10-01): user requested agents
  run production, 4K and browser/kernel tests. Production/4K children completed;
  root handled browser/kernel after agent thread cap blocked another worker.
  All preserve exact GL-passing binary fc08226...a6e10 from 5HPJnr and source
  /tmp/libva-gl-last-source-z88fqfmg. Source driver edits remain with external
  hardware owner; this lane changed reports/temporary runners only. Results
  are recorded under Completed recently. Hardware tests cannot run in root's
  sandbox; use prepared exact-binary host runners when the shared lease is free.

- Host GL follow-up (this session, 2026-10-01): strict GL now PASSES
  300/300 ordered frames with a clean kernel window on fixed source
  `/tmp/libva-gl-last-source-z88fqfmg`; user-run evidence
  `/tmp/libva-host-gl-check.5HPJnr`. START now waits for userspace dequeue
  of STOP's LAST buffer, avoiding vb2's late-LAST stopped-state race.
  Buffer flags confirm LAST (0x104001) precedes START. Host checks on this
  snapshot: 204 Rust tests, fmt, strict Clippy, release build pass.
  Full matrix/churn/EOS/seek remain pending on this same source.
  Details: `docs/production-host-gl-followup-20261001.txt`.

- Production-readiness hardware qualification (2026-10-01, this session):
  source fixes and host checks complete; see `docs/production-fixes-20261001.txt`.
  BLOCKED here: decoder/DRM nodes are absent. Required matrix/churn and production
  gate were attempted and failed hardware access, not code checks. New GL
  completion/cache changes need strict full-stream and lifecycle hardware runs;
  keep kernel boot, small-stream, 4K/browser and experimental AV1 blockers open.

- Top-down review handoff to the active **Continue libav support** agent
  (user-requested, 2026-10-01): this review session is no longer running
  code changes or hardware probes; do not assume an active parallel GL owner.
  Direct thread delivery was blocked by the tool approval requirement.
  Please incorporate `docs/production-review.txt` and the historical evidence
  in `/tmp/libva-v4l2-production-review-results/`. The review's final CPU-copy
  run matched all 300 native H.264 frames with a clean kernel window. Earlier
  GL coverage missed one reference frame; a later run emitted eight unmatched
  frames, and `gl-diagnostic.log` showed GStreamer recycling pending surfaces
  (`vaBeginPicture: surface is in use`). Those results predate your newer
  integration fixes and must not be treated as current-revision validation.
  Late exports now copy dequeue-time snapshots instead of firmware-owned
  queued mappings. Preserve the strict release/verifier checks while resolving
  completion synchronization and pool reuse. Your newer SOURCE_CHANGE ERROR
  marker handling and provisional Main10 negotiation notes are acknowledged;
  this session will not overwrite them. Remaining GL/kernel/browser/AV1 work
  needs an explicit owner; this session relinquishes its top-down lane.

- Bottom-up next task (codex, 2026-10-01): AV1 authoritative refresh/sequence
  metadata remains necessary for full-stream parity. Strict GL missing-frame
  coverage and browser throughput stay in the parallel top-down lane. H.264,
  HEVC/Main10, and VP9 4K correctness and 4K H.264 churn are verified below.
  Small H.264 follow-up: bounded probe complete; firmware blocker recorded.
  Completed in `6ef214a`: kernel-fault and repeated-frame verification; candidate
  Iris metadata-index patch with an isolated UBSAN reproducer. Claiming
  `capture-iris-kernel-log.sh`, `verify-4k-decode.sh`, standalone quality tests,
  and `verify-session-churn.sh` (its old self-reference and ignored FFmpeg
  status could falsely certify partial/software playback). Claiming `v4l2/submit.rs`: drain replay occurred after
  the free-OUTPUT check and could fill every slot before new-frame QBUF.
  Resume before pacing, and skip replay when a fresh keyframe replaces the
  old reference chain. Browser/compositor throughput stays top-down.
  CPU image-read allocation reduction complete: borrow the published
  snapshot in `vaGetImage`; copy only the declared layout for independent
  `vaDeriveImage` storage. Tests moved into `image/tests.rs` and cover NV12,
  P010, immutable snapshots, and mapped-image lifetime after surface deletion.
  4K checksum output now uses one rawvideo encoder thread; prior RSS may
  include the encoder frame backlog and is not evidence of a driver leak.
  Reference-chain guard implemented in `v4l2/replay.rs` and `submit.rs`;
  only the early validation and first-keyframe flag hunks in `recovery.rs`
  are owned here. Reject incomplete GOPs and FIFO/payload mismatches before
  START/reopen; preserve hidden reference pictures in decode order.
  Isolated staged source: 169 Rust tests, strict clippy and fmt pass.
  Hardware validation BLOCKED: renderD128/video16 do not exist here; matrix
  and churn both stop at initialization. Do not call this hardware-verified.
  Next bottom-up: bounded complete-GOP retention and rebuilding consumed
  references; the 64-chunk history remains a reliability limit.
  Next: kernel boot validation, strict GL owner fix, authoritative AV1
  metadata, and fresh bounded-buffer 4K/browser performance measurements.
  A hardening guard in
  `poll.rs` rejected Iris's empty ERROR-marked SOURCE_CHANGE completion
  before existing marker handling could run. Restrict ERROR rejection to
  nonempty pixels; empty completions still follow drain/source-change/fatal
  classification. Leave this integration hunk with the queue owner.
  Coordinate future runs using
  `/tmp/libva-v4l2-hardware.lock` and this file. Integration note for the queue
  hardening owner: preserve `setup.rs`'s provisional Main10 NV12 exception;
  final P010 is validated before STREAMON. The temporary borrow-check fix in
  `capture.rs` is also part of the parallel owner's working changes.

- Post-merge production hardening / agent split (codex agent, 2026-09-20): continue on `main` after merging Phase 4/5 at `fd2987b`. Phase 4 is complete for the covered gates: H.264 sample-1/30/full, mixed-resolution CPU-copy, long playback, seek stress, and repeated-open lifecycle all have passing evidence. Phase 5 is complete for HEVC Main, HEVC Main10, and VP9 Profile 0; AV1 remains intentionally hidden until the missing OBU synthesis exists.

- Bottom-up AV1 lane (codex agent, 2026-09-20): work from the existing raw AV1
  VA buffer collector upward. Scope is OBU synthesis and tests first; do not
  advertise `VAProfileAV1Profile0` until `tools/verify-codec-expansion.sh`
  passes an AV1 reference-parity leg.

  Parallel-safe work items for other agents:
  1. Firmware/small-stream lane: keep `bframes-240p` as an expected xfail, gather root-only `qcom_iris` dynamic_debug/HFI traces for a failing small stream versus passing 720p, and update `docs/08-iris-firmware-errors.md`. Do not weaken the required 720p matrix.
  2. GL/export verifier lane: keep the now-hard `tools/verify-gl-roundtrip.sh` gate green and improve diagnostics around tolerated gst-va pool warmup frames. Keep `verify-rust-driver.sh` and `verify-session-churn.sh` green after any V4L2 queue/export/teardown change.
  3. Browser/client lane: rerun `tools/verify-browser-vaapi.sh` on clean hardware after export or pool changes, add an unconfined Firefox path if available, and implement only callbacks/importer behavior that browser logs prove are required.
  4. AV1 lane: synthesize the missing temporal delimiter, sequence, and frame OBU headers before advertising AV1; current evidence shows the conformance sample has a 41-byte prefix before the first VA tile payload.
  5. Rust cleanup lane: keep reducing oversized modules around surface lifecycle, VA entrypoints, codec parsing/synthesis, and V4L2 backend boundaries while preserving current verifier behavior.

- HEVC parameter-set parser split (claude agent, 2026-09-20): claiming
  parallel-safe item 5 for one bounded refactor — move the
  `profile_tier_level`/SPS/VPS/PPS parsers and their tests from
  `rust/src/h265.rs` (1202 lines) into `rust/src/h265/parse.rs`. The NAL
  model, Annex-B assembly, and the `synth` re-exports stay in the parent.
  Codex files (`codec/raw.rs`, `config.rs`, `av1/*`) are untouched; this is
  a host-only change.

- Post-decode PRIME export stabilization + unconfined Firefox reach (claude
  agent, 2026-09-20): claiming the unconfined-browser gap. Evidence from the
  unconfined Firefox 156 aarch64 tarball (`/home/mq/apps/firefox`, snap-free,
  no root): the driver loads in RDD, VA-API FFmpeg init succeeds, but
  `GetVAAPISurfaceDescriptor` exported only through the frame callback, and
  `export_ready_surface` rejected every Ready-surface export from a
  legacy-flow session (`Ready && !stable_capture -> OperationFailed`), which
  tore the VA-API decoder down to software after one frame. Fix, entirely in
  driver code owned by neither codex's AV1 lane nor the gst/Chromium warmup
  path: (1) `V4l2Session::stabilize_published_capture` reserves a stable
  slot at export time and copies the completed frame into it (or adopts the
  still-Free published slot in place), reusing `reserve_capture` +
  `copy_capture_slot` (widened to `pub(super)`); (2) `export_ready_surface`
  calls it for Ready surfaces when stable mode is off; (3) `begin_picture`
  tolerates reservation starvation in a session converted mid-flight — the
  legacy phase queued every CAPTURE slot at streamon, so the first frames
  after the flip have no slack until completions drain the kernel queue
  (dequeued slots stay Free because `queue_working_capture` caps the queue
  at WORKING_QUEUE_MAX=6); a starved surface now decodes without a
  reservation and deque publishes the working slot directly. Result:
  16 successful exports / 0 failed / 20 BeginPictures / no software
  fallback in one probe window (was 1/1/4 with teardown), probe log
  `/home/mq/.cache/libva-v4l2-browser-verify/run-1789886403-834287/firefox.log`.
  Required gates on the fix build (clean worktree at HEAD + lane files,
  since codex's uncommitted AV1 WIP fails 6 tests + 1 clippy lint in the
  shared tree): `verify-rust-driver.sh` green, `verify-session-churn.sh`
  pass=7 fail=0. Also this session: `verify-resolution-churn.sh` gained a
  `V4L2_VA_RESOLUTION_CYCLES` knob (default 2 = previous behavior);
  cycles=4 hardware run passed (1560/1560 frames, 8 source changes,
  sanity pass, 0 firmware fatals). Post-commit standalone rerun of the
  churn probe was skipped twice by its pre-decode sanity
  (`node_unhealthy_pre_decode`): kernel log shows device-wide
  `0x5000003` system-fatals at 13:49/13:53 triggered by OTHER
  `av:h264` processes' `vb2_start_streaming` warnings (concurrent
  agent hardware runs) — the same build had already passed the
  embedded `resolution_probe=pass cycles=2 780/780` leg of
  `verify-rust-driver.sh` minutes earlier; did not retry further to
  avoid deepening the firmware poisoning for the other lane.

## Last verified clean baseline

- Commit under test: `main` with the staged Main10/P010 lane.
- `tools/verify-rust-driver.sh /tmp/libva-v4l2-rust-driver-main10-full-20260920-120916`: passed 109
  Rust tests, H.264 sample-1/sample-30/sample-full byte-exact matrix,
  GStreamer export callback, GL zero-copy roundtrip (`missing=1 tolerated=2`),
  FFmpeg mixed-resolution gate (`decoded=780 expected=780 source_changes=4`),
  long playback (`decoded=3600 expected=3600`), HEVC Main native parity,
  HEVC Main10 P010 reference parity, and VP9 Profile 0 native parity. Codec
  logs: `/tmp/libva-v4l2-codec5-20260920-120916`. AV1 is skipped because it
  is not advertised.
- `tools/verify-session-churn.sh /tmp/libva-v4l2-rust-driver-main10-full-20260920-120916`:
  `pass=7 fail=0`. The GStreamer leg now retries once with driver debug after
  timeout so intermittent empty-log stalls leave useful evidence and still fail
  if persistent.
- Expected optional verifier results remain: native one-frame EOS produces no
  frames; `bframes-240p` is a Rust decode xfail and can poison the next hardware
  session.

## Completed recently

- Production kernel observation fixed (review root, 2026-10-01): gate uses
  shared complete seven-counter validator before advancing each probe; partial
  or malformed summaries cannot pass. New isolated2tests cover clean fullgate
  and four failure cases, proving nextprobe does not start. Independent exact
  distro-source regressions rerun: metadata4096 and PSC256 both pass. Kernel
  review packet preserved on disk at ~/.cache/libva-v4l2-qualification/kernel/
  review-20261001. No module activated; full new-build hardware gate pending.

- Seek verifier closure (gl child, 2026-10-01): acknowledged seeks now need a
  fresh seek/playback-restart event pair, seeking=false and two forward playback
  advances with vaapi-copy active. Shared4s per-seek/RPC deadline; keyframe
  landing offsets allowed.22IPC/shell tests and101full Python tests pass;
  three regressions fail actual historical controller. Files:mpv_seek_drive.py
  and test_seek_ipc.py; report docs/pending-lane-seek.txt. No hardware opens
  after the firmware fatal; fresh candidate/hardware seek remains pending.

- EOS fail-fast verification fixed (review root, 2026-10-01): every wrapped
  leg now requires command success and complete clean seven-counter evidence
  before another decoder opens. Partial natural/post-cut output and invalid
  mpv cut stop immediately. New isolated integration regression exercises
  clean path, all seven faults at all three phases, missing summaries, command
  failures and partial output. Both sanity decodes now require clean kernel
  evidence too, including stop-before-next-open for initial faults. Passed5 regression tests (33 scenarios),
  existing14 verification tests and bash syntax. Hardware remains pending.

- Storage failure diagnosed and GL retested (review root, 2026-10-01): eDLZvx
  failed because GStreamer filesink hit Disk quota exceeded, truncating gl.raw;
  CPU1/30/300 and kernel window had passed. Generated exact-production runner
  now defaults results and Cargo target to ~/.cache/libva-v4l2-qualification
  on disk (override V4L2_VA_QUALIFICATION_ROOT); source/binary pins preserved.
  Direct leased rerun of fc08226...a6e10: strict GL300/300 ordered match,
  missing=0 tolerated=0, exit0, all seven kernel counters zero. Evidence:
  ~/.cache/libva-v4l2-qualification/gl-storage-retest-20261001.log and matching
  dump directory. Shell syntax and whitespace pass. Original failed artifacts
  retained. This is a storage/GL fix only; active EOS/seek lane remains open.

- Direct hardware continuation completed (root, 2026-10-01): exact production
  e1tXqa, lifecycle jezSwc, corrected seek PIvRqk/debug svZYKG; results above.
  Fixed bounded mpv startup handling without masking post-seek hardware loss;
  `python3 -m unittest discover -s tools/tests -q` passes 79/79; seek12/12;
  `git diff --check` passes. Driver source/binary unchanged. Production remains
  FAILED; no user-run handoff needed for currently accessible hardware.

- Stalled-run harness diagnosis/fix (root, 2026-10-01): preserved OQuT34
  artifacts; independently confirmed saved CPU parity and full GL raw equality;
  stopped only verified stale process groups and released hardware lease.
  Added diagnostic FFmpeg -nostdin, prepared hash-verified same-binary host
  runner with null stdin. Validation: bash -n; 75 tooling tests pass via
  `python3 -m unittest discover -s tools/tests -q`. Hardware rerun requested;
  kernel/lifecycle/production qualification stays pending above.

- Exact GL-passing revision qualification attempted (root + delegated tests,
  2026-10-01): frozen source hash/prebuilt identity verified. Production host
  gates pass fmt, strict Clippy, 75 tooling tests and 208 Rust/stress tests;
  actual gate stops at hardware_in_use. Independent discovery confirms absent
  decoder/DRM nodes here; no matrix/churn/EOS/seek hardware stages ran. 4K
  fixtures/hashes for four codecs verified; runner stops missing_render_device.
  Browser strict attempt stops hardware_busy; pinned runner stops missing
  hardware. Display env/sockets exist, but no browser launched. Actual Iris
  function sanitizer regression passes; patched kernel boot remains unproven.
  Prepared same-binary executable packages:
  /tmp/libva-exact-production-20261001/run-production-exact.sh
  /tmp/libva-4k-pinned-qualification-20261001/run-four-codecs.sh
  /tmp/libva-exact-browser-kernel-20261001/run-browser-exact.sh
  No rebuild or system install. Integration tightened 4K runner to stop at
  first failed codec and pinned snap-browser staging to the same tested binary.
  Reports: docs/qualification-run-{production,4k,browser-kernel}.txt. Real
  hardware execution remains with physical-host owner; no release pass claimed.

- Six-subagent sweep integrated (root, user-requested 2026-10-01): all six
  lanes completed achievable host work; reports in docs/production-lane-*.txt,
  aggregate docs/production-completion-20261001.txt. Qualification fingerprints
  source/fixtures/binary; GL checks entire streams and individual reservations;
  kernel probes reject partial/corrupt output; complete GOP history is bounded
  at 1,024 units/32 MiB; 4K/browser checks require real performance/seek/frame
  evidence; malformed AV1 metadata rejected, production profile still hidden.
  Cross-review fixed keyframe hidden-owner clearing before guard, bounded old
  owner completion before START, preserved peer replay pacing/stable top-ups.
  Frozen source /tmp/libva-six-agent-20261001/source: Rust 204/204; injected
  stress suite 208/208; Python 75/75; fmt, strict Clippy, shell syntax, release
  build and whitespace pass. Production gate host stages/provenance pass then
  explicitly missing_hardware; final matrix/churn stop at absent DRM device.
  Current external hardware evidence supplied by user: CPU1/30/300 pass,
  strict GL18/300 fails, clean kernel (A1KZRr logs above); that binary predates
  final corrections. Root claims no hardware pass. Actual Linux tree candidate
  patch applicability and expanded UBSAN pass, but tree/build do not match
  running deployment kernel. All root child edits are finished; external
  hardware owner can continue, freezing/testing source before each run.

- Production source fixes (2026-10-01): race-free export-fd EOF regression;
  exported-surface completion before EndPicture returns; bounded importer-fence
  wait plus dma-buf cache maintenance for stable CPU copies; propagation of
  failed/undersized copies; per-display capability tables without optimistic
  fallback; custom H.264 scaling lists and truncated-IQ rejection. Hardened GL
  exact count/order, independent EOS reference, seek acknowledgments/status,
  experimental-AV1 rejection and serialized production hardware checks.
  Validation: Rust 189/189 with 16 threads, host stress 4/4 and copied suite
  193/193, Python 37/37, fmt/strict clippy/shell/whitespace/release build pass.
  Kernel candidate sanitizer regression: 4096 inputs pass, not installed.
  `verify-rust-driver.sh` and `verify-session-churn.sh` attempted, fail missing
  device; `verify-production.sh` passes host stages then missing_hardware.
  Evidence: `/tmp/libva-prod-final-20261001`; report above. Production release
  remains blocked pending real-hardware and deployment-browser qualification.

- Additional production input/image audit (top-down review agent, 2026-10-01):
  completed host-only changes in `buffer.rs`, `codec/mod.rs`, `image.rs`,
  `image/layout.rs`, and `image/tests.rs`. Preserve original buffer allocation
  when changing valid element count, allowing shrink/reset/grow to capacity;
  reject inactive or truncated decode buffers before reading parameters.
  Check buffer-table capacity before allocating/copying and report allocation
  failure through VA status. Allocate/copy complete odd-height chroma rows
  and odd-width UV pairs in NV12/P010. Validate every source/destination bound
  before modifying image pixels; reject truncated snapshots in Get/DeriveImage.
  Reject odd crop origins because this raw-copy path cannot resample chroma.
  Seven new regressions fail against saved pre-fix implementations and pass
  after fixes (isolated replay: 178 passed, seven expected failures).
  Validation: `cargo test --locked --manifest-path rust/Cargo.toml --quiet`
  passed 185 tests at the initial snapshot; subsequent isolated
  `tools/verify-host-stress.sh /tmp/libva-v4l2-input-audit/stress-after` passed
  four stress tests and all 193 tests with 16 test threads, including concurrent
  agents' newer tests. `cargo clippy --locked --manifest-path rust/Cargo.toml
  --all-targets -- -D warnings` and `git diff --check` pass. Evidence resides
  in `/tmp/libva-v4l2-input-audit/`. No hardware runs in this audit; GL, kernel,
  queue, and browser qualification remain with their active owners above.

- Reference-chain recovery guard (codex, 2026-10-01): truncated history can
  lose its keyframe; published-only filtering can omit hidden dependencies;
  pending OUTPUT alone can omit already-consumed reference pictures. Added
  pure replay validation and fail-closed guards before START/reopen, plus
  first-keyframe QBUF metadata during valid rebuilds. Eleven new tests cover
  truncation, missing references, hidden pictures, ownership/byte mismatches,
  and the actual 64-chunk limit. Isolated staged-source checks passed: 169
  tests with 16 threads, clippy -D warnings, fmt. Snapshot:
  `/tmp/libva-v4l2-replay-isolated-9l4pu9n_`. Required hardware attempts:
  `/tmp/libva-v4l2-replay-guard-{rust-driver,session-churn}-20261001.log`;
  both BLOCKED at initialization (renderD128/video16 absent). Full consumed
  reference rebuild and long-GOP recovery remain mandatory pending work.

- CPU image read follow-up (codex, 2026-10-01): removed the redundant
  full-frame `vaGetImage` clone under the driver lock. Derived images retain
  independent storage. Rust 166 tests and strict clippy pass; hardware H.264
  1/30/full pixels pass and codec-expansion parity passes 4/4. Required matrix
  still FAILS strict GL missing=1; the same window also includes a native
  one-frame Iris system-fatal plus vb2 warning. Kernel wrapper returns
  failure despite a successful native fallback. Logs:
  `/tmp/libva-v4l2-image-borrow-{rust-driver,codec-expansion}-20261001.log`.
  4K rawvideo checksum output now pins one encoder thread for reproducible
  memory measurements. No new 4K throughput/RSS claim: installed kernel is
  still affected by the separately reproduced metadata-index bounds fault.

- Bottom-up quality gate hardening (codex, 2026-10-01): repeated 4K H.264
  output is byte-exact across 600 frames with continuous DTS/PTS, checked
  against the full native 60-frame reference. Overall run FAILED: kernel
  UBSAN index-32 reads at `iris_buffer.c:869/870`. Candidate immediate-wrap
  kernel patch and extracted-function sanitizer runner are in `kernel/` and
  `tools/verify-iris-metadata.py`; baseline reproduced, patched 4096-input
  regression passed. Kernel installation/boot validation requires host sudo
  access, which is unavailable noninteractively. Do not treat suppressed
  repeat UBSAN reports as a clean bill of health for the old kernel.
  Kernel capture now fails on faults/warnings or missing journal observation;
  4K references are counted before driver probes and repeated comparisons
  verify all records. Churn now rejects fallback, partial/failed output,
  unexercised signals, and hidden timeout retries.
  Validation: 19 dedicated acceptance regressions and 7 parallel-owner
  verifier regressions passed; Rust 164 tests passed with 16 threads, fmt
  and strict clippy passed. Required matrix rerun passes H.264 1/30/full,
  then fails strict GL (missing=1, tolerated=0). Hardened churn passes 7/7
  with kernel-bugs=0. Logs: `/tmp/libva-v4l2-quality-strict-fixed-{rust-driver,session-churn}-20261001.log`.
  Matrix rerun first exposed a concurrent test counter race (3 mappings
  counted instead of 2); thread-local teardown instrumentation fixes this
  without changing production behavior. Only this test hunk in `v4l2.rs`
  is owned here; preserve the parallel queue-hardening changes.

- Bottom-up quality fixes (codex, 2026-10-01): corrected AV1 quantizer values,
  restoration enums, hidden-reference materialization, integer-motion flag
  syntax, and 128x128 restoration shift syntax; added a shadow-reference guard
  and experimental profile opt-in. Moved VA mapping out of `codec/raw.rs`
  and split the 1,300-line AV1 frame writer into types/header assembly,
  syntax helpers, and fixture tests. Fixed H.264's encoder-specific PPS
  reference default; original 4K PPS is pinned as a fixture. HEVC tile counts
  and dimensions now reject malformed layouts before synthesis.
  - `cargo test`: 164 pass; strict clippy and formatting pass.
  - `tools/verify-host-stress.sh /tmp/libva-v4l2-host-stress-fixed-20261001`:
    host stress pass; copied parallel suite 168 pass. Closes the audit's
    excessive-HEVC-tile-count failure.
  - `tools/verify-4k-decode.sh` with each of h264/hevc/hevc10/vp9:
    3840x2160 1/30/full (60) frame reference parity and clean kernel windows.
    Artifact `/tmp/libva-v4l2-quality-integrated-20261001`; logs
    `/tmp/libva-v4l2-4k/{h264,hevc,hevc10,vp9}`. Hardware output/download is
    required, so software fallback cannot pass. Main10 uses software P010
    reference; the other three use native references.
  - Same artifact: experimental codec expansion 4/4 at 30 frames, logs
    `/tmp/libva-v4l2-quality-codecs-integrated-20261001`; 4K H.264 churn
    `pass=7 fail=0` with 30-frame playback cuts and kills, logs
    `/tmp/libva-v4l2-4k-churn-20261001`. Earlier 720p churn also 7/7.
  - Full baseline `/tmp/libva-v4l2-quality-final-matrix-20261001.log`
    passes required H.264 parity, then FAILS strict GL coverage
    (`missing=1 tolerated=0`). This remains a release blocker, not a pass.
  - AV1 full stream is still blocked: original trace has 43 refresh-flag
    disagreements with the heuristic, first at order hint 64. Keep gated
    until authoritative metadata exists. See `docs/10-quality-validation.md`.


- AV1 uncompressed_header writer landed, byte-exact (claude agent, 2026-09-20):
  new `rust/src/av1/frame.rs` writes spec 5.9.1 `uncompressed_header()` (all
  sub-sections: tile_info derivation from tile_cols/tile_rows counts,
  quantization, segmentation, delta q/lf, loop filter, CDEF, LR, tx mode, ref
  mode, skip mode, global motion, film grain) plus `synthesize_frame_obu()`
  (OBU_FRAME wrap). Pinned byte-exact against the real libsvtav1 sample
  (`/home/mq/tmp/vaatest/codec5/av1-720p.mp4`): keyframe header 22 bytes,
  first inter header 28 bytes, and the full 41-byte TD+Seq+Frame access-unit
  prefix. Field-by-field ground truth came from
  `ffmpeg -f obu -i <file> -c copy -bsf:v trace_headers -f null -` — the
  authoritative oracle for this lane, use it first next time. Three findings
  encoded in the writers:
  1. Fixed the committed sequence header writer: the `seq_choose_integer_mv`
     bit was missing entirely (proven by the real payload and trace position
     86); `SequenceHeaderInput` gains `seq_choose_integer_mv` /
     `seq_force_integer_mv`.
  2. `uncompressed_header()` does NOT end with `trailing_bits()`: frame_obu
     pads with `byte_alignment()` = zero bits only. The one-bit marker is
     exclusive to OBUs ending at payload granularity (sequence_header_obu).
  3. `skip_mode_present` is coded only when spec 5.9.16 skipModeAllowed holds
     (forward+backward or two forward refs by `get_relative_dist`). The writer
     derives allowed-ness from `ref_frame_idx`/`ref_order_hint` (VA carries no
     flag); the real inter frame codes `allow_warped_motion=1` and no skip bit.
  Stable API for codex's wiring into `codec/raw.rs::finish_picture`:
  `crate::av1::{synthesize_sequence_header, synthesize_uncompressed_header,
  synthesize_frame_obu, SequenceHeaderInput, FrameHeaderInput, FrameType,
  Av1SynthError}`. Remaining AV1 lane work (codex): wire the AU prefix into
  raw.rs, tile_group wrap with per-tile sizes, then unhide
  `VAProfileAV1Profile0` behind `tools/verify-codec-expansion.sh` AV1 parity.
  Validation: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`
  clean, `cargo test` = 142 passed / 2 failed where both failures are codex's
  own uncommitted `config.rs` WIP tests (AV1 unhide present, its test
  expectations not yet updated) — pre-existing on the shared tree, untouched
  by this lane.

- Phase 5 Main10 / P010 lane (codex agent, 2026-09-20): added decoded-format
  plumbing across configs, surfaces, CPU-copy images, V4L2 CAPTURE setup, and
  DRM PRIME descriptors. HEVC Main10 now advertises only when `/dev/video16`
  exposes P010 CAPTURE. Generated and documented
  `/home/mq/tmp/vaatest/codec5/hevc-main10-720p.mp4`. Validation:
  `cargo fmt --check`, 109 Rust tests, strict clippy, release build, and
  full `tools/verify-rust-driver.sh` on
  `/tmp/libva-v4l2-rust-driver-main10-full-20260920-120916` with codec logs
  under `/tmp/libva-v4l2-codec5-20260920-120916`, and
  `tools/verify-session-churn.sh` on the same artifact: HEVC Main native
  reference parity passed, HEVC Main10 software-HEVC-to-P010 reference parity
  passed, VP9 native reference parity passed, AV1 skipped because hidden, and
  session churn passed `pass=7 fail=0`. Note:
  FFmpeg's `hevc_v4l2m2m` native wrapper aborts or emits no frame rows for the
  Main10 sample, so Main10 is gated against a software P010 reference instead
  of that broken wrapper.

- Phase 4/5 merged into `main` (codex agent, 2026-09-20): branch
  `codex/phase4-5` was merged as `fd2987b`; the isolated worktree is no longer
  the working target. Follow-up verifier fixes: `verify-long-playback.sh` now
  counts frames from the generated playlist it actually decodes, and
  `verify-session-churn.sh` adds a diagnostic retry for intermittent GStreamer
  timeout flakes after mpv cuts. `verify-rust-driver.sh` now includes the GL
  zero-copy roundtrip gate.

- Phase 4/5 implementation (codex agent, 2026-09-20): CPU-copy lifecycle gates
  are complete for the covered browser-style workload. `verify-rust-driver.sh`
  passed the H.264 sample-1/30/full matrix, GStreamer export callback,
  mixed-resolution CPU-copy gate (`decoded=780 expected=780 source_changes=4`,
  zero Iris faults), long playback (`3600/3600` over 12 segments), HEVC Main
  30-frame native parity, and VP9 Profile 0 30-frame native parity.
  `verify-session-churn.sh` passed `pass=7 fail=0`; `verify-seek-storm.sh`
  passed 24 same-resolution seeks and 12 mixed-resolution seeks, with mixed
  seeks recovering from per-session `0x4000003` Iris aborts and no system-fatal
  faults. Host validation: 102 Rust tests and strict clippy passed. Main10 and
  AV1 are intentionally hidden until P010 and AV1 OBU synthesis exist.

- PHASE 2 COMPLETE (claude/opus agent, 2026-09-19): the required CPU-copy gate
  is green on hardware. Root fix: synthesize the H.264 SPS VUI with
  `max_num_reorder_frames=0` (`rust/src/h264.rs`) so iris emits every frame in
  decode order instead of withholding the first displayable frames until a
  drain. That reorder delay was the residual deadlock — FFmpeg's VAAPI-copy
  hwaccel only pipelines reorder_depth+1 frames before blocking in
  `vaSyncSurface` on the first surface, and the STOP/drain/rebuild path used to
  break it corrupted reference continuity. With decode-order output the driver
  maps each CAPTURE buffer to its surface by timestamp and the client reorders
  by PTS. Results: `sample-1`/`sample-30`/`sample-full` all BYTE-EXACT vs native
  `h264_v4l2m2m`; `verify-session-churn.sh` `pass=7 fail=0` (mpv-cut, GStreamer,
  SIGKILL/SIGTERM recovery); `gst_export_probe=reached_driver`; 91 host tests +
  fmt + strict clippy pass. Driver artifact `/tmp/libva-v4l2-rust-driver-reorder0`.
  This fix REQUIRES codex's uncommitted SOURCE_CHANGE-handshake / recovery
  groundwork — verified that clean HEAD + the reorder change alone still fails
  (rc=251), and that codex's tree alone deadlocked on `surface=0x40000003`; the
  two together complete Phase 2. NOT fixed by this and NOT a regression from it:
  `verify-resolution-churn.sh` still times out (status 124) on the small 480p
  GStreamer stream — reproduced identically at pure HEAD, so it is a
  long-standing Phase 4 issue, tracked separately. Golden SPS/PPS byte tests in
  `rust/src/h264.rs` were updated for the new VUI value.

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

- Small H.264 follow-up (codex, 2026-10-01): the PPS correction closes 4K
  correctness but not `bframes-240p`. Latest bounded probe on
  `/tmp/libva-v4l2-quality-small-20261001` still reports five session-fatal
  `0x4000003`, zero system-fatal faults; log
  `/tmp/libva-v4l2-small-fixed-20261001.log`. Noninteractive sudo requires
  authentication, so no dynamic-debug flags were modified. Prepared
  `tools/capture-iris-dynamic-debug.sh` for the root trace, with callsite
  restoration and decoder execution as the original user. Source-change
  empty ERROR completions must reach the marker handling in the shared queue
  hardening branch; nonempty damaged pixels remain rejected.

- Phase 3 zero-copy export produces ZERO decoded frames (claude/opus agent,
  2026-09-19, on committed c15992c+e0ddcd9): `verify-resolution-churn.sh` times
  out (status 124), and root-causing it showed the GStreamer `vah264dec`
  zero-copy EXPORT path decodes nothing on this driver — for BOTH seq-480p and
  test_720p, and reproduced identically at pure HEAD, so it is pre-existing and
  unrelated to the Phase 2 reorder-0 fix. Evidence:
  * ffmpeg CPU-copy on seq-480p is byte-exact (90 frames) — decode + SPS synth
    are correct; the failure is specific to the export/`stable_capture` path.
  * gst-va submits ONE IDR then blocks in `vaSyncSurface`. iris never even
    consumes that OUTPUT buffer (`OUT DQ=0`, `out=1/16` frozen); it returns only
    empty CAPTURE/source-change/EOS markers, and the mid-stream `DECODER_CMD
    STOP` sync drain issued right after the source-change `START` does NOT flush
    the lone IDR — iris ignores the drain in that state.
  * Ruled out: NOT reorder/DPB latency (`max_num_reorder_frames=0` shipped;
    `max_dec_frame_buffering=1/2` kept 720p+480p byte-exact but gst still emits
    nothing). NOT CAPTURE-buffer starvation (queueing all 20 buffers made
    `cap=20/20` and only converted the hang into a silent 0-frame completion —
    same failure mode as the 720p gst run, whose surfaces all go Dead while
    `gst-launch` still exits 0, which is why session-churn's exit-status-only
    gst legs falsely "pass").
  Conclusion: this is Phase 3 (dmabuf zero-copy), NOT a Phase 2 regression.
  Next step needs root-level `qcom_iris` HFI tracing to see why iris will not
  drain a single IDR across the initial `SOURCE_CHANGE` for the export path
  (same tooling gap as the `bframes-240p` blocker). Meanwhile
  `verify-resolution-churn.sh` should arguably be de-gated from the Phase 2
  required verifier since it exercises the export path, not CPU-copy.

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

## Quality stress audit (2026-10-01)

- Added host-only isolated runner `tools/verify-host-stress.sh` and
  `tools/host-stress.rs`; report: `QUALITY-STRESS-20261001.txt`. No production
  source changes from this audit. Existing work continued concurrently, so
  hardware results apply to `/tmp/libva-quality-20261001` source snapshot.
- Passed 4,000 buffer cycles with eight shared-driver callers, 10,000 malformed
  codec sequences, table exhaustion/recovery, two simultaneous byte-exact
  120-frame H.264 sessions plus post-stress sanity, and session churn 7/7.
- Full hardware verifier failed Main10: P010 S_FMT returned NV12, context init
  failed and FFmpeg fell back to software. Other covered codec/lifecycle gates
  passed. Host runner intentionally fails a new regression for HEVC tile counts
  exceeding VA arrays; 162 other tests passed in its copied current tree.
- One snapshot suite run exposed the export-fd test's stale numeric-fd race;
  see report and `/tmp/libva-quality-20261001/unit-tests.log`.
