# 4K60 / buffer ownership checkpoint (2026-10-04)

At the time of the ownership traces, the installed userspace library was RC7, SHA256
`bd734b985e73d74b209b200cea071728ced2096b9916244d75ea92579f90ef7f`.
The driver supports direct decoding into driver-owned exported GPU surfaces;
caller-imported layouts retain the compatibility copy path.
The local RC8 userspace build adds debug timing. This checkpoint records the
pre-installation evidence; see the [RC8 development notes](releases/0.1.1-rc.8.md)
for the subsequent verified user-space installation.

The currently booted EL2 Iris module is the original adapted module, build ID
`caccaef16827d57b71ae27ae7d3c00dff4877bad`. Temporary candidates do not survive a
restart. Boot and release files have not been changed by these experiments.

The local low-delay clock candidate improved one 62-second 4K60 trial to
59.5969 presented frames/s with 0.7254% drops and no steady-state waiting events.
That short result does not qualify stability. A second decoder session repeatedly
fails on its third visible submission, and forced shutdowns were actual freezes.

The previous boot logged a NULL page-pointer dereference while the Iris IRQ
released an internal DMA allocation. The lifetime candidate in patch 0008 pins
instances through IRQ handling, retains m2m queues until SESSION_CLOSE, and
ignores responses after teardown. Its actual lookup/close functions reproduce
two baseline use-after-free paths under ASAN and pass the candidate model.
It compiled with all 168 imported symbol CRCs checked. On hardware, first
sessions completed, but reopen still failed. On a subsequent boot,
page-table corruption, invalid swap entries,
and a kernel fault followed the second no-power-collapse session's close.
Thus patch 0008 is not an established fix for the observed memory corruption.
Power/control=on also did not eliminate the reopen failure. Hardware testing
stopped on the corrupted boot; temporary power settings returned to auto.

The buffer ownership question is now explicit: compare the currently attached
VB2 DMA address, Iris's cached address at firmware submission, and the address
returned by firmware, while the same CAPTURE index changes its backing buffer.
The matching VB2 source calls buf_init again when a different DMA-BUF is
attached, and Iris obtains the address in buf_init. Therefore ordinary slot
rebinding is expected to refresh the address; stale ownership is a hypothesis,
not an established cause. Reusing an address also does not establish that an
SMMU translation or a firmware reference points to the correct physical pages.

## User-run observation script

Close playing video tabs/apps before beginning. On a fresh boot with no kernel
faults, run:

```sh
sudo python3 /home/mq/oss/libva-v4l2/tools/trace-iris-buffer-ownership.py --seconds 45
```

After READY, play one 4K video in Chrome. Do not reopen a failed session. The
script observes the existing driver, starts no decoder, changes no module or
boot files, and removes only its own temporary tracing instance and probes.
It refuses a debug module file which differs from the loaded Iris build, and
stops collecting on a recorded kernel/decoder fault. Type offsets come from the
matching module's debug information and live videobuf2 DMA-contig BTF.

The result and raw trace are saved under the printed `/tmp/iris-buffer-ownership-*`
directory. Share **only `share.json`**: it replaces raw buffer addresses and
session IDs with consistent labels, retains counts and mismatch relationships,
and excludes boot IDs, file descriptors, process names, and kernel log lines.
Raw `result.json`, `trace.log`, and `kernel.log` remain local for detailed review.
`submitted_addresses_match` means observed submissions and responses
matched attached CAPTURE addresses and slot address or object-token changes
occurred, without reported trace loss or kernel fault. This does **not** prove physical DMA writes,
pixel correctness, guard-buffer isolation, sustained 4K60, or warm reopen safety.
Those require a subsequent bounded test with pixel comparison and guard buffers.
An observed address mismatch is actionable evidence; no mismatch narrows the
investigation toward translation, firmware references, or another ownership path.

Offline checks: Rust 291 passed / 4 ignored, Python 233 passed, all-targets
all-features Clippy passed. The RC8 release remains pending hardware correctness
and sustained playback/reopen qualification; Firefox 4K60 is not newly qualified.

## User trace: address rebinding observed

