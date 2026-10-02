#!/usr/bin/env python3
"""Isolated clean/error PSC-LAST completion checks from actual Iris functions.

Copies and patches only the response source in /tmp. Extracts flag mapping,
output handling and VB2 completion; fake kernel queues/state helpers make the
source-level contract observable without hardware. Not an HFI trace, module
activation, concurrency test, or proof that small-stream firmware emits a clean
PSC marker.
"""
import argparse
import importlib.util
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('metadata', ROOT / 'tools/verify-iris-metadata.py')
metadata = importlib.util.module_from_spec(spec)
spec.loader.exec_module(metadata)

PRELUDE = r'''
#include <assert.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <errno.h>
#include <time.h>
#include <linux/videodev2.h>
typedef uint32_t u32;
typedef uint64_t u64;
#define BUF_INPUT 1
#define BUF_OUTPUT 2
#define BUF_ATTR_QUEUED 4
#define BUF_ATTR_DEQUEUED 8
#define IRIS_INST_STREAMING 1
#define VB2_BUF_STATE_ERROR 1
#define VB2_BUF_STATE_DONE 2
struct vb2_buffer { u64 timestamp; u32 payload; };
struct vb2_v4l2_buffer { struct vb2_buffer vb2_buf; u32 flags, sequence; };
struct v4l2_m2m_buffer { struct vb2_v4l2_buffer vb; };
struct v4l2_m2m_ctx { struct v4l2_m2m_buffer *dst; bool stopped; u32 done, done_state; };
struct iris_buffer { struct v4l2_m2m_buffer m2m; u32 type, index, attr, flags,
    device_addr, data_offset, data_size; u64 timestamp; };
struct iris_inst { struct v4l2_m2m_ctx *m2m_ctx; u32 state, fh, sequence_cap,
    sequence_out, psc, drain; bool last_buffer_dequeued; };
struct frame_info { u32 picture_type, data_corrupt, overflow; };
struct iris_inst_hfi_gen2 { struct iris_inst inst; struct frame_info hfi_frame_info; };
struct iris_hfi_buffer { u32 index, flags, base_address, data_offset, data_size; u64 timestamp; };
#define to_iris_inst_hfi_gen2(inst) ((struct iris_inst_hfi_gen2 *)(inst))
#define to_iris_buffer(vb) ((struct iris_buffer *)(vb))
#define v4l2_m2m_for_each_dst_buf_safe(ctx, item, next) \
    for ((item) = (ctx)->dst, (next) = NULL; (item); (item) = (next))
static struct v4l2_m2m_ctx *current;
static u32 eos;
static int transition_error;
static int iris_inst_sub_state_change_drain_last(struct iris_inst *inst)
{ inst->drain++; return transition_error; }
static int iris_inst_sub_state_change_drc_last(struct iris_inst *inst)
{ inst->psc++; return transition_error; }
static struct vb2_v4l2_buffer *iris_helper_find_buf(struct iris_inst *inst, u32 type, u32 index)
{ (void)type; (void)index; return &inst->m2m_ctx->dst->vb; }
static void vb2_set_plane_payload(struct vb2_buffer *vb, u32 plane, u32 size)
{ assert(plane == 0); vb->payload = size; }
static void iris_get_ts_metadata(struct iris_inst *inst, u64 timestamp,
                                struct vb2_v4l2_buffer *vb)
{ (void)inst; (void)timestamp; (void)vb; }
static bool v4l2_m2m_has_stopped(struct v4l2_m2m_ctx *ctx) { return ctx->stopped; }
static void v4l2_m2m_mark_stopped(struct v4l2_m2m_ctx *ctx) { ctx->stopped = true; }
static void v4l2_event_queue_fh(u32 *fh, const struct v4l2_event *event)
{ (void)fh; assert(event->type == V4L2_EVENT_EOS); eos++; }
static void v4l2_m2m_buf_done(struct vb2_v4l2_buffer *vb, u32 state)
{ (void)vb; current->done++; current->done_state = state; }
'''
MAIN = r'''
static void scenario(bool baseline, u32 hfi_flags, u32 size, u32 corruption,
                     u32 overflow, bool noshow, bool streaming, bool already_stopped)
{
    struct iris_buffer buf = { .type = BUF_OUTPUT, .index = 7,
        .attr = BUF_ATTR_QUEUED, .device_addr = 1234 };
    struct v4l2_m2m_ctx ctx = { .dst = &buf.m2m, .stopped = already_stopped };
    struct iris_inst_hfi_gen2 gen = { .inst = { .m2m_ctx = &ctx,
        .state = streaming ? IRIS_INST_STREAMING : 0 },
        .hfi_frame_info = { .picture_type = noshow ? HFI_GEN2_PICTURE_NOSHOW : HFI_GEN2_PICTURE_P,
                          .data_corrupt = corruption, .overflow = overflow } };
    struct iris_hfi_buffer hfi = { .index = 7, .base_address = 1234,
        .flags = hfi_flags, .data_size = size, .timestamp = 123456 };
    bool marker = hfi_flags & (HFI_BUF_FW_FLAG_LAST | HFI_BUF_FW_FLAG_PSC_LAST);
    bool synthetic = streaming && !size && !(hfi_flags & HFI_BUF_FW_FLAG_LAST) &&
                     (baseline || !(hfi_flags & HFI_BUF_FW_FLAG_PSC_LAST));
    bool error = corruption || overflow || noshow || synthetic;
    current = &ctx;
    eos = 0;
    transition_error = 0;
    assert(iris_hfi_gen2_handle_output_buffer(&gen.inst, &hfi) == 0);
    assert(((buf.flags & V4L2_BUF_FLAG_ERROR) != 0) == error);
    assert(((buf.flags & V4L2_BUF_FLAG_LAST) != 0) == marker);
    assert((buf.attr & BUF_ATTR_QUEUED) == 0);
    assert(buf.attr & BUF_ATTR_DEQUEUED);
    assert(gen.inst.psc == !!(hfi_flags & HFI_BUF_FW_FLAG_PSC_LAST));
    assert(gen.inst.drain == !!(hfi_flags & HFI_BUF_FW_FLAG_LAST));
    assert(iris_vb2_buffer_done(&gen.inst, &buf) == 0);
    assert(ctx.done == 1);
    assert(ctx.done_state == (error ? VB2_BUF_STATE_ERROR : VB2_BUF_STATE_DONE));
    assert(buf.m2m.vb.vb2_buf.payload == (error ? 0 : size));
    assert(buf.m2m.vb.vb2_buf.timestamp == (error ? 0 : hfi.timestamp));
    assert(gen.inst.last_buffer_dequeued == (marker && !error));
    assert(ctx.stopped == (already_stopped || (marker && !error)));
    assert(eos == (marker && !error && !already_stopped));
    assert(gen.inst.sequence_cap == (error ? 0 : 1));
}
int main(int argc, char **argv)
{
    assert(argc == 2);
    bool baseline = strcmp(argv[1], "baseline") == 0;
    const u32 flags[] = {0, HFI_BUF_FW_FLAG_LAST, HFI_BUF_FW_FLAG_PSC_LAST,
                        HFI_BUF_FW_FLAG_LAST | HFI_BUF_FW_FLAG_PSC_LAST};
    u32 cases = 0;
    for (u32 f = 0; f < 4; f++)
        for (u32 size = 0; size < 2; size++)
            for (u32 errors = 0; errors < 8; errors++)
                for (u32 streaming = 0; streaming < 2; streaming++)
                    for (u32 stopped = 0; stopped < 2; stopped++) {
                        scenario(baseline, flags[f], size ? 115200 : 0,
                                 errors & 1, errors & 2, errors & 4, streaming, stopped);
                        cases++;
                    }
    printf("iris_psc_%s=pass cases=%u clean_psc_synthetic_error=%s\n",
           argv[1], cases, baseline ? "reproduced" : "removed");
    return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('linux_source', type=Path)
    args = parser.parse_args()
    source = args.linux_source / 'drivers/media/platform/qcom/iris'
    response = (source / 'iris_hfi_gen2_response.c').read_text()
    completion = metadata.function((source / 'iris_buffer.c').read_text(), 'iris_vb2_buffer_done')
    definitions = (source / 'iris_hfi_gen2_defines.h').read_text()
    constants = []
    for name in ('HFI_BUF_FW_FLAG_LAST', 'HFI_BUF_FW_FLAG_PSC_LAST',
                 'HFI_GEN2_PICTURE_IDR', 'HFI_GEN2_PICTURE_I', 'HFI_GEN2_PICTURE_CRA',
                 'HFI_GEN2_PICTURE_BLA', 'HFI_GEN2_PICTURE_NOSHOW', 'HFI_GEN2_PICTURE_P',
                 'HFI_GEN2_PICTURE_B'):
        value = re.search(r'\b' + name + r'\s*=\s*(0x[0-9a-fA-F]+)', definitions).group(1)
        constants.append(f'#define {name} {value}\n')
    with tempfile.TemporaryDirectory(prefix='iris-psc-') as directory:
        work = Path(directory)
        target = work / 'drivers/media/platform/qcom/iris'
        target.mkdir(parents=True)
        shutil.copyfile(source / 'iris_hfi_gen2_response.c', target / 'iris_hfi_gen2_response.c')
        subprocess.run(['patch', '-p1', '--batch', '--fuzz=0', '-i',
                        str(ROOT / 'kernel/0002-iris-preserve-empty-psc-last-completion.patch')],
                       cwd=work, check=True)
        for label, text in [('baseline', response), ('patched', (target / 'iris_hfi_gen2_response.c').read_text())]:
            harness = work / (label + '.c')
            harness.write_text(PRELUDE + ''.join(constants) +
                               metadata.function(text, 'iris_hfi_gen2_get_driver_buffer_flags') + '\n' +
                               metadata.function(text, 'iris_hfi_gen2_handle_output_buffer') + '\n' +
                               completion + MAIN)
            executable = work / label
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror', '-g',
                            '-fsanitize=undefined', '-fno-sanitize-recover=undefined',
                            str(harness), '-o', str(executable)], check=True)
            subprocess.run([str(executable), label], check=True, timeout=10)


if __name__ == '__main__':
    main()
