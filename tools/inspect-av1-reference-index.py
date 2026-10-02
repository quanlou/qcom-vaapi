#!/usr/bin/env python3
"""Audit parsed original AV1 headers; no hardware or pixel ownership proof.

Uses the CBS probe's resolved refresh mask, including show-existing key frames.
Frame numbers are host audit identities, never VA surfaces or DMA handles.
"""
import argparse
import json
from pathlib import Path


def inspect(records):
    slots = [None] * 8
    frames = []
    aliases = []
    displayed = 0
    for unit in records:
        # Redundant headers do not represent an additional decoded picture.
        if unit['type'] not in (3, 6):
            continue
        refresh = unit['resolved_refresh']
        if not isinstance(refresh, int) or not 0 <= refresh <= 255:
            raise ValueError('invalid resolved refresh mask')
        if unit['show_existing'] == 1:
            slot = unit['show_slot']
            if not isinstance(slot, int) or not 0 <= slot < 8 or slots[slot] is None:
                raise ValueError('show-existing refers to unavailable slot')
            current = slots[slot]
            aliases.append({'packet': unit['packet'], 'slot': slot,
                            'frame': current, 'hidden': frames[current]['hidden']})
            displayed += 1
        elif unit['show_existing'] == 0 and unit['show_frame'] in (0, 1):
            current = len(frames)
            frames.append({'packet': unit['packet'],
                           'hidden': unit['show_frame'] == 0})
            displayed += unit['show_frame']
        else:
            raise ValueError('missing original frame visibility')
        for slot in range(8):
            if refresh & (1 << slot):
                slots[slot] = current
    return {'decoded_headers': len(frames),
            'hidden_headers': sum(frame['hidden'] for frame in frames),
            'display_events': displayed, 'show_existing_events': len(aliases),
            'hidden_reference_aliases': sum(alias['hidden'] for alias in aliases),
            'aliases': aliases,
            'scope': 'host original-header reference index only; not pixels, '
                     'VA surface lifetimes, hardware or complete AV1 conformance'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('index', type=Path)
    args = parser.parse_args()
    records = [json.loads(line) for line in args.index.read_text().splitlines()]
    print(json.dumps(inspect(records), indent=2))


if __name__ == '__main__':
    main()
