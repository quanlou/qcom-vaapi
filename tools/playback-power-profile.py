#!/usr/bin/env python3
"""Reserve GPU headroom during active Iris decoding on Snapdragon X Elite.

This opt-in host policy never changes CPU clocks or the GPU's maximum. Restore
the original GPU minimum when Iris suspends, disappears, or the service stops.
"""
import argparse
import signal
import time
from pathlib import Path

GPU = Path('/sys/class/devfreq/3d00000.gpu')
IRIS = Path('/sys/module/qcom_iris/refcnt')
POWER = Path('/sys/bus/platform/devices/aa00000.video-codec/power/runtime_status')
FLOOR = 550_000_000


def decoder_active(refs, status):
    return int(refs) > 0 and status.strip() == 'active'


class Profile:
    def __init__(self, gpu):
        self.minimum = gpu / 'min_freq'
        frequencies = {int(x) for x in (gpu / 'available_frequencies').read_text().split()}
        if FLOOR not in frequencies:
            raise ValueError('550 MHz is not an available GPU frequency')
        self.original = int(self.minimum.read_text())
        self.applied = False

    def update(self, active):
        if active and not self.applied and self.original < FLOOR:
            self.minimum.write_text(str(FLOOR))
            self.applied = True
        elif not active:
            self.restore()

    def restore(self):
        if self.applied:
            self.minimum.write_text(str(self.original))
            self.applied = False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--seconds', type=float, help='Bound a diagnostic run')
    args = parser.parse_args()
    if b'qcom,x1e80100\0' not in Path('/proc/device-tree/compatible').read_bytes():
        raise SystemExit('This policy is qualified only on X1E80100')
    profile = Profile(GPU)
    running = True

    def stop(*_):
        nonlocal running
        running = False

    signal.signal(signal.SIGINT, stop)
    signal.signal(signal.SIGTERM, stop)
    deadline = time.monotonic() + args.seconds if args.seconds is not None else None
    try:
        while running and (deadline is None or time.monotonic() < deadline):
            try:
                active = decoder_active(IRIS.read_text(), POWER.read_text())
            except (OSError, ValueError):
                active = False
            profile.update(active)
            time.sleep(0.2)
    finally:
        profile.restore()


if __name__ == '__main__':
    main()
