#!/usr/bin/env python3
"""Bound a private process tree and sample summed descendant RSS on Linux.

Summed RSS counts shared pages more than once; this is a conservative browser
memory budget, not private/unique memory or a proof of no leaks. Sampling every
100 ms can miss shorter peaks. Descendants surviving the leader are a failure.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import time


def session_rss(session, tracked=None):
    tracked = {} if tracked is None else tracked
    records = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            # comm can contain spaces and parentheses: fields after the last
            # ')' start at state(3); session is field(6), RSS is field(24).
            fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
            if fields[0] != "Z":
                records[int(entry.name)] = (int(fields[1]), int(fields[3]), int(fields[19]),
                                            int(fields[21]) * os.sysconf("SC_PAGE_SIZE") // 1024)
        except (OSError, ValueError, IndexError):
            continue  # The process may exit between enumeration and stat.
    members = {pid for pid, (_, sid, started, _) in records.items()
               if sid == session or tracked.get(pid) == started}
    # PPID membership catches descendants that create new sessions. Retain
    # observed PID + starttime identity after reparenting without PID reuse.
    while True:
        children = {pid for pid, (parent, _, _, _) in records.items() if parent in members}
        added = children - members
        if not added:
            break
        members.update(added)
    for pid in members:
        tracked[pid] = records[pid][2]
    return sum(records[pid][3] for pid in members), sorted(members)


def terminate_session(session, tracked):
    # Browser subprocesses may choose a new process group within our session.
    # Signal every current member rather than only the leader's process group.
    denied = []
    for sig, grace in ((signal.SIGTERM, 5), (signal.SIGKILL, 1)):
        _, members = session_rss(session, tracked)
        for pid in members:
            try:
                os.kill(pid, sig)
            except ProcessLookupError:
                pass
            except PermissionError as exc:
                denied.append({"pid": pid, "signal": int(sig), "errno": exc.errno})
        until = time.monotonic() + grace
        while time.monotonic() < until:
            _, members = session_rss(session, tracked)
            if not members:
                return {"signal_denied": denied, "unresolved_pids": []}
            time.sleep(0.1)
    return {"signal_denied": denied, "unresolved_pids": session_rss(session, tracked)[1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--seconds", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--run-id")
    parser.add_argument("--inherit-fd", type=int, action="append", default=[],
                        help="keep a reviewed lease descriptor in the private child")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not 1 <= args.seconds <= 3600 or not command:
        parser.error("provide a command and a timeout between 1 and 3600 seconds")
    for fd in args.inherit_fd:
        if fd < 3:
            parser.error("inherited descriptors must be above stderr")
        try:
            os.fstat(fd)
        except OSError:
            parser.error("inherited descriptor is not open")
    start = time.monotonic()
    peak = 0
    timed_out = False
    tracked = {}
    interrupted_run = False
    lingering = False
    cleanup = {"signal_denied": [], "unresolved_pids": []}
    def interrupted(_signal, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    process = subprocess.Popen(command, start_new_session=True,
                               pass_fds=tuple(args.inherit_fd))
    try:
        while process.poll() is None:
            current, _ = session_rss(process.pid, tracked)
            peak = max(peak, current)
            if time.monotonic() - start >= args.seconds:
                timed_out = True
                break
            time.sleep(0.1)
        # Give ordinary browser child teardown a short bounded grace window.
        until = time.monotonic() + 2
        while not timed_out and time.monotonic() < until and session_rss(process.pid, tracked)[1]:
            time.sleep(0.1)
        lingering = bool(session_rss(process.pid, tracked)[1])
    except KeyboardInterrupt:
        interrupted_run = True
    finally:
        cleanup = terminate_session(process.pid, tracked)
        # A denied signal or an uninterruptible task must not turn this
        # bounded observer into an indefinite wait. Preserve its identity
        # as unfinished evidence; never claim clean shutdown in that case.
        status = process.poll()
        if interrupted_run:
            status = 130
        elif timed_out:
            status = 124
        elif lingering or cleanup["signal_denied"] or cleanup["unresolved_pids"] or status is None:
            status = 1
        args.output.write_text(json.dumps({
            "elapsed_s": time.monotonic() - start,
            "peak_rss_kib": peak,
            "exit_status": status,
            "timed_out": timed_out,
            "lingering_descendants": lingering,
            "interrupted": interrupted_run,
            **cleanup,
            "memory_method": "100ms_sum_observed_tree_rss_shared_pages_counted_per_process",
            "run_id": args.run_id,
        }, allow_nan=False) + "\n")
    return status if status is not None and 0 <= status <= 255 else 1


if __name__ == "__main__":
    raise SystemExit(main())
