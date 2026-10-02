#!/usr/bin/env python3
"""Sanitizer reproduction using actual upstream Iris metadata functions.

Supply a Linux source tree. The tree is never modified: the candidate patch is
applied to a temporary copy. This is an isolated function test, not kernel or
hardware validation.
"""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def function(source, name):
    start = source.index('\n', source.rindex('\n', 0, source.index(name + '('))) + 1
    # Include the declaration and multiline argument list through its body.
    opening = source.index('{', start)
    depth = 1
    end = opening + 1
    while depth:
        depth += (source[end] == '{') - (source[end] == '}')
        end += 1
    return source[start:end]


PRELUDE = r'''
#include <assert.h>
#include <stdint.h>
typedef uint32_t u32;
typedef uint64_t u64;
#define ARRAY_SIZE(x) (sizeof(x) / sizeof((x)[0]))
#define V4L2_BUF_FLAG_TIMECODE 0x100
#define V4L2_BUF_FLAG_TSTAMP_SRC_MASK 0x70000
#define NSEC_PER_USEC 1000
#define do_div(n, base) ((n) /= (base))
struct vb2_buffer { u64 timestamp; };
struct vb2_v4l2_buffer { struct vb2_buffer vb2_buf; u32 flags; u32 timecode; };
struct iris_ts_metadata { u64 ts_ns; u64 ts_us; u32 flags; u32 tc; };
struct iris_inst { struct iris_ts_metadata tss[32]; u32 metadata_idx; };
'''
MAIN = r'''
int main(void)
{
    struct iris_inst inst = {0};
    struct vb2_v4l2_buffer input = {0}, output = {0};
    const u32 mask = V4L2_BUF_FLAG_TIMECODE | V4L2_BUF_FLAG_TSTAMP_SRC_MASK;
    const u32 unrelated = 0x80000000;
    for (u32 n = 0; n < 4096; n++) {
        input.vb2_buf.timestamp = (u64)(n + 1) * 1000;
        input.flags = unrelated | ((n & 1) ? mask : 0);
        input.timecode = n;
        iris_set_ts_metadata(&inst, &input);
        output.flags = unrelated | mask;
        iris_get_ts_metadata(&inst, input.vb2_buf.timestamp, &output);
        assert(output.timecode == n);
        assert(output.flags == (unrelated | (input.flags & mask)));
        /* Missing timestamps reproduce the real capture fallback at wrap. */
        iris_get_ts_metadata(&inst, UINT64_MAX, &output);
        assert(inst.metadata_idx < ARRAY_SIZE(inst.tss));
        assert(output.timecode == inst.tss[inst.metadata_idx].tc);
        assert(output.flags == (unrelated | inst.tss[inst.metadata_idx].flags));
        /* Capture may complete out of submission order. Check every retained
         * timestamp in reverse order, including both sides of a ring wrap. */
        u32 retained = n + 1 < ARRAY_SIZE(inst.tss) ? n + 1 : ARRAY_SIZE(inst.tss);
        for (u32 age = 0; age < retained; age++) {
            u32 expected = n - age;
            iris_get_ts_metadata(&inst, (u64)(expected + 1) * 1000, &output);
            assert(output.timecode == expected);
            assert(output.flags == (unrelated | ((expected & 1) ? mask : 0)));
        }
    }
    /* Preserve the existing defensive writer check for stale indices. */
    const u32 invalid_indices[] = {32, 33, UINT32_MAX};
    for (u32 n = 0; n < ARRAY_SIZE(invalid_indices); n++) {
        inst.metadata_idx = invalid_indices[n];
        input.vb2_buf.timestamp = UINT64_MAX - n - 1;
        input.timecode = 10000 + n;
        iris_set_ts_metadata(&inst, &input);
        assert(inst.metadata_idx == 1);
        iris_get_ts_metadata(&inst, input.vb2_buf.timestamp, &output);
        assert(output.timecode == input.timecode);
    }
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('linux_source', type=Path)
    args = parser.parse_args()
    source = args.linux_source / 'drivers/media/platform/qcom/iris'
    common = (source / 'iris_common.c').read_text()
    getter = function((source / 'iris_buffer.c').read_text(), 'iris_get_ts_metadata')
    with tempfile.TemporaryDirectory(prefix='iris-metadata-') as directory:
        work = Path(directory)
        target = work / 'drivers/media/platform/qcom/iris'
        target.mkdir(parents=True)
        shutil.copyfile(source / 'iris_common.c', target / 'iris_common.c')
        subprocess.run(['patch', '-p1', '--batch', '--fuzz=0', '-i',
                        str(ROOT / 'kernel/0001-iris-wrap-timestamp-metadata-index.patch')],
                       cwd=work, check=True)
        for label, text in [('baseline', common), ('patched', (target / 'iris_common.c').read_text())]:
            harness = work / (label + '.c')
            harness.write_text(PRELUDE + function(text, 'iris_set_ts_metadata') + '\n' + getter + MAIN)
            executable = work / label
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror', '-g',
                            '-fsanitize=undefined', '-fno-sanitize-recover=undefined',
                            str(harness), '-o', str(executable)], check=True)
            result = subprocess.run([str(executable)], capture_output=True, text=True, timeout=10)
            if label == 'baseline':
                if result.returncode == 0 or 'index 32 out of bounds' not in result.stderr:
                    raise SystemExit('iris_metadata=fail reason=baseline_did_not_reproduce_expected_fault\n' + result.stderr)
                print('iris_metadata_baseline=reproduced index=32 capacity=32')
            elif result.returncode != 0:
                raise SystemExit('iris_metadata=fail reason=patched_regression\n' + result.stderr)
            else:
                print('iris_metadata_patch=pass inputs=4096 matched_and_missing_timestamps=checked reordered_history=checked flags=checked stale_indices=checked')


if __name__ == '__main__':
    main()
