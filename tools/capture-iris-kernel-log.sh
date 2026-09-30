#!/usr/bin/env bash
# Capture qcom-iris firmware/V4L2 kernel messages around a decode run.
#
# The bframes-240p (small-stream) failure and the cross-session "poison" window
# are firmware-side. The roadmap long assumed root-only `dmesg`/venus HFI traces
# were required to see them. They are not: `kernel.dmesg_restrict=1` blocks the
# `dmesg` syscall, but `journalctl -k` reads the same kernel ring from the
# journal without privileges on this host. This script operationalizes that.
#
# Two modes:
#   summary                 Summarize every iris firmware error seen this boot,
#                           with an escalation timeline (session vs system).
#   <cmd> [args...]         Snapshot the journal cursor, run the command, then
#                           report only the iris/vb2 kernel lines emitted during
#                           that window and classify them.
#
# Firmware error classes observed on qcom-iris aa00000.video-codec (x1e80100):
#   session error 0x4000003 : per-session fatal abort. Recoverable by closing
#                             and reopening the session. This is the signature
#                             behind the small-stream (<=~512px height) abort and
#                             the Rust driver's bounded rebuild path.
#   system  error 0x5000003 : device-wide fatal error. Forces a firmware reload
#                             ("video hw is power on") and trips a WARN in
#                             vb2_start_streaming because the iris driver fails
#                             start_streaming without returning buffers to vb2.
#                             This is the ~90s "poisoned node" state; it fires
#                             even for native h264_v4l2m2m, so it is not a bug in
#                             this VAAPI driver.
#
# HFI-level detail (the exact firmware command that provoked 0x4000003) needs
# root: dynamic_debug/debugfs and a writable iris log-level param are not exposed
# unprivileged on this host, and qcom_iris has no module parameters. This script
# captures everything obtainable without privilege.
#
# Usage:
#   tools/capture-iris-kernel-log.sh summary
#   tools/capture-iris-kernel-log.sh -- <command> [args...]
#   tools/capture-iris-kernel-log.sh env LIBVA_DRIVERS_PATH=/tmp/build \
#       ffmpeg -hwaccel vaapi -i clip.mp4 -f null -
set -u
set -o pipefail

IRIS_RE='qcom-iris|session error|system error|video hw is power|vb2_start_streaming|videobuf2-core.c|iris|UBSAN:|KASAN:|BUG:|WARNING:'
NOISE_RE='Modules linked in|snd_|pinctrl_|phy_|qcom_glink|videobuf2_dma|drm_|x[0-9]+ :|Call trace|Hardware name|pstate:|Tainted|CPU:|sp :|pc :|lr :'

filter_messages() {
  # WARNING headers include CPU:, which is otherwise stack-trace noise.
  # Keep fault headers before dropping noise so a warning cannot disappear.
  awk -v message_re="$IRIS_RE" -v noise="$NOISE_RE" '
    /UBSAN:|KASAN:|BUG:|WARNING:/ { print; next }
    $0 ~ message_re && $0 !~ noise { print }
  '
}

have_journal() { command -v journalctl >/dev/null 2>&1 && journalctl -k -n0 >/dev/null 2>&1; }

classify() {
  # Reads kernel lines on stdin, prints a per-line classification + a summary.
  awk '
    /session error received 0x4000003/ { sess++; print "  [SESSION-FATAL 0x4000003] " $0; next }
    /received system error of type 0x5000003/ { sys++; print "  [SYSTEM-FATAL  0x5000003] " $0; next }
    /session error received/ { sess_other++; print "  [SESSION-ERR ] " $0; next }
    /received system error/  { sys_other++;  print "  [SYSTEM-ERR  ] " $0; next }
    /video hw is power on/   { power++; print "  [POWER-CYCLE ] " $0; next }
    /WARNING: .*vb2_start_streaming|videobuf2-core.c:.* vb2_start_streaming/ { warn++; print "  [VB2-WARN    ] " $0; next }
    /UBSAN:|KASAN:|BUG:|WARNING:/ { bugs++; print "  [KERNEL-BUG  ] " $0; next }
    { print "  [.............] " $0 }
    END {
      print ""
      printf "  summary: session-fatal(0x4000003)=%d  system-fatal(0x5000003)=%d  power-cycles=%d  vb2-warns=%d  other-session=%d  other-system=%d  kernel-bugs=%d\n", \
        sess+0, sys+0, power+0, warn+0, sess_other+0, sys_other+0, bugs+0
      if (bugs+0 > 0) print "  verdict: kernel warning or memory-safety fault; quality gate failed."
      else if (sys+0 > 0)  print "  verdict: DEVICE-WIDE firmware crash occurred -> node poisoned, expect ~90s recovery. Even native decode will fail during this window."
      else if (sess+0 > 0) print "  verdict: per-session firmware abort(s) only -> recoverable; matches the small-stream/bframes signature."
      else if (warn+sess_other+sys_other > 0) print "  verdict: kernel/firmware errors occurred; quality gate failed."
      else print "  verdict: no iris firmware errors in this window (clean)."
      if (bugs+warn+sess+sys+sess_other+sys_other > 0) exit 1
    }'
}

# Host-only fixture mode exercises the same classifier as live probes.
if [[ "${1:-}" == --classify ]]; then
  classify
  exit $?
fi

if ! have_journal; then
  echo "capture-iris-kernel-log: journalctl -k is unavailable; cannot read kernel messages." >&2
  exit 77
fi

journal_file="$(mktemp)" || exit 1
trap 'rm -f "$journal_file"' EXIT
mode="${1:-summary}"

if [ "$mode" = "summary" ]; then
  echo "== qcom-iris firmware error summary (this boot) =="
  if ! journalctl -k -b -o short-precise > "$journal_file" 2>/dev/null; then
    echo "capture-iris-kernel-log: failed to read the journal." >&2
    exit 77
  fi
  filter_messages < "$journal_file" | classify
  exit $?
fi

# Wrapped-command mode. Support an optional leading "--".
[ "$mode" = "--" ] && shift
if [ "$#" -eq 0 ]; then
  echo "usage: $0 summary | [--] <command> [args...]" >&2
  exit 2
fi

# Snapshot the newest kernel cursor so we only report lines emitted after it.
cursor="$(journalctl -k -n0 --show-cursor 2>/dev/null | grep -aoP 'cursor: \K.*')"
start_epoch="$(date +%s)"
echo "== running: $* =="
"$@"
rc=$?
echo "== command exited rc=$rc =="

# Give journald a moment to flush late kernel lines from the run.
sleep 1

echo "== iris/vb2 kernel messages during the run =="
if [ -n "$cursor" ]; then
  journalctl -k --after-cursor "$cursor" -o short-precise > "$journal_file" 2>/dev/null
else
  # Fallback: time-window filter if cursor capture failed.
  journalctl -k --since "@$start_epoch" -o short-precise > "$journal_file" 2>/dev/null
fi
journal_rc=$?
if [[ "$journal_rc" -ne 0 ]]; then
  echo "capture-iris-kernel-log: failed to read the run's journal window." >&2
  [[ "$rc" -ne 0 ]] && exit "$rc"
  exit 77
fi
filter_messages < "$journal_file" | classify
classification_rc=$?
if [[ "$rc" -ne 0 ]]; then
  exit "$rc"
fi
exit "$classification_rc"
