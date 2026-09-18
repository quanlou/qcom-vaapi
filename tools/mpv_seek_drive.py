#!/usr/bin/env python3
"""Drive a bounded storm of seeks over a running mpv's JSON IPC socket.

Companion of tools/verify-seek-storm.sh, which starts mpv with
--input-ipc-server and --loop=inf, then calls this helper to issue absolute
keyframe seeks spread across the file, and finally "quit".

Responses are drained with a short timeout but never checked: mpv liveness
and the post-storm decoder sanity are asserted by the shell probe, so the
driver stays fire-and-forget and cannot deadlock on event floods.
"""

import json
import socket
import sys
import time

# Seek target fractions, cycled; alternates deep jumps and cross-midpoint
# jumps so both directions are exercised repeatedly.
FRACTIONS = (0.15, 0.85, 0.4, 0.7, 0.1, 0.95, 0.3, 0.6)

SOCKET_WAIT_SECONDS = 20.0
RESPONSE_TIMEOUT = 0.5
SEEK_INTERVAL = 0.3


def connect(sock_path):
    deadline = time.time() + SOCKET_WAIT_SECONDS
    while time.time() < deadline:
        sock = socket.socket(socket.AF_UNIX)
        try:
            sock.connect(sock_path)
            return sock
        except OSError:
            sock.close()
            time.sleep(0.25)
    return None


def main():
    if len(sys.argv) != 4:
        sys.stderr.write(
            "usage: mpv_seek_drive.py SOCK_PATH SEEK_COUNT DURATION_SECONDS\n"
        )
        return 2
    sock_path = sys.argv[1]
    count = int(sys.argv[2])
    duration = float(sys.argv[3])

    sock = connect(sock_path)
    if sock is None:
        sys.stderr.write("mpv_seek_drive: ipc socket never appeared\n")
        return 3

    f = sock.makefile("rw")
    sock.settimeout(RESPONSE_TIMEOUT)

    def send(command):
        try:
            f.write(json.dumps({"command": command}) + "\n")
            f.flush()
            f.readline()  # best-effort drain: response or an event
        except (OSError, ValueError):
            pass  # mpv exited; the shell probe judges liveness

    for i in range(count):
        frac = FRACTIONS[i % len(FRACTIONS)]
        target = duration * frac
        if duration > 0.25:
            target = min(max(0.0, target), duration - 0.25)
        # mpv 0.41 rejects singular "absolute+keyframe" ("invalid
        # parameter"); the accepted flag is plural "absolute+keyframes".
        send(["seek", round(target, 2), "absolute+keyframes"])
        time.sleep(SEEK_INTERVAL)

    # mpv 0.41 rejects the bare-string command form ("invalid parameter"),
    # so every command — quit included — is sent as an argument array.
    send(["quit"])
    f.close()
    sock.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
