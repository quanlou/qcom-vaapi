#!/usr/bin/env python3
"""Prepare exact-module memory tracing or analyze a saved trace; never attach.

The generated probe observes HFI submissions, release requests and DMA frees.
An address match identifies a reported range, not a valid mapping or the cause
of a fault. A future operator wrapper must enforce identities, clean boot,
shared lease, observer and process cleanup before attaching or decoding.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import subprocess


FIELDS = {
    'session': ('iris_inst', 'session_id', 'uint32'),
    'core': ('iris_inst', 'core', 'uint64'),
    'codec': ('iris_inst', 'codec', 'uint32'),
    'device': ('iris_core', 'dev', 'uint64'),
    'type': ('iris_buffer', 'type', 'uint32'),
    'index': ('iris_buffer', 'index', 'uint32'),
    'size': ('iris_buffer', 'buffer_size', 'uint64'),
    'offset': ('iris_buffer', 'data_offset', 'uint32'),
    'data': ('iris_buffer', 'data_size', 'uint64'),
    'base': ('iris_buffer', 'device_addr', 'uint64'),
    'timestamp': ('iris_buffer', 'timestamp', 'uint64'),
    'attr': ('iris_buffer', 'attr', 'uint32'),
}
SYMBOLS = {
    'queue': 'iris_hfi_gen2_session_queue_buffer',
    'release': 'iris_hfi_gen2_session_release_buffer',
    'destroy': 'iris_destroy_internal_buffer',
}
BUFFER_KEYS = {'time', 'tid', 'session', 'codec', 'type', 'index', 'base',
               'size', 'offset', 'data', 'timestamp', 'attr'}


def sha(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def layout(module):
    note = subprocess.check_output(['readelf', '-n', str(module)], text=True, timeout=10)
    matches = re.findall(r'Build ID: ([0-9a-f]+)', note)
    if len(matches) != 1:
        raise ValueError('exact module build identity required')
    command = ['gdb', '-q', '-batch', str(module)]
    for struct, field, _ in FIELDS.values():
        command += ['-ex', f'p/d (unsigned long)&((struct {struct} *)0)->{field}']
    output = subprocess.check_output(command, text=True, stderr=subprocess.STDOUT, timeout=30)
    offsets = [int(value) for value in re.findall(r'^\$\d+ = (\d+)$', output, re.M)]
    if len(offsets) != len(FIELDS) or any(value > 65536 for value in offsets):
        raise ValueError('DWARF layout unavailable or ambiguous')
    symbols = subprocess.check_output(['nm', '-an', str(module)], text=True, timeout=10)
    for symbol in SYMBOLS.values():
        if not re.search(r'^[0-9a-f]+ [tT] ' + re.escape(symbol) + r'$', symbols, re.M):
            raise ValueError('exact probe symbol unavailable: ' + symbol)
    return {'module_sha256': sha(module), 'build_id': matches[0],
            'offsets': dict(zip(FIELDS, offsets)), 'gdb_output': output}


def program(offsets):
    def read(key, arg):
        return f'*({FIELDS[key][2]}*)({arg}+{offsets[key]})'
    fields = ['session', 'codec', 'type', 'index', 'base', 'size', 'offset', 'data', 'timestamp', 'attr']
    fmt = ' '.join(key + '=%llu' for key in fields)
    args = ', '.join(read(key, 'arg0' if key in ['session', 'codec'] else 'arg1') for key in fields)
    blocks = ['// Exact DWARF offsets; prepare-only, not an authorized live invocation.\n'
              'BEGIN { printf("IRIS_MEMORY_READY version=1\\n"); }']
    for op, symbol in SYMBOLS.items():
        store = '' if op == 'destroy' else f'@{op}[tid] = $now;'
        blocks.append(f'''kprobe:{symbol} {{
  $now = nsecs(monotonic);
  $core = {read('core', 'arg0')};
  $device = {read('device', '$core')};
  @devices[$device] = 1;
  {store}
  printf("IRIS_MEMORY_BUFFER op={op} time=%llu tid=%llu {fmt}\\n", $now, tid, {args});
}}''')
        if op != 'destroy':
            blocks.append(f'''kretprobe:{symbol} /@{op}[tid]/ {{
  printf("IRIS_MEMORY_RETURN op={op} time=%llu tid=%llu entry=%llu retval=%lld\\n", nsecs(monotonic), tid, @{op}[tid], (int64)retval);
  delete(@{op}[tid]);
}}''')
    blocks += ['''kprobe:dma_free_attrs /@devices[arg0]/ {
  printf("IRIS_MEMORY_FREE time=%llu base=%llu size=%llu\\n", nsecs(monotonic), arg3, arg1);
}''', '''interval:s:120 { printf("IRIS_MEMORY_LIMIT\\n"); exit(); }
END {
  clear(@devices); clear(@queue); clear(@release);
  printf("IRIS_MEMORY_STOPPED\\n");
}''']
    return '\n\n'.join(blocks) + '\n'


def analyze(trace, errors, address, fault_ns):
    if not 0 <= address < 2**64 or fault_ns <= 0:
        raise ValueError('invalid fault address/time')
    if errors.strip():
        raise ValueError('tracer diagnostics must be reviewed; no complete evidence claim')
    lines = trace.splitlines()
    if (lines.count('IRIS_MEMORY_READY version=1') != 1 or
            lines.count('IRIS_MEMORY_STOPPED') != 1 or 'IRIS_MEMORY_LIMIT' in lines or
            not lines or lines[0] != 'IRIS_MEMORY_READY version=1' or lines[-1] != 'IRIS_MEMORY_STOPPED'):
        raise ValueError('missing, truncated or time-limited trace')
    buffers, returns, frees = [], {}, []
    for line in lines[1:-1]:
        parts = line.split()
        if not parts:
            raise ValueError('empty trace record')
        record = {}
        for item in parts[1:]:
            key, separator, value = item.partition('=')
            if not separator or key in record:
                raise ValueError('malformed trace record')
            record[key] = value if key == 'op' else int(value)
        if parts[0] == 'IRIS_MEMORY_BUFFER':
            if set(record) != BUFFER_KEYS | {'op'} or record['op'] not in SYMBOLS:
                raise ValueError('malformed buffer observation')
            if (any(record[key] < 0 for key in BUFFER_KEYS) or record['base'] >= 2**64 or
                    record['size'] <= 0 or record['base'] + record['size'] > 2**64):
                raise ValueError('invalid buffer extent')
            buffers.append(record)
        elif parts[0] == 'IRIS_MEMORY_RETURN':
            if set(record) != {'op', 'time', 'tid', 'entry', 'retval'} or record['op'] not in ['queue', 'release']:
                raise ValueError('malformed return observation')
            key = (record['op'], record['tid'], record['entry'])
            if key in returns or record['time'] < record['entry'] or record['entry'] < 0 or record['tid'] < 0:
                raise ValueError('ambiguous return observation')
            returns[key] = record
        elif parts[0] == 'IRIS_MEMORY_FREE':
            if (set(record) != {'time', 'base', 'size'} or any(value < 0 for value in record.values()) or
                    record['size'] <= 0 or record['base'] + record['size'] > 2**64):
                raise ValueError('malformed DMA free observation')
            frees.append(record)
        else:
            raise ValueError('unknown or dropped trace output')
    entries = {(row['op'], row['tid'], row['time']) for row in buffers if row['op'] != 'destroy'}
    if len(entries) != sum(row['op'] != 'destroy' for row in buffers) or not set(returns).issubset(entries):
        raise ValueError('unpaired or duplicate submission identifiers')
    matches = []
    for row in buffers:
        if row['op'] != 'queue' or row['time'] > fault_ns or not row['base'] <= address < row['base'] + row['size']:
            continue
        returned = returns.get(('queue', row['tid'], row['time']))
        if returned and (returned['retval'] != 0 or returned['time'] > fault_ns):
            continue
        freed = [free for free in frees if row['time'] <= free['time'] <= fault_ns and
                 free['base'] == row['base'] and free['size'] == row['size']]
        matches.append(dict(row, fault_offset=address-row['base'],
                            submission_returned_zero=returned is not None,
                            exact_dma_free_after_submission=bool(freed)))
    return {'status': 'analysis_only', 'fault_address': hex(address), 'fault_monotonic_ns': fault_ns,
            'observed_submissions': sum(row['op'] == 'queue' for row in buffers),
            'unreturned_calls': len(entries - set(returns)), 'matching_reported_ranges': matches,
            'scope': 'Reported HFI buffer ranges only; no match does not prove an unmapped address. '
                     'A match or DMA free does not establish causation or valid mapping lifetime.'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='action', required=True)
    prepare = commands.add_parser('prepare')
    prepare.add_argument('module', type=Path)
    prepare.add_argument('destination', type=Path)
    report = commands.add_parser('analyze')
    report.add_argument('trace', type=Path)
    report.add_argument('errors', type=Path)
    report.add_argument('--address', type=lambda value: int(value, 0), required=True)
    report.add_argument('--fault-monotonic-ns', type=int, required=True)
    args = parser.parse_args()
    if args.action == 'analyze':
        print(json.dumps(analyze(args.trace.read_text(), args.errors.read_text(), args.address,
                                 args.fault_monotonic_ns), indent=2))
    else:
        metadata = layout(args.module)
        args.destination.mkdir(exist_ok=False, parents=True)
        (args.destination / 'layout.json').write_text(json.dumps(metadata, indent=2) + '\n')
        (args.destination / 'trace.bt').write_text(program(metadata['offsets']))
        print(json.dumps({'status': 'prepared_only', 'attached': False,
                          'build_id': metadata['build_id'], 'module_sha256': metadata['module_sha256']}))


if __name__ == '__main__':
    main()
