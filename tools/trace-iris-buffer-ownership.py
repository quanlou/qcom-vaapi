#!/usr/bin/env python3
"""Trace live Iris CAPTURE ownership without replacing the driver or starting video.

Run with sudo on the deployment host. Once READY appears, play one video in
Chrome. Do not reopen a failing session. This audits the DMA address submitted
to firmware and returned by firmware; it cannot prove physical writes/pixels.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import select
import signal
import struct
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]
DEFAULT_ELF = '/home/mq/oss/EL2-setup/iris-module/qcom-iris.ko'
FAULT = re.compile(r'BUG:|Internal error: Oops|Unable to handle kernel|KASAN:|Bad page|Bad swap|arm-smmu.*fault|session error received|received system error|watchdog:.*lockup')


def ownership_report(trace_text, stats_text, kernel_text, module_build_id):
    """Retain address relationships without disclosing addresses or session IDs."""
    sessions, buffers, slots = {}, {}, {}
    objects, mappings, owners = {}, {}, {}
    changes, object_changes, mismatches = [], [], []
    submits = completions = all_completions = empty_completions = 0
    reused = object_reused = alias_reused = object_samples = 0

    def label(table, value, prefix):
        if value is None:
            return 'unavailable'
        if value == 0 and prefix in {'buffer', 'dmabuf', 'mapping'}:
            return 'unmapped'
        if value not in table:
            table[value] = f'{prefix}-{len(table) + 1}'
        return table[value]

    for line in trace_text.splitlines():
        fields = re.findall(r'(sid|type|index|cached|current|address|bytes|object|mapping)=([^\s]+)', line)
        row = {key: int(value, 0) for key, value in fields if not value.startswith('(')}
        if 'sid' not in row or 'index' not in row:
            continue
        key = row['sid'], row['index']
        session = label(sessions, row['sid'], 'session')
        if ': complete:' in line and 'bytes' in row:
            all_completions += 1
            empty_completions += int(row['bytes'] == 0)
        if ': submit:' in line and row.get('type') == 2:
            submits += 1
            current = row.get('current')
            cached = row.get('cached')
            object_id, mapping_id = row.get('object'), row.get('mapping')
            if object_id and mapping_id:
                object_samples += 1
                object_label = label(objects, object_id, 'dmabuf')
                mapping_label = label(mappings, mapping_id, 'mapping')
                if key in owners and owners[key][0] != object_id:
                    object_reused += 1
                    same_address = slots.get(key) == current
                    alias_reused += int(same_address)
                    object_changes.append({'submission': submits, 'session': session,
                                           'slot': row['index'],
                                           'previous_object': label(objects, owners[key][0], 'dmabuf'),
                                           'current_object': object_label,
                                           'previous_mapping': label(mappings, owners[key][1], 'mapping'),
                                           'current_mapping': mapping_label,
                                           'same_dma_address': same_address})
                owners[key] = object_id, mapping_id
            if key in slots and slots[key] != current:
                reused += 1
                changes.append({'submission': submits, 'session': session,
                                'slot': row['index'],
                                'previous': label(buffers, slots[key], 'buffer'),
                                'current': label(buffers, current, 'buffer')})
            slots[key] = current
            label(buffers, current, 'buffer')
            if cached != current or not current:
                mismatches.append({'kind': 'submission_address_mismatch',
                                   'submission': submits, 'session': session,
                                   'slot': row['index'],
                                   'attached': label(buffers, current, 'buffer'),
                                   'submitted': label(buffers, cached, 'buffer')})
        elif ': complete:' in line and row.get('bytes', 0):
            completions += 1
            if slots.get(key) != row.get('address'):
                mismatches.append({'kind': 'completion_address_mismatch',
                                   'completion': completions, 'session': session,
                                   'slot': row['index'], 'bytes': row['bytes'],
                                   'attached': label(buffers, slots.get(key), 'buffer'),
                                   'returned': label(buffers, row.get('address'), 'buffer')})
    lost = any(int(n) for n in re.findall(r'(?:overrun|dropped events):\s*(\d+)', stats_text))
    kernel_fault = bool(FAULT.search(kernel_text))
    return {'status': 'failed' if mismatches or kernel_fault else
            'incomplete' if lost or not (reused or object_reused) or not completions else 'submitted_addresses_match',
            'module_build_id': module_build_id, 'capture_submissions': submits,
            'capture_completions': completions, 'slot_address_changes': reused,
            'all_completion_events': all_completions,
            'empty_completion_events': empty_completions,
            'capture_completion_count_meaning': 'Nonempty completions; empty markers are counted separately.',
            'report_version': 2,
            'address_label_meaning': 'DMA address labels; these are not unique buffer objects.',
            'submissions_with_object_identity': object_samples,
            'object_identity_coverage': 'complete' if submits and object_samples == submits else 'incomplete',
            'slot_object_changes': object_reused,
            'same_address_object_changes': alias_reused,
            'object_labels_observed': len(objects), 'mapping_labels_observed': len(mappings),
            'object_change_examples': object_changes if len(object_changes) <= 16 else object_changes[:8] + object_changes[-8:],
            'object_change_examples_omitted': max(0, len(object_changes) - 16),
            'identity_scope': 'Kernel object/attachment tokens during this trace; token reuse after destruction and physical DMA writes are not verified.',
            'sessions_observed': len(sessions), 'address_labels_observed': len(buffers),
            'trace_events_lost': lost, 'kernel_fault': kernel_fault,
            'mismatch_count': len(mismatches), 'mismatches': mismatches[:128],
            'mismatch_examples_omitted': max(0, len(mismatches) - 128),
            'slot_change_examples': changes if len(changes) <= 16 else changes[:8] + changes[-8:],
            'slot_change_examples_omitted': max(0, len(changes) - 16),
            'masking': 'No raw addresses, session IDs, boot ID, file descriptors, process names, or kernel log lines.',
            'scope': 'Firmware submission/response addresses only; physical writes and pixel isolation remain unverified.'}


def command(*args):
    return subprocess.check_output(args, text=True, timeout=15)


def write_control(path, text):
    # tracefs controls reject O_APPEND; never use O_TRUNC on kprobe_events,
    # which would remove probes owned by other users/tools.
    fd = os.open(path, os.O_WRONLY)
    try:
        data = text.encode()
        if os.write(fd, data) != len(data):
            raise RuntimeError('incomplete trace control write')
    finally:
        os.close(fd)


def btf_strings(path):
    blob = path.read_bytes()
    magic, version, flags, size, toff, tlen, soff, slen = struct.unpack_from('<HBBIIIII', blob)
    if magic != 0xeb9f or version != 1 or flags:
        raise RuntimeError('unsupported BTF header')
    return blob, size, toff, tlen, blob[size + soff:size + soff + slen]


def dma_offset():
    # Split module BTF name offsets include the base vmlinux string table.
    _, _, _, _, base = btf_strings(Path('/sys/kernel/btf/vmlinux'))
    blob, size, toff, length, local = btf_strings(Path('/sys/kernel/btf/videobuf2_dma_contig'))
    def name(offset):
        strings, start = (base, offset) if offset < len(base) else (local, offset - len(base))
        return strings[start:strings.index(b'\0', start)].decode()
    sizes = {1:4, 2:0, 3:12, 4:12, 5:12, 6:8, 7:0, 8:0, 9:0, 10:0,
             11:0, 12:0, 13:8, 14:4, 15:12, 16:0, 17:4, 18:0, 19:12}
    cursor, end = size + toff, size + toff + length
    while cursor < end:
        n, info, _ = struct.unpack_from('<III', blob, cursor)
        kind, count = (info >> 24) & 31, info & 65535
        cursor += 12
        if kind not in sizes:
            raise RuntimeError('unsupported BTF type')
        if kind == 4 and name(n) == 'vb2_dc_buf':
            for i in range(count):
                member, _, bit_offset = struct.unpack_from('<III', blob, cursor + i * 12)
                if name(member) == 'dma_addr':
                    if info >> 31 or bit_offset % 8:
                        raise RuntimeError('unexpected DMA address bitfield')
                    return bit_offset // 8
        extra = sizes[kind] * (count if kind in {4,5,6,13,15,19} else 1)
        cursor += extra
    raise RuntimeError('vb2_dc_buf.dma_addr missing from module BTF')


def offsets(elf):
    fields = [('iris_buffer', f) for f in ['type','index','device_addr','fd',
              'vb2.vb2_buf.planes[0].mem_priv', 'vb2.vb2_buf.planes[0].dbuf']]
    fields += [('iris_inst','session_id')]
    fields += [('iris_hfi_buffer', f) for f in ['index','base_address','data_size','flags']]
    args = ['gdb','-nx','-nh','-batch',str(elf)]
    for typ, field in fields:
        args += ['-ex', f'p/d (unsigned long)&((struct {typ} *)0)->{field}']
    values = re.findall(r'^\$\d+ = (\d+)$', command(*args), re.M)
    if len(values) != len(fields):
        raise RuntimeError('debug type offsets unavailable')
    return {typ + '.' + field: int(value) for (typ, field), value in zip(fields, values)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--module-elf', type=Path, default=Path(DEFAULT_ELF))
    parser.add_argument('--seconds', type=int, default=45)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--check', action='store_true', help='check types and identity only; install no probes')
    args = parser.parse_args()
    if not 5 <= args.seconds <= 120:
        parser.error('--seconds must be 5..120')
    if os.geteuid() != 0:
        raise SystemExit('Run with sudo; kernel tracing requires administrator access.')
    spec = importlib.util.spec_from_file_location('activation', ROOT / 'tools/activate-iris-candidate.py')
    activation = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(activation)
    loaded = activation.loaded_build_id()
    if loaded != activation.file_build_id(args.module_elf):
        raise SystemExit('STOP: module debug file differs from the running Iris module.')
    journal = command('journalctl','-b','-k','--no-pager','-o','cat')
    if FAULT.search(journal):
        raise SystemExit('STOP: kernel fault recorded on this boot; restart before tracing.')
    fields, dma = offsets(args.module_elf), dma_offset()
    print('Type offsets and running module identity verified.', flush=True)
    if args.check:
        print(json.dumps({'module_build_id':loaded,'offsets':fields,'dma_addr_offset':dma}, indent=2))
        return
    out = args.output or Path(f'/tmp/iris-buffer-ownership-{int(time.time())}')
    out.mkdir(mode=0o755, parents=False, exist_ok=False)
    trace = Path('/sys/kernel/tracing')
    if not (trace/'kprobe_events').exists():
        trace = Path('/sys/kernel/debug/tracing')
    group = f'iris_owner_{os.getpid()}'
    instance = trace/'instances'/group
    symbols = command('cat','/proc/kallsyms')
    functions = ['iris_hfi_gen2_session_queue_buffer','iris_hfi_gen2_handle_output_buffer']
    for function in functions:
        if not re.search(r'\b' + function + r'\s+\[qcom_iris\]', symbols):
            raise SystemExit('STOP: required trace function unavailable: ' + function)
    definitions = [
        f'p:{group}/submit {functions[0]} sid=+{fields["iris_inst.session_id"]}($arg1):u32 '
        f'type=+{fields["iris_buffer.type"]}($arg2):u32 index=+{fields["iris_buffer.index"]}($arg2):u32 '
        f'fd=+{fields["iris_buffer.fd"]}($arg2):s32 cached=+{fields["iris_buffer.device_addr"]}($arg2):x64 '
        f'current=+{dma}(+{fields["iris_buffer.vb2.vb2_buf.planes[0].mem_priv"]}($arg2)):x64 '
        f'object=+{fields["iris_buffer.vb2.vb2_buf.planes[0].dbuf"]}($arg2):x64 '
        f'mapping=+{fields["iris_buffer.vb2.vb2_buf.planes[0].mem_priv"]}($arg2):x64',
        f'p:{group}/complete {functions[1]} sid=+{fields["iris_inst.session_id"]}($arg1):u32 '
        f'index=+{fields["iris_hfi_buffer.index"]}($arg2):u32 address=+{fields["iris_hfi_buffer.base_address"]}($arg2):x64 '
        f'bytes=+{fields["iris_hfi_buffer.data_size"]}($arg2):u32 flags=+{fields["iris_hfi_buffer.flags"]}($arg2):x32'
    ]
    boot = Path('/proc/sys/kernel/random/boot_id').read_text().strip()
    cursor = command('journalctl','-b','-k','-n','0','--show-cursor','--no-pager').split('-- cursor: ')[1].strip()
    registered, running, faults = [], True, ''
    def stop(*_):
        nonlocal running
        running = False
    signal.signal(signal.SIGINT,stop)
    signal.signal(signal.SIGTERM,stop)
    try:
        for event, definition in zip(['submit','complete'],definitions):
            write_control(trace/'kprobe_events', definition+'\n')
            registered.append(event)
        instance.mkdir()
        (instance/'buffer_size_kb').write_text('2048')
        (instance/'events'/group/'enable').write_text('1')
        (instance/'tracing_on').write_text('1')
        fd = os.open(instance/'trace_pipe',os.O_RDONLY|os.O_NONBLOCK)
        started, next_check = time.monotonic(), time.monotonic()
        print('READY: play ONE video now. Do not reopen it if playback fails.',flush=True)
        try:
            with (out/'trace.log').open('wb') as log:
                while running and time.monotonic()-started < args.seconds:
                    if select.select([fd],[],[],.2)[0]:
                        log.write(os.read(fd,65536))
                        log.flush()
                    if time.monotonic() >= next_check:
                        faults = command('journalctl','-b','-k','--after-cursor',cursor,'--no-pager','-o','cat')
                        if FAULT.search(faults):
                            print('STOP: kernel/decoder fault; do not run another video test.',flush=True)
                            break
                        next_check = time.monotonic()+1
                (instance/'tracing_on').write_text('0')
                while select.select([fd],[],[],0)[0]:
                    data=os.read(fd,65536)
                    if not data:break
                    log.write(data)
        finally:
            os.close(fd)
        dropped = '\n'.join(p.read_text() for p in (instance/'per_cpu').glob('cpu*/stats'))
        (out/'trace-stats.txt').write_text(dropped)
    finally:
        if instance.exists():
            (instance/'tracing_on').write_text('0')
            (instance/'events'/group/'enable').write_text('0')
            instance.rmdir()
        for event in reversed(registered):
            write_control(trace/'kprobe_events', f'-:{group}/{event}\n')
        (out/'kernel.log').write_text(command('journalctl','-b','-k','--after-cursor',cursor,'--no-pager','-o','cat'))
    submits, completions, reused, mismatches = 0, 0, 0, []
    slots = {}
    for line in (out/'trace.log').read_text().splitlines():
        row = dict(re.findall(r'(sid|type|index|fd|cached|current|address|bytes|flags)=([^\s]+)',line))
        if not row:continue
        row = {k:int(v,0) for k,v in row.items() if not v.startswith('(')}
        if ': submit:' in line and row.get('type') == 2:
            submits += 1
            key=(row['sid'],row['index'])
            if key in slots and slots[key] != row.get('current'):reused+=1
            slots[key]=row.get('current')
            if row.get('cached') != row.get('current') or not row.get('current'):
                mismatches.append(line)
        elif ': complete:' in line and row.get('bytes',0):
            completions += 1
            if slots.get((row['sid'],row['index'])) != row.get('address'):
                mismatches.append(line)
    lost = any(int(n) for n in re.findall(r'(?:overrun|dropped events):\s*(\d+)',dropped))
    kernel_fault = bool(FAULT.search((out/'kernel.log').read_text()))
    result = {'status':'failed' if mismatches or kernel_fault else 'incomplete' if lost or not reused or not completions else 'submitted_addresses_match',
              'boot_id':boot,'module_build_id':loaded,'capture_submissions':submits,
              'capture_completions':completions,'slot_address_changes':reused,
              'trace_events_lost':lost,'mismatches':mismatches,'kernel_fault':kernel_fault,
              'scope':'Firmware submission/response addresses only; physical writes and pixel isolation remain unverified.'}
    (out/'result.json').write_text(json.dumps(result,indent=2)+'\n')
    shared = ownership_report((out/'trace.log').read_text(), dropped,
                              (out/'kernel.log').read_text(), loaded)
    (out/'share.json').write_text(json.dumps(shared, indent=2)+'\n')
    uid,gid=int(os.environ.get('SUDO_UID',os.getuid())),int(os.environ.get('SUDO_GID',os.getgid()))
    for path in [out,*out.iterdir()]:os.chown(path,uid,gid)
    print(json.dumps(shared,indent=2))
    print('Evidence: '+str(out),flush=True)
    print('Share only share.json; raw trace and kernel logs stay local.',flush=True)


if __name__ == '__main__':
    main()
