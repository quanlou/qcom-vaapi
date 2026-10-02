#!/usr/bin/env python3
"""Audit original Iris NOSHOW completion through actual source functions.

This does not patch the kernel or prove that firmware supplies valid hidden-frame
pixels. It identifies whether userspace receives the payload/timestamp needed to
associate a hidden AV1 reference with its submitted surface.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('psc', ROOT / 'tools/verify-iris-psc-last.py')
psc = importlib.util.module_from_spec(spec)
spec.loader.exec_module(psc)

MAIN = r'''
int main(void)
{
    unsigned cases = 0;
    for (unsigned noshow = 0; noshow < 2; noshow++)
        for (unsigned corrupt = 0; corrupt < 2; corrupt++)
            for (unsigned overflow = 0; overflow < 2; overflow++) {
                struct iris_buffer buf = { .type = BUF_OUTPUT, .index = 7,
                    .attr = BUF_ATTR_QUEUED, .device_addr = 1234 };
                struct v4l2_m2m_ctx ctx = { .dst = &buf.m2m };
                struct iris_inst_hfi_gen2 gen = { .inst = { .m2m_ctx = &ctx,
                    .state = IRIS_INST_STREAMING }, .hfi_frame_info = {
                    .picture_type = noshow ? HFI_GEN2_PICTURE_NOSHOW : HFI_GEN2_PICTURE_P,
                    .data_corrupt = corrupt, .overflow = overflow } };
                struct iris_hfi_buffer hfi = { .index = 7, .base_address = 1234,
                    .data_size = 115200, .timestamp = 123456 };
                current = &ctx;
                eos = 0;
                transition_error = 0;
                assert(iris_hfi_gen2_handle_output_buffer(&gen.inst, &hfi) == 0);
                assert(iris_vb2_buffer_done(&gen.inst, &buf) == 0);
                bool error = noshow || corrupt || overflow;
                assert(ctx.done == 1);
                assert(ctx.done_state == (error ? VB2_BUF_STATE_ERROR : VB2_BUF_STATE_DONE));
                assert(buf.m2m.vb.vb2_buf.payload == (error ? 0 : hfi.data_size));
                assert(buf.m2m.vb.vb2_buf.timestamp == (error ? 0 : hfi.timestamp));
                assert(gen.inst.sequence_cap == (error ? 0 : 1));
                assert(!ctx.stopped && !eos);
                cases++;
            }
    printf("iris_noshow_audit=pass cases=%u clean_noshow_payload=0 clean_noshow_timestamp=0\n", cases);
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('linux_source', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    source = args.linux_source / 'drivers/media/platform/qcom/iris'
    response = (source / 'iris_hfi_gen2_response.c').read_text()
    completion = (source / 'iris_buffer.c').read_text()
    definitions = (source / 'iris_hfi_gen2_defines.h').read_text()
    constants = []
    for name in ('HFI_BUF_FW_FLAG_LAST', 'HFI_BUF_FW_FLAG_PSC_LAST',
                 'HFI_GEN2_PICTURE_IDR', 'HFI_GEN2_PICTURE_I', 'HFI_GEN2_PICTURE_CRA',
                 'HFI_GEN2_PICTURE_BLA', 'HFI_GEN2_PICTURE_NOSHOW', 'HFI_GEN2_PICTURE_P',
                 'HFI_GEN2_PICTURE_B'):
        match = re.search(r'\b' + name + r'\s*=\s*(0x[0-9a-fA-F]+)', definitions)
        if not match:
            raise ValueError('missing_source_constant:' + name)
        constants.append(f'#define {name} {match.group(1)}\n')
    harness = args.output / 'actual-functions.c'
    harness.write_text(psc.PRELUDE + ''.join(constants) +
                       psc.metadata.function(response, 'iris_hfi_gen2_get_driver_buffer_flags') + '\n' +
                       psc.metadata.function(response, 'iris_hfi_gen2_handle_output_buffer') + '\n' +
                       psc.metadata.function(completion, 'iris_vb2_buffer_done') + MAIN)
    with (args.output / 'model.log').open('x') as log:
        with tempfile.TemporaryDirectory(prefix='iris-noshow-audit-') as tmp:
            executable = Path(tmp) / 'audit'
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                            '-fsanitize=undefined', '-fno-sanitize-recover=undefined',
                            str(harness), '-o', str(executable)], check=True, stdout=log, stderr=log)
            subprocess.run([str(executable)], check=True, timeout=10, stdout=log, stderr=log)
    result = {
        'status': 'pass', 'scope': 'host completion audit only; no fix or firmware pixel validity proof',
        'cases': 8, 'clean_noshow': {'payload': 0, 'timestamp': 0, 'capture_sequence_advanced': False},
        'corruption_overflow': 'remain error with zero payload and timestamp',
        'source_sha256': {name: hashlib.sha256((source / name).read_bytes()).hexdigest()
                          for name in ('iris_hfi_gen2_response.c', 'iris_buffer.c', 'iris_hfi_gen2_defines.h')},
    }
    (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