On the recovered boot, the user ran the observation script with the original
EL2 Iris module for 45 seconds. It captured 1,175 submissions, 1,091 nonempty
completions, five address changes at CAPTURE slot 0, four session identifiers,
no trace loss, no address mismatch, and no recorded kernel fault in the window.
The local trace contains 84 additional empty completion events, bringing all
completion events to 1,175. The reported count difference is therefore empty
markers, not evidence of 84 lost frames. Four identifiers do not prove four
playing tabs: decoder clients can create/recreate sessions within one tab.

This supports correct address refreshing for the observed slot changes. It
does not establish physical-write correctness, teardown safety, sustained 60fps,
or the cause of earlier page-table corruption. Distinct DMA-BUF objects can
reuse the same IOVA, so address labels must not be treated as object identities.
The updated script also labels the live DMA-BUF object and VB2 attachment,
counts object replacements at unchanged addresses, and reports identity coverage.
All object/attachment pointers remain masked in share.json. These tokens describe
objects during the trace, not identity across destruction and allocator reuse.

## Two additional user traces

Both version-2 reports used the original EL2 module, supplied object tokens on
every CAPTURE submission, recorded no trace loss, and found no submission or
nonempty-response address mismatch.

| Report directory suffix | Submissions | Nonempty completions | All completions | Object-token changes | Changes at the same DMA address |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1791093848 | 41 | 33 | 35 | 13 | 12 |
| 1791093923 | 2,701 | 2,518 | 2,700 | 2,518 | 2,518 |

The first trace contains two session identifiers. Their observed event spans
were approximately 0.039 and 0.185 seconds; six submissions in the second
session have no matching completion count within the recorded window. This is
not sufficient to distinguish cancelled/teardown buffers from a decode failure,
and must not be reported as a playback pass or six dropped frames.

The longer trace contains one session, spans approximately 42.199 seconds of
buffer events, and has one more submission than completion events. Its 182 empty
responses account for nearly all of the difference between submissions and
nonempty completions. These are firmware-handler observations, not presented
frame measurements, so they do not qualify sustained 4K60.

The longer trace observed 23 distinct object-pointer tokens at one DMA address
and one attachment-pointer token. Repeated object-token changes strengthen the
evidence against a stale cached DMA address during these submissions. A reused
attachment-pointer value can reflect allocator reuse; it does not prove a
single attachment remained alive for the entire trace. Neither report verifies
the physical pages behind a reused IOVA or cessation of firmware writes before
unmapping/freeing a retired buffer.

After both traces, the whole current boot's readable kernel journal contained
none of the checked fault signatures, and the Iris module reference count was
zero. No candidate driver was installed, and the reopen/corruption problem
remains unresolved. The next investigation concerns mapping/firmware ownership
across completion, STOP/CLOSE and teardown, rather than an unsupported change
to refresh an already matching cached address.

## Installed RC8: live Chrome CPU profile

After installation, a user-provided media-internals report selected
VaapiVideoDecoder for VP9 Profile 0 at 3840x2160. The Chrome GPU process mapped
the installed RC8 library inode. A 20-second CPU-clock profile of that GPU
process and the YouTube renderer recorded 2,966 samples without sample loss.
50.20% of the combined profile was in `__memcpy_oryon1`: 45.14 percentage points
through `capture_copy` / `Vec::spec_extend`, and 4.92 through
`SurfaceBacking::copy_decoded`. Another 10.11% was in the kernel data-abort
handler, with the dominant call chain again originating in snapshot memcpy.
Thus this playback performs real decoded-pixel CPU copying despite hardware
decoding; direct capture is not in use for those copied completions.

Source explicitly excludes contexts with declared caller-imported surfaces
from direct CAPTURE and excludes imported backings from deferred single-copy
publication. This can require a working-buffer snapshot followed by a copy
into caller storage. The current video's imported descriptor/geometry was not
inspected, so the exact mode-selection trigger is still unverified. The earlier
direct-buffer observations must not be generalized to every Chrome video.
The profile changed no driver or clock settings, and the current boot journal
remained free of the checked fault signatures. Fixing this CPU cost requires
addressing compatibility publication or imported-layout support; RC8's timing
diagnostics and kernel clock votes do not remove it.
