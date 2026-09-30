#!/usr/bin/env python3
"""Compare every pixel checksum and timestamp against a complete repeated clip."""
import argparse
from pathlib import Path


def read_frames(path):
    headers, rows = [], []
    for line in Path(path).read_text().splitlines():
        if line.startswith('#'):
            if not line.startswith('#software:'):
                headers.append(line)
        elif line.strip():
            fields = [value.strip() for value in line.split(',')]
            if len(fields) != 6:
                raise ValueError(f'{path}: malformed frame record')
            rows.append(tuple(map(int, fields[:5])) + (fields[5],))
    return headers, rows


def compare(reference, actual, reference_frames, repeats):
    ref_headers, ref = read_frames(reference)
    got_headers, got = read_frames(actual)
    if reference_frames <= 0 or repeats <= 0:
        raise ValueError('frame count and repetitions must be positive')
    if len(ref) != reference_frames:
        raise ValueError(f'short reference: decoded={len(ref)} expected={reference_frames}')
    if len(got) != reference_frames * repeats:
        raise ValueError(f'actual frame count: decoded={len(got)} expected={reference_frames * repeats}')
    if ref_headers != got_headers:
        raise ValueError('frame format, dimensions, or time base differ')
    span = ref[-1][2] + ref[-1][3] - ref[0][2]
    if span <= 0:
        raise ValueError('invalid reference duration')
    for index, frame in enumerate(got):
        loop, offset = divmod(index, reference_frames)
        stream, dts, pts, duration, size, checksum = ref[offset]
        expected = (stream, dts + loop * span, pts + loop * span, duration, size, checksum)
        if frame != expected:
            raise ValueError(f'frame mismatch at index={index}: actual={frame} expected={expected}')
    return len(got)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('reference')
    parser.add_argument('actual')
    parser.add_argument('--reference-frames', required=True, type=int)
    parser.add_argument('--repeats', default=1, type=int)
    args = parser.parse_args()
    try:
        frames = compare(args.reference, args.actual, args.reference_frames, args.repeats)
    except (OSError, ValueError) as error:
        parser.exit(1, f'frame_parity=fail reason={error}\n')
    print(f'frame_parity=pass frames={frames} repeats={args.repeats}')


if __name__ == '__main__':
    main()
