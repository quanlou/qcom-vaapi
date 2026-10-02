#!/usr/bin/env python3
"""Drive a bounded storm of seeks over a running mpv's JSON IPC socket.

Companion of tools/verify-seek-storm.sh, which starts mpv with
--input-ipc-server and --loop=inf, then calls this helper to issue absolute
keyframe seeks spread across the file, and finally "quit".

Replies are matched by request ID with bounded timeouts. Every seek must
be acknowledged, followed by a fresh seek/restart event pair, seeking=false
and forward playback progress while hardware decoding stays active. Keyframe
landing may differ from the target. The shell separately checks process exit
and post-storm decoder sanity; this controller does not prove pixel parity.
"""

import json
import socket
import sys
import time
import math

# Seek target fractions, cycled; alternates deep jumps and cross-midpoint
# jumps so both directions are exercised repeatedly.
FRACTIONS = (0.15, 0.85, 0.4, 0.7, 0.1, 0.95, 0.3, 0.6)

SOCKET_WAIT_SECONDS = 20.0
RESPONSE_TIMEOUT = 0.5
SEEK_COMPLETION_SECONDS = 4.0
SEEK_POLL_INTERVAL = 0.05
MIN_PROGRESS_SECONDS = 0.005


def connect(sock_path):
    deadline = time.monotonic() + SOCKET_WAIT_SECONDS
    while time.monotonic() < deadline:
        sock = socket.socket(socket.AF_UNIX)
        try:
            sock.connect(sock_path)
            return sock
        except OSError:
            sock.close()
            time.sleep(0.25)
    return None


class PropertyUnavailable(RuntimeError):
    """mpv has not made the requested property available."""


class RpcClient:
    """Match replies by request ID; asynchronous events are not acknowledgments."""
    def __init__(self, sock):
        self.sock = sock
        self.pending = b""
        self.request_id = 0
        self.event_sequence = 0
        self.last_seek_event = 0
        self.last_restart_event = 0

    def request(self, command, deadline=None):
        self.request_id += 1
        request_id = self.request_id
        self.sock.sendall((json.dumps({"command": command, "request_id": request_id}) + "\n").encode())
        response_deadline = time.monotonic() + RESPONSE_TIMEOUT
        deadline = response_deadline if deadline is None else min(deadline, response_deadline)
        while time.monotonic() < deadline:
            if b"\n" in self.pending:
                line, self.pending = self.pending.split(b"\n", 1)
                response = json.loads(line)
                if response.get("event"):
                    self.event_sequence += 1
                    if response["event"] == "seek":
                        self.last_seek_event = self.event_sequence
                    elif response["event"] == "playback-restart":
                        self.last_restart_event = self.event_sequence
                if response.get("request_id") != request_id:
                    continue
                if response.get("error") != "success":
                    error_type = PropertyUnavailable if response.get("error") == "property unavailable" else RuntimeError
                    raise error_type(f"mpv rejected {command}: {response.get('error')}")
                return response.get("data")
            self.sock.settimeout(max(0.001, deadline - time.monotonic()))
            data = self.sock.recv(65536)
            if not data:
                raise RuntimeError("mpv disconnected before acknowledging the command")
            self.pending += data
        raise TimeoutError(f"mpv did not acknowledge {command}")


def wait_for_seek(client, event_marker):
    """Require fresh completion and two forward playback advances, not a clock jump.

    mpv's seeking property describes restart state; playback-restart follows a
    seek. See https://mpv.io/manual/stable/#list-of-events. Track those events
    without allowing them to satisfy the seek command's request-ID reply.
    """
    deadline = time.monotonic() + SEEK_COMPLETION_SECONDS
    landed = None
    previous = None
    advances = 0
    while time.monotonic() < deadline:
        hwdec = client.request(["get_property", "hwdec-current"], deadline=deadline)
        if hwdec != "vaapi-copy":
            raise RuntimeError("hardware decoding stopped after seek")
        seeking = client.request(["get_property", "seeking"], deadline=deadline)
        if not isinstance(seeking, bool):
            raise RuntimeError("invalid seeking state after seek")
        completed = (client.last_seek_event > event_marker
                     and client.last_restart_event > client.last_seek_event)
        if not seeking and completed:
            position = client.request(["get_property", "time-pos"], deadline=deadline)
            if (isinstance(position, bool)
                    or not isinstance(position, (int, float))
                    or not math.isfinite(position)):
                raise RuntimeError("no finite playback position after seek")
            if previous is None:
                landed = position
            elif position < previous - MIN_PROGRESS_SECONDS:
                raise RuntimeError("playback restarted before post-seek progress was proven")
            elif position > previous + MIN_PROGRESS_SECONDS:
                advances += 1
                if advances >= 2:
                    return landed, position
            previous = position
        else:
            # A new restart invalidates any samples collected before it.
            landed = previous = None
            advances = 0
        time.sleep(min(SEEK_POLL_INTERVAL, max(0, deadline - time.monotonic())))
    raise TimeoutError("seek did not complete with forward playback progress")


def drive(sock, count, duration):
    if count <= 0 or not math.isfinite(duration) or duration <= 0:
        raise ValueError("seek count and finite duration must be positive")
    client = RpcClient(sock)
    deadline = time.monotonic() + 10
    while True:
        try:
            hwdec = client.request(["get_property", "hwdec-current"])
        except PropertyUnavailable:
            # IPC appears before decoder initialization. Only startup may wait;
            # property loss after a seek still fails the probe immediately.
            hwdec = None
        if hwdec == "vaapi-copy":
            break
        if hwdec not in (None, "no", "") or time.monotonic() >= deadline:
            raise RuntimeError(f"hardware decode is not active: {hwdec}")
        time.sleep(0.1)
    for i in range(count):
        target = min(max(0.0, duration * FRACTIONS[i % len(FRACTIONS)]), max(0.0, duration - 0.25))
        event_marker = client.event_sequence
        client.request(["seek", round(target, 2), "absolute+keyframes"])
        landed, position = wait_for_seek(client, event_marker)
        print(f"seek_completed index={i} target={target:.3f} "
              f"landed={landed:.3f} progressed={position:.3f}", flush=True)
    print(f"seek_commands=pass acknowledged={count} hardware=vaapi-copy "
          f"completed={count} progressed={count}", flush=True)
    # quit can close IPC before the response reaches the reader. All seeks
    # have already been acknowledged; the shell separately checks mpv's exit.
    try:
        client.request(["quit"])
    except (OSError, RuntimeError, TimeoutError):
        pass


def main():
    if len(sys.argv) != 4:
        sys.stderr.write("usage: mpv_seek_drive.py SOCK_PATH SEEK_COUNT DURATION_SECONDS\n")
        return 2
    sock = None
    try:
        count = int(sys.argv[2])
        duration = float(sys.argv[3])
        sock = connect(sys.argv[1])
        if sock is None:
            raise RuntimeError("ipc socket never appeared")
        drive(sock, count, duration)
    except (OSError, RuntimeError, ValueError, TimeoutError) as error:
        sys.stderr.write(f"mpv_seek_drive: {error}\n")
        return 1
    finally:
        if sock is not None:
            sock.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
