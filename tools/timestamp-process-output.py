#!/usr/bin/env python3
"""Timestamp a child's merged output; propagate its exit status unchanged."""
import argparse
import json
import subprocess
import sys
import time


def run(command, output, clock_output=None):
    started = time.monotonic_ns()
    if clock_output is not None:
        with open(clock_output, 'x') as clock:
            json.dump({'monotonic_ns': started, 'wall_time_ns': time.time_ns()}, clock)
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
        for line in child.stdout:
            elapsed = time.monotonic_ns() - started
            output.write(f"PROCESS_TIME_NS={elapsed} ".encode() + line)
            output.flush()
        return child.wait()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--clock-output')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command
    if command and command[0] == '--':
        command = command[1:]
    if not command:
        raise SystemExit('expected a child command')
    raise SystemExit(run(command, sys.stdout.buffer, args.clock_output))
