# 08 · Reading Iris firmware errors from the kernel log

> **Status:** diagnostic reference. Written 2026-09-18 while investigating the
> `bframes-240p` small-stream failure (the long-standing `framemd5_xfail`) and
> the cross-session "poison" window. It records a way to see the **firmware
> side** of those failures without root, and what that evidence says.

## Why this chapter exists

For most of this project the small-stream abort and the cross-session wedge
were described as firmware black boxes: *"needs privileged `dmesg`/venus HFI
traces to continue."* That is only half true. The `dmesg` **syscall** is
blocked here (`kernel.dmesg_restrict = 1`), but `systemd-journald` already read
the same kernel ring buffer at boot, and this user can read the journal:

```sh
journalctl -k -b        # kernel messages, this boot, no root needed
```

So the firmware's own error events **are** visible. What is *not* visible
unprivileged is finer-grained HFI tracing: `qcom_iris` exposes no module
parameters, and `/sys/kernel/debug/dynamic_debug/control` needs root. So we can
see *that* the firmware raised an error and *which class* it was, but not the
individual HFI command that provoked it.

`tools/capture-iris-kernel-log.sh` wraps this up:

```sh
tools/capture-iris-kernel-log.sh summary          # all iris errors this boot
tools/capture-iris-kernel-log.sh -- <decode cmd>  # only errors during the run
```

## The two firmware error classes

The decoder is the upstream **`qcom-iris`** driver (`aa00000.video-codec`,
node `/dev/video16`) on the X1E80100 — *not* the older `venus` path the roadmap
prose sometimes refers to. Two distinct error events appear:

| Kernel line | Class | Meaning | Recovery |
|---|---|---|---|
| `session error received 0x4000003: fatal error` | **Session-fatal** | One decode session aborted in firmware. | Close + reopen the session. The Rust driver's bounded rebuild path already targets this. |
| `received system error of type 0x5000003` | **System-fatal** | Device-wide firmware crash. | Firmware is reloaded (`video hw is power on`); the node is unusable for ~90 s. Nothing userspace can do but wait. |

The high nibble is the scope: `0x4…` = session, `0x5…` = system. Each session
abort is printed as a burst of ~5 identical lines.

### The system-fatal signature (the "poison")

A `0x5000003` always comes with a kernel WARNING and a power cycle:

```
qcom-iris aa00000.video-codec: received system error of type 0x5000003
------------[ cut here ]------------
WARNING: drivers/media/common/videobuf2/videobuf2-core.c:1821
         at vb2_start_streaming+0x110/0x1c0 [videobuf2_common]
  Call trace: vb2_start_streaming <- vb2_core_streamon <- vb2_streamon
              <- v4l2_m2m_ioctl_streamon <- v4l2_ioctl <- ioctl
qcom-iris aa00000.video-codec: video hw is power on
```

The WARN fires because `VIDIOC_STREAMON` reached the firmware while it was
crashing, `start_streaming()` failed, and the iris driver returned the error
without handing its buffers back to vb2 (the `owned_by_drv_count != 0` check at
`videobuf2-core.c:1821`). That is arguably an upstream **iris/vb2 kernel bug**,
but it is only *triggered* by the firmware crash — it is not the cause.

## The single most important conclusion

**The device-wide crash is client-agnostic.** In this boot's journal the two
`vb2_start_streaming` WARNs were raised by two *different* userspace clients:

- `Comm: dec0:0:h264_v4l` — native `ffmpeg -c:v h264_v4l2m2m`.
- `Comm: queue0:src` — a GStreamer pipeline (this driver via `vah264dec`).

Native decode trips the identical firmware crash. So the abort/poison is **not**
a defect in this VAAPI→V4L2 translator. This is the kernel-side confirmation of
what the roadmap already deduced behaviorally (native `h264_v4l2m2m` also fails
the small-stream clip; the device self-recovers in ~90 s).

## What this means for the driver's strategy

Everything the driver already does is validated by this evidence, and the
strategy should **not** change:

1. `MAX_SESSION_RECOVERIES = 1` is correct. A single rebuild rescues a one-off
   `0x4000003`; a second abort means the firmware is heading toward `0x5000003`,
   and every extra `STREAMON` only feeds the crash. Never loop session opens.
2. `bframes-240p` stays an **xfail**, not a fail. It is a probabilistic
   firmware abort correlated with small picture height, reproducible on native
   decode, with no userspace fix available at the current privilege level.
3. Clients should treat small-stream decode failures as **retryable at the
   client**, after a delay long enough for firmware recovery.

## What would move this further (needs root)

The error *class* is now visible; the provoking HFI command is not. To get
that, someone with root on this box would:

- `echo 'module qcom_iris +p' > /sys/kernel/debug/dynamic_debug/control`
  (or the equivalent per-file lines) to enable the driver's debug prints, then
  re-run `tools/capture-iris-kernel-log.sh -- <failing small-stream decode>`.
- Compare the HFI property/command sequence the firmware receives for a failing
  small (`bframes-240p`, 320×240–512p) session against a passing 720p session.

Until then, this is the maximum-fidelity firmware evidence obtainable, and it is
enough to keep the current mitigations honest.

## Quick recipe

```sh
# See the whole boot's firmware error history, classified:
tools/capture-iris-kernel-log.sh summary

# Correlate a specific run (owns the shared node for its duration — do not run
# while another agent is using /dev/video16):
tools/capture-iris-kernel-log.sh -- \
  env LIBVA_DRIVERS_PATH=/tmp/libva-v4l2-rust-driver \
  ffmpeg -hide_banner -hwaccel vaapi -hwaccel_device /dev/dri/renderD128 \
         -i /home/mq/tmp/vaatest/bframes-240p.mp4 -f null -
```
