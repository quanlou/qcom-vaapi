# Iris timestamp metadata bounds fix

## Patch licensing

The repository-root MIT license covers the user-space driver and project
documentation. The kernel patch files in this directory modify Linux kernel
source and are licensed under GPL-2.0-only; see
[`LICENSE-GPL-2.0-only`](LICENSE-GPL-2.0-only). This exception applies to the
patch files, including the QRTR patch copied with its original attribution.

`0001-iris-wrap-timestamp-metadata-index.patch` is a candidate kernel fix,
not part of the Rust VA driver and not installed on this host.

During a 600-frame 4K H.264 VAAPI-copy run on `7.3.0-15-qcom-x1e`, UBSAN
reported `iris_buffer.c:869` and `:870`: index 32 exceeds `iris_ts_metadata[32]`.
The source explains the boundary: input metadata advances the next-write index
without wrapping until another input arrives; capture's unmatched-timestamp
fallback accesses that index directly. Wrap immediately after incrementing.
The patch preserves the existing next-slot fallback behavior. Whether that
fallback selects the appropriate metadata is a separate unresolved question.

Sources inspected on 2026-10-01:
[metadata writer](https://github.com/torvalds/linux/blob/master/drivers/media/platform/qcom/iris/iris_common.c),
[capture reader](https://github.com/torvalds/linux/blob/master/drivers/media/platform/qcom/iris/iris_buffer.c).

Test against a Linux source tree without modifying it:

```sh
python3 tools/verify-iris-metadata.py /path/to/linux
```

The runner copies the source into a temporary directory, applies this exact
patch without fuzz, extracts the original metadata functions, and compiles them
with UBSAN and minimal stand-in kernel types. It requires the original code to
reproduce the index-32 fault, then checks 4,096 patched inputs with matching and
missing output timestamps. It also checks every retained timestamp in reverse
completion order across ring wraps, preserves unrelated buffer flags, checks
masked timestamp flags and the existing next-slot fallback, and exercises
stale writer indices 32, 33, and UINT32_MAX. This isolates the bounds defect; it is not a kernel
build, concurrency test, or proof of hardware behavior.

To prepare the change in the matching distro kernel source tree:

```sh
git -C /path/to/linux apply --check /path/to/libva-v4l2/kernel/0001-iris-wrap-timestamp-metadata-index.patch
git -C /path/to/linux apply /path/to/libva-v4l2/kernel/0001-iris-wrap-timestamp-metadata-index.patch
```

Build and boot that kernel using the distro's kernel workflow. Afterward rerun
the full decode/lifecycle matrix and the 600-frame 4K test with kernel logging.
This environment has no noninteractive sudo access, so kernel installation and
boot verification remain pending. A later warning-free run on the old kernel
does not close this defect: UBSAN reporting can suppress repeated occurrences.

The October 1 resumption reproduced both index-32 reads during the 720p
production gate as well. `tools/activate-iris-candidate.py` prepares a temporary
runtime module replacement without changing installed modules. With no action
flag it checks the candidate SHA256, running release/vermagic, every imported
symbol CRC, loaded build ID, and idle module under the shared hardware lease:

```sh
python3 tools/activate-iris-candidate.py /path/to/reviewed/qcom-iris.ko \
  --sha256 <reviewed-sha256> --evidence /path/to/fresh/preflight.json
```

The October 1 user-started activation deadlocked while removing the original
module, before insertion. The helper now refuses live removal of the original
and bounds-only builds. Recover with a normal reboot and load a reviewed module
on a boot where Iris is absent; use `--require-absent` to enforce that condition
inside the shared lease. `--exclude-boot-id` rejects a known faulted boot.
It pins checked candidate bytes and verifies the loaded GNU build ID. Ordinary
insertion/identity failures attempt restoration of the installed module, but an
unfinished kernel operation never starts a competing rollback. Command timeout
records the unfinished PID and returns; it cannot recover a D-state kernel
syscall. `--rollback` uses the same identity and unsafe-removal guards.
Every invocation needs a fresh evidence path; phase records are written before
mutation, and only `status: pass` with phase `complete` proves success.
A successful activation proves runtime module identity on that boot;
it does not prove playback correctness or establish a persistent deployment.
Rerun all hardware gates before qualifying the module. Production provenance
now rejects a different boot or loaded Iris module even with the same release.

## Bounded firmware comparison probe

On the deployment host, the root-only dynamic-debug probe checks readable
fixtures/driver and actual device nodes before changing logging, and bounds
software reference counting and each hardware command. A successful FFmpeg exit
alone cannot pass: every leg must emit the independently counted number of
frames (capped at 30). The 720p driver leg must match native NV12 pixels in exact
display order. Logging flags are restored on exit, and the shared hardware lock
serializes the probe.

```sh
sudo tools/capture-iris-dynamic-debug.sh /path/to/driver /tmp/iris-driver-probe
```

Use a separate recovered/cold device session to compare the native small-stream
path; the script stops at the first failure and must not repeatedly reopen a
crashing firmware session:

```sh
sudo env V4L2_VA_IRIS_SMALL_DECODER=native \
  tools/capture-iris-dynamic-debug.sh /path/to/driver /tmp/iris-native-probe
```

Fixture overrides are `V4L2_VA_IRIS_SAMPLE_720P` and
`V4L2_VA_IRIS_SAMPLE_240P`. Keep the failed command, checksum output, full kernel
window, and enabled callsites from each separate output directory. This is a
bounded diagnostic, not full-stream or kernel boot qualification. It requires
privileged access on the deployment host; no logging or boot changes have been
performed in this environment.

A boot qualification record must identify the distro kernel source and patch,
built kernel/modules and their release/build IDs, the running boot ID/kernel,
and firmware artifacts. A matching `uname -r` or hashes of firmware files alone
cannot prove the patched kernel or those firmware bytes are running. Re-run
`tools/verify-production.sh` and the bounded small-stream/native comparison on
the patched boot, preserving clean kernel windows and full-frame parity evidence.
The existing fallback policy may still select semantically wrong metadata for
unmatched timestamps; this patch fixes the out-of-bounds access only.

## Empty PSC-LAST candidate

`0002-iris-preserve-empty-psc-last-completion.patch` targets the matching
`linux-qcom-x1e 7.3.0-15.15` source. Both firmware drain LAST and picture-sequence
change PSC_LAST already map to V4L2 LAST. The existing empty-buffer check exempts
only drain LAST, so a clean streaming PSC_LAST gets a synthetic ERROR. ERROR
completion returns before LAST/stopped/EOS bookkeeping. The candidate exempts
PSC_LAST from that one synthetic-error check; real NOSHOW/corruption/overflow
errors remain intact. It leaves genuine error completion handling unchanged.

```sh
python3 tools/verify-iris-psc-last.py /path/to/matching-linux-source
```

The runner extracts actual flag mapping, output handling, and VB2 completion,
then uses fake queues and successful sub-state helpers under UBSAN. Its 256-case
matrix covers LAST/PSC_LAST combinations, empty/nonempty payloads,
streaming/nonstreaming state, all corruption/overflow/NOSHOW combinations, and
previously stopped queues. Baseline reproduces synthetic PSC error and skipped
LAST bookkeeping; patched code preserves normal terminal-marker bookkeeping
and all genuine errors. This tests source behavior, not real HFI transitions.

The observed small-stream LAST|ERROR does not establish clean PSC_LAST: raw HFI
flags and frame-info errors are missing, and the session fatal may precede the
marker. This is a justified source-contract candidate, not a proved firmware or
small-stream fix. Keep the original bounds patch and strict production gates.
No candidate module has been loaded or installed by this lane.

## Removal IRQ lock cycle candidate

`0003-iris-quiesce-irq-before-remove.patch` addresses the observed device-removal
deadlock in `linux-qcom-x1e 7.3.0-15.15`. Hung-task logs show module removal
waiting in `disable_irq()` while holding `core->lock`; the threaded IRQ waits
for that same mutex in `iris_hfi_queue_msg_read()`.

The patch takes a runtime-PM reference, synchronizes the IRQ before entering
core teardown, cancels queued system-error work, and balances its PM reference
before destroying the lock. It preserves IRQ synchronization. Changing back to
`disable_irq_nosync()` could allow a queued handler to access powered-off
hardware; see the [upstream fix](https://github.com/torvalds/linux/commit/b9c2215bdedc9c532a7e9d57ec49ee1b6381f863)
and [IRQ API contract](https://docs.kernel.org/core-api/genericirq.html).

```sh
python3 tools/verify-iris-remove.py /path/to/matching-linux-source
```

This extracts the actual original and patched removal functions plus core
teardown and tests them with a pthread IRQ model under UBSAN. Original code
deadlocks; patched active, cold, PM-failure, and null-device scenarios complete
with balanced modeled PM references. A bounds-plus-removal module builds
against the matching distro headers, with all 152 imported symbol CRCs checked.
The new `cancel_delayed_work_sync` import must match the running release's
complete `Module.symvers`; shared imports also match the installed module.
Cold insertion loads the candidate's declared dependencies under the lease,
then checks Iris absence again before `insmod`. Dependency failures stop before
insertion or original-module restoration.

This is a source-order model and ABI review, not runtime qualification. It does
not test real PM callbacks, firmware, active-handle unbind, or other power-off
paths that take the core lock. General PM lock ordering remains unresolved.
The bounds-plus-removal candidate was cold loaded on 2026-10-01 and passed
the recovery-v5 headless gate. Real-use performance and browser qualification
failed. A later reduced-allocation experiment passed VA 1/30 frames, then its
next native reference emitted session-fatal errors; that boot is stopped for
hardware work. This does not establish the allocation as the sole cause.
Cold-boot instructions and exact local
artifact identities are in [the resumption report](../docs/production-resumption-20261001.txt).

`0004-iris-expose-decode-order-output.patch` adds the standard decoder display
delay controls to the SM8550 Gen2 firmware table used by X1E. An explicit
enable=1/delay=0 request sends Qualcomm's published decode-order property on
OUTPUT stream setup. Default clients send no new property; control changes
while streaming remain rejected. Property failures now propagate from stream
setup. The matching VA driver requests the pair only when both controls exist.
The v7 module was cold loaded on 2026-10-02 with its exact build ID verified.
The full v7/v8 correctness gates passed, and requesting decode order removed
costly per-frame compatibility drains. v8 Chromium passed strict playback/seek/
exit checks; 4K process RSS remains over budget. A later smaller capture-pool
experiment emitted firmware faults and was reverted. That boot is stopped for
hardware work. These results do not qualify persistent deployment or general
PM/unload behavior. Keep the bounds/removal patches; activate only from an
absent-module cold boot. See the current continuation record for exact identities.

### QRTR platform resume dependency

`0006-qrtr-resend-hello-on-mhi-resume.patch` is the unchanged upstream Linux
commit `6a5719cc3ef2e4d9857cc4ae18e6db09d59a8cc9` by Daniel J Blueman. It
resends the QRTR HELLO handshake after MHI resume and addresses the matching
WCN7850 Wi-Fi restart timeout. This is a platform dependency, outside the Iris
module and VA-API backend; apply it to full kernel QRTR sources separately.
Its module pair is host-verified and installed for the next boot, but sleep is
not qualified yet. Earlier failed platform evidence is retained.

Deployment artifacts and rollback instructions are maintained in
`/home/mq/.cache/libva-v4l2-qualification/resume-20261002/deployment/qrtr-resume-r2/INSTALL.txt`,
not in Chromium Snap storage. Original distro modules and root rollback backup
remain intact.

### Experimental 4K60 and session-lifetime candidates

`0007-iris-budget-low-delay-vp9-playback.patch` votes interconnect bandwidth
using the measured input rate and raises the frequency estimate for explicitly
requested decode-order VP9. It changes the host clock budget, not firmware pipe
configuration. It requires 0004's display-delay controls. The isolated actual-C
model checks codec/control scope and resolution/rate scaling:

```sh
python3 tools/verify-iris-playback-clock.py /path/to/baseline-iris /path/to/patched-iris
```

`0008-iris-retain-session-through-firmware-close.patch` pins an instance across
firmware response handling and retains its queues through the close exchange.
The actual-C AddressSanitizer model reproduces two baseline lifetime failures
and checks the candidate ordering:

```sh
python3 tools/verify-iris-session-lifetime.py /path/to/baseline-iris /path/to/patched-iris
```

These model checks do not qualify hardware behavior. A short combined clock
trial approached 60fps, but warm reopen still failed, and a later lifetime
candidate trial recorded kernel memory corruption. Neither patch is installed
by the RC8 user-space package. The user-run ownership traces used the original
EL2 module; their matching addresses do not qualify these candidates. See the
[ownership checkpoint](../docs/4k60-ownership-status-20261004.md).
