#!/usr/bin/env python3
"""Software-only AV1 visibility experiment; never establishes VA/hardware support."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import struct


def records(path):
    return [json.loads(line) for line in path.read_text().splitlines()]


def hashes(path):
    return [line.split(',')[-1].strip() for line in path.read_text().splitlines()
            if line and not line.startswith('#')]


def project(units, decoded):
    slots = [None] * 8
    display = []
    generation = 0
    for unit in units:
        if unit['type'] not in (3, 6):
            continue
        if unit['show_existing']:
            current = slots[unit['show_slot']]
            if current is None or unit['resolved_refresh']:
                raise ValueError('unsupported/unavailable show-existing reference')
            display.append(current)
        else:
            current = generation
            generation += 1
            if unit['show_frame']:
                display.append(current)
        for slot in range(8):
            if unit['resolved_refresh'] & (1 << slot):
                slots[slot] = current
    if generation != len(decoded):
        raise ValueError('decoded count differs from original coded-frame count')
    return [decoded[index] for index in display]


def tiles(data, units):
    offset = 0
    result = []
    for unit in units:
        size = unit['bytes']
        tile_size = unit['tile_bytes']
        if tile_size:
            start = offset + unit['tile_offset']
            tile = data[start:start + tile_size]
            if len(tile) != tile_size:
                raise ValueError('truncated tile data')
            result.append(hashlib.sha256(tile).hexdigest())
        offset += size
    if offset != len(data):
        raise ValueError('unit coverage mismatch')
    return result


def coded_packets(data, units):
    """One complete FRAME OBU per packet; reject layouts not yet modeled."""
    offset, pending, packets = 0, [], []
    for unit in units:
        size = unit['bytes']
        payload = data[offset:offset + size]
        offset += size
        if size <= 0 or len(payload) != size:
            raise ValueError('truncated normalized OBU')
        if unit['type'] in (1, 2):
            # No frame consumes superseded sequence headers before this frame.
            # MP4 extradata can differ from the immediately following inband header.
            if unit['type'] == 1:
                pending = [entry for entry in pending if entry[0] != 1]
            pending.append((unit['type'], payload))
        elif unit['type'] == 6:
            if unit['show_existing'] != 0 or unit['show_frame'] != 1:
                raise ValueError('normalized frame is not a coded shown frame')
            packets.append(b''.join(data for _, data in pending) + payload)
            pending = []
        else:
            raise ValueError('OBU layout not qualified for frame packetization')
    if offset != len(data) or not packets or pending:
        raise ValueError('incomplete normalized coded-frame packetization')
    return packets


def run(command, log):
    with log.open('x') as output:
        subprocess.run(command, stdout=output, stderr=subprocess.STDOUT, check=True, timeout=60)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('probe', type=Path)
    parser.add_argument('original_probe', type=Path)
    parser.add_argument('samples_json', type=Path)
    parser.add_argument('evidence', type=Path)
    parser.add_argument('--va-transport', action='store_true',
                        help='Exercise actual producer full-OBU transport helper, software only')
    args = parser.parse_args()
    args.evidence.mkdir(exist_ok=False)
    results = []
    for number, sample in enumerate(json.loads(args.samples_json.read_text())):
        root = args.evidence / str(number)
        root.mkdir()
        source = sample['sample']
        run([str(args.probe), source, str(root/'normalized.obu'), str(root/'index.jsonl')]
            + (['--va-transport'] if args.va_transport else []), root/'normalize.log')
        metadata = json.loads(subprocess.check_output(['ffprobe', '-v', 'error', '-select_streams', 'v:0',
                         '-show_entries', 'stream=width,height', '-of', 'json', source], text=True))['streams'][0]
        body = (root/'normalized.obu').read_bytes()
        offset, extra, packets = 0, b'', []
        for entry in records(root/'index.jsonl'):
            if entry['type'] != -1:
                continue
            data = body[offset:offset + entry['bytes']]
            offset += entry['bytes']
            if len(data) != entry['bytes']:
                raise ValueError('normalized packet coverage')
            if entry['packet'] == -1:
                extra += data
            else:
                packets.append(data)
        if offset != len(body) or not packets:
            raise ValueError('normalized body coverage')
        packets[0] = extra + packets[0]
        with (root/'normalized.ivf').open('xb') as output:
            output.write(struct.pack('<4sHH4sHHIIII', b'DKIF', 0, 32, b'AV01',
                                     metadata['width'], metadata['height'], 30, 1, len(packets), 0))
            for number, data in enumerate(packets):
                output.write(struct.pack('<IQ', len(data), number))
                output.write(data)
        for name, input_path in [('original', source), ('normalized', str(root/'normalized.obu'))]:
            run([str(args.original_probe), input_path, str(root/(name+'-units.obu')),
                 str(root/(name+'-units.jsonl'))] + (['--raw-obu'] if name == 'normalized' else []), root/(name+'-units.log'))
            run(['ffmpeg', '-nostdin', '-hide_banner', '-v', 'error', '-c:v', 'libdav1d',
                 '-i', str(root/'normalized.ivf') if name == 'normalized' else input_path, '-map', '0:v:0', '-an', '-fps_mode', 'passthrough',
                 '-f', 'framemd5', str(root/(name+'.md5'))], root/(name+'-decode.log'))
        framed_packets = coded_packets((root/'normalized-units.obu').read_bytes(), records(root/'normalized-units.jsonl'))
        with (root/'coded-frames.ivf').open('xb') as output:
            output.write(struct.pack('<4sHH4sHHIIII', b'DKIF', 0, 32, b'AV01',
                                     metadata['width'], metadata['height'], 30, 1, len(framed_packets), 0))
            for number, data in enumerate(framed_packets):
                output.write(struct.pack('<IQ', len(data), number))
                output.write(data)
        run(['ffmpeg', '-nostdin', '-hide_banner', '-v', 'error', '-nofind_stream_info',
             '-c:v', 'libdav1d', '-threads:v', '1', '-i', str(root/'coded-frames.ivf'),
             '-map', '0:v:0', '-an', '-fps_mode', 'passthrough', '-f', 'framemd5',
             str(root/'coded-frames.md5')], root/'coded-frames-decode.log')
        if hashes(root/'coded-frames.md5') != hashes(root/'normalized.md5'):
            raise ValueError('single coded-frame packetization changed pixel/order output')
        original_tiles = tiles((root/'original-units.obu').read_bytes(), records(root/'original-units.jsonl'))
        normalized_tiles = tiles((root/'normalized-units.obu').read_bytes(), records(root/'normalized-units.jsonl'))
        reference = hashes(root/'original.md5')
        projected = project(records(root/'index.jsonl'), hashes(root/'normalized.md5'))
        if not original_tiles or original_tiles != normalized_tiles:
            raise ValueError('compressed tile bytes/order changed')
        if reference != projected or len(reference) != sample['displayed_frames']:
            raise ValueError('original display pixel/order projection failed')
        results.append({'sample': source, 'status': 'pass', 'displayed_frames': len(reference),
                        'decoded_frames': len(hashes(root/'normalized.md5')),
                        'tile_groups_unchanged': len(original_tiles), 'pixel_parity': True,
                        'source_sha256': hashlib.sha256(Path(source).read_bytes()).hexdigest(),
                        'scope': 'software normalized visibility/reference projection only; no VA/hardware proof'})
        (args.evidence/'partial-result.json').write_text(json.dumps(results, indent=2)+'\n')
    (args.evidence/'result.json').open('x').write(json.dumps(results, indent=2)+'\n')
    print(json.dumps(results))


if __name__ == '__main__':
    main()
