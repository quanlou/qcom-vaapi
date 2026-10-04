#!/usr/bin/env python3
"""Exercise actual Iris lookup and close functions under AddressSanitizer.

The model forces an IRQ lookup before close and a buffer callback during the
firmware close exchange. It checks host object lifetime, not firmware DMA,
real interrupt timing, or runtime power management.
"""
import argparse
import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile

SPEC = importlib.util.spec_from_file_location('metadata', Path(__file__).with_name('verify-iris-metadata.py'))
metadata = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata)

PRELUDE = r'''
#include <assert.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef uint32_t u32;
struct list_head { struct iris_inst *owner; };
struct kref { unsigned int refs; };
struct iris_core { int lock; struct list_head instances; };
struct context { volatile int alive; int streaming[2]; };
struct iris_inst {
 struct list_head list; struct kref kref; struct iris_core *core;
 u32 session_id; int lock, ctx_q_lock, ctrl_handler;
 bool closed; struct context *m2m_ctx; void *m2m_dev, *fmt_src, *fmt_dst;
};
struct file { struct iris_inst *inst; };
static int exchange_callbacks, destroyed, mode;
#define IRIS_INST_DEINIT 0
#define V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE 0
#define V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE 1
#define container_of(p,t,m) ((t *)((char *)(p)-offsetof(t,m)))
#define list_for_each_entry(p,h,m) for ((p)=(h)->owner; (p); (p)=NULL)
static void mutex_lock(int *p) { assert(!*p); *p=1; }
static void mutex_unlock(int *p) { assert(*p); *p=0; }
static void mutex_destroy(int *p) { assert(!*p); }
static void kref_get(struct kref *p) { assert(p->refs); p->refs++; }
static void kref_put(struct kref *p, void (*release)(struct kref *))
{ assert(p->refs); if (!--p->refs) release(p); }
static void kfree(void *p) { free(p); }
static struct iris_inst *iris_get_inst(struct file *f) { return f->inst; }
static void v4l2_ctrl_handler_free(int *p) { (void)p; }
static void *v4l2_m2m_get_dst_vq(struct context *c) { return &c->streaming[1]; }
static void *v4l2_m2m_get_src_vq(struct context *c) { return &c->streaming[0]; }
static int vb2_streamoff(void *p, int type) { (void)type; *(int *)p=0; return 0; }
static void v4l2_m2m_ctx_release(struct context *p) { free(p); }
static void v4l2_m2m_release(void *p) { (void)p; }
static void iris_session_close(struct iris_inst *p)
{
 /* A queued BUFFER response can arrive before SESSION_CLOSE completes. */
 if (mode==1) { assert(p->m2m_ctx->alive==1); exchange_callbacks++; }
}
static void iris_inst_change_state(struct iris_inst *p, int s) { (void)p; (void)s; }
static void iris_v4l2_fh_deinit(struct iris_inst *p, struct file *f) { (void)p; (void)f; }
static void iris_destroy_all_internal_buffers(struct iris_inst *p, int t) { (void)p; (void)t; }
static void iris_check_num_queued_internal_buffers(struct iris_inst *p, int t) { (void)p; (void)t; }
static void iris_remove_session(struct iris_inst *p) { p->core->instances.owner=NULL; }
'''
MAIN = r'''
int main(int argc, char **argv)
{
 assert(argc==2); mode=atoi(argv[1]);
 for (int n=0;n<128;n++) {
  struct iris_core core={0};
  struct iris_inst *inst=calloc(1,sizeof(*inst));
  inst->core=&core; inst->session_id=42; inst->kref.refs=1;
  inst->m2m_ctx=calloc(1,sizeof(*inst->m2m_ctx)); inst->m2m_ctx->alive=1;
  inst->m2m_ctx->streaming[0]=inst->m2m_ctx->streaming[1]=1;
  core.instances.owner=inst; struct file file={inst};
  struct iris_inst *irq=iris_get_instance(&core,42);
  assert(irq==inst);
  iris_close(&file);
  /* The IRQ had obtained its pointer but had not yet taken inst->lock. */
  assert(irq->session_id==42);
#ifdef PATCHED
  assert(irq->closed); assert(irq->kref.refs==1);
  assert(!iris_get_instance(&core,42));
  iris_inst_put(irq); destroyed++;
#else
  free(irq); /* Unreachable: the preceding read must reproduce the UAF. */
#endif
 }
 assert(destroyed==128); assert(mode!=1 || exchange_callbacks==128);
 puts("iris_session_lifetime=pass forced_lookup_close=128 queued_close_callbacks=checked");
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline', type=Path)
    parser.add_argument('candidate', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='iris-session-lifetime-') as folder:
        for label, tree in [('baseline', args.baseline), ('candidate', args.candidate)]:
            vidc = (tree / 'iris_vidc.c').read_text()
            bodies = []
            if label == 'candidate':
                bodies += [metadata.function(vidc, name) for name in ['iris_inst_release', 'iris_inst_put']]
            bodies += [metadata.function((tree / 'iris_utils.c').read_text(), 'iris_get_instance'),
                       metadata.function(vidc, 'iris_close')]
            source, executable = Path(folder) / (label + '.c'), Path(folder) / label
            source.write_text(PRELUDE + '\n'.join(bodies) + MAIN)
            subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Wno-unused-function',
                            '-Werror', '-g', '-fsanitize=address,undefined',
                            *(['-DPATCHED'] if label == 'candidate' else []),
                            str(source), '-o', str(executable)], check=True)
            for mode in ['0', '1']:
                result = subprocess.run([str(executable), mode], capture_output=True, text=True,
                                        env=dict(os.environ, ASAN_OPTIONS='detect_leaks=0'), timeout=10)
                if label == 'baseline':
                    if result.returncode == 0 or 'heap-use-after-free' not in result.stderr:
                        raise SystemExit('baseline did not reproduce expected UAF\n' + result.stderr)
                    print('iris_session_baseline=reproduced mode=' + mode)
                elif result.returncode:
                    raise SystemExit('candidate failed\n' + result.stderr)
                else:
                    print(result.stdout.strip() + ' mode=' + mode)


if __name__ == '__main__':
    main()
