#!/usr/bin/env python3
"""Reject empty/partial Iris diagnostic output and compare canonical pixels."""
import argparse
from pathlib import Path
import re


def frames(path):
    result = []
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if not line.strip() or line.lstrip().startswith('#'):
            continue
        fields = [value.strip() for value in line.split(',')]
        if len(fields) != 6 or not re.fullmatch(r'[0-9a-fA-F]{32}', fields[5]):
            raise ValueError(f'{path}:{number}: malformed framemd5 record')
        try:
            numeric = [int(value) for value in fields[:5]]
        except ValueError as error:
            raise ValueError(f'{path}:{number}: invalid framemd5 fields') from error
        if numeric[0] != 0 or numeric[4] <= 0:
            raise ValueError(f'{path}:{number}: invalid video stream or empty frame')
        result.append((numeric[4], fields[5].lower()))
    return result


def check(actual, expected, reference=None):
    if expected <= 0:
        raise ValueError('expected frame count must be positive')
    observed = frames(actual)
    if len(observed) != expected:
        raise ValueError(f'{actual}: frames={len(observed)} expected={expected}')
    if reference is not None:
        baseline = frames(reference)
        if len(baseline) != expected or baseline != observed:
            raise ValueError(f'{actual}: native/driver pixel parity mismatch')
    return len(observed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('actual', type=Path)
    parser.add_argument('--expected', required=True, type=int)
    parser.add_argument('--reference', type=Path)
    args = parser.parse_args()
    try:
        count = check(args.actual, args.expected, args.reference)
    except (OSError, ValueError) as error:
        parser.exit(1, f'iris_probe=fail reason={error}\n')
    print(f'iris_probe=pass frames={count} parity={args.reference is not None}')


if __name__ == '__main__':
    main()
