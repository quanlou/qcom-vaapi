#!/usr/bin/env python3
"""Test actual Iris power-transition functions against an isolated IRQ model.

Requires a baseline tree and a tree containing patch0005. Does not touch
devices, load modules, or claim real firmware/power-management qualification.
"""
import argparse
import importlib.util
from pathlib import Path
import subprocess
import tempfile

SPEC = importlib.util.spec_from_file_location(
    'metadata', Path(__file__).with_name('verify-iris-metadata.py'))
METADATA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(METADATA)

MODEL = r'''
#include <assert.h>
#include <errno.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define __maybe_unused
#define IRIS_CORE_DEINIT 0
#define IRIS_CORE_INIT 1
#define IRIS_CORE_ERROR 2
struct iris_core;
struct device { int refs; };
struct vpu_ops {
 void (*power_off_hw)(struct iris_core *);
 int (*power_off_controller)(struct iris_core *);
};
struct platform_data { struct vpu_ops *vpu_ops; };
struct iris_hfi_sys_ops { int (*sys_interframe_powercollapse)(struct iris_core *); };
struct firmware_data { void (*init_hfi_ops)(struct iris_core *); };
struct iris_core {
 struct device *dev;
 pthread_mutex_t lock;
 int state, irq, intr_status, sys_error_handler, v4l2_dev;
 void *vdev_dec, *vdev_enc;
 struct platform_data *iris_platform_data;
 struct iris_hfi_sys_ops *hfi_sys_ops;
 struct firmware_data *iris_firmware_data;
};
struct platform_device {struct iris_core *core;};
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t condition = PTHREAD_COND_INITIALIZER;
static struct iris_core *current;
static bool irq_active, permit_irq, powered=true, queues=true;
static int depth, pc_status, hw_status, fail_stage, pm_status;
static int power_offs, syncs, get_count, put_count;
static void mutex_lock(pthread_mutex_t *m) { assert(!pthread_mutex_lock(m)); }
static void mutex_unlock(pthread_mutex_t *m) { assert(!pthread_mutex_unlock(m)); }
static void enable_irq(int irq) {
 assert(irq == 232); mutex_lock(&gate); assert(depth > 0); depth--;
 mutex_unlock(&gate);
}
static bool disable_hardirq(int irq) {
 assert(irq == 232); mutex_lock(&gate); depth++;
 bool idle=!irq_active; mutex_unlock(&gate); return idle;
}
static void disable_irq(int irq) {
 assert(irq == 232); mutex_lock(&gate); depth++; syncs++;
 permit_irq=true; assert(!pthread_cond_broadcast(&condition));
 while (irq_active) assert(!pthread_cond_wait(&condition,&gate));
 mutex_unlock(&gate);
}
static void *irq_thread(void *unused) {
 (void)unused; mutex_lock(&gate);
 while (!permit_irq) assert(!pthread_cond_wait(&condition,&gate));
 mutex_unlock(&gate); mutex_lock(&current->lock);
 assert(powered && queues); mutex_unlock(&current->lock);
 /* Balance the primary handler's disable_irq_nosync(). */
 enable_irq(232); mutex_lock(&gate); irq_active=false;
 assert(!pthread_cond_broadcast(&condition)); mutex_unlock(&gate); return NULL;
}
static void off_hw(struct iris_core *core) {
 (void)core; assert(depth > 0);
#ifndef BASELINE
 assert(!irq_active);
#endif
 powered=false; power_offs++;
}
static int off_controller(struct iris_core *core) { (void)core; return 0; }
static int iris_opp_set_rate(struct device *dev, int rate) { (void)dev; (void)rate; return 0; }
static void iris_unset_icc_bw(struct iris_core *core) { (void)core; }
static bool iris_vpu_watchdog(struct iris_core *core, int status) { (void)core; return status != 0; }
static struct iris_core *dev_get_drvdata(struct device *dev) { (void)dev; return current; }
static void pm_runtime_mark_last_busy(struct device *dev) { (void)dev; }
static int iris_vpu_prepare_pc(struct iris_core *core) { (void)core; return pc_status; }
static int iris_set_hw_state(struct iris_core *core, bool on) { (void)core; (void)on; return hw_status; }
static void dev_err(struct device *dev,const char *msg) { (void)dev; (void)msg; }
static int pm_runtime_resume_and_get(struct device *dev) {
 get_count++; if (pm_status >= 0) dev->refs++; return pm_status;
}
static struct iris_core *platform_get_drvdata(struct platform_device *p) {return p->core;}
static void cancel_delayed_work_sync(int *work) {(void)work; assert(!irq_active);}
static void video_unregister_device(void *dev) {(void)dev; assert(!irq_active);}
static void v4l2_device_unregister(int *dev) {(void)dev; assert(!irq_active);}
static void mutex_destroy(pthread_mutex_t *m) {assert(!pthread_mutex_destroy(m));}
static void pm_runtime_put_sync(struct device *dev) { assert(dev->refs > 0); dev->refs--; put_count++; }
static void iris_fw_unload(struct iris_core *core) { (void)core; }
static void iris_hfi_queues_deinit(struct iris_core *core) { (void)core; assert(!irq_active); queues=false; }
static int iris_hfi_queues_init(struct iris_core *core) { (void)core; queues=true; return fail_stage==1 ? -EIO : 0; }
static int iris_vpu_power_on(struct iris_core *core) {
 if (fail_stage==2) return -EIO;
 powered=true; core->intr_status=0; enable_irq(core->irq); return 0;
}
static int iris_fw_load(struct iris_core *core) { (void)core; return fail_stage==3 ? -EIO : 0; }
static int iris_vpu_boot_firmware(struct iris_core *core) { (void)core; return fail_stage==4 ? -EIO : 0; }
static int iris_vpu_switch_to_hwmode(struct iris_core *core) { (void)core; return fail_stage==5 ? -EIO : 0; }
static void init_ops(struct iris_core *core) { (void)core; }
static int iris_hfi_core_init(struct iris_core *core) { (void)core; return fail_stage==6 ? -EIO : 0; }
static int ifpc(struct iris_core *core) { (void)core; return fail_stage==6 ? -EIO : 0; }
static int iris_wait_for_system_response(struct iris_core *core) {
 (void)core; assert(depth==0); assert(powered && queues); return 0;
}
'''

MAIN = r'''
int main(int argc, char **argv) {
 assert(argc==2);
 struct device dev={0};
 struct vpu_ops vpu={off_hw,off_controller};
 struct platform_data platform={&vpu};
 struct iris_hfi_sys_ops hfi={ifpc}; struct firmware_data firmware={init_ops};
 struct iris_core core={.dev=&dev,.lock=PTHREAD_MUTEX_INITIALIZER,
  .state=IRIS_CORE_INIT,.irq=232,.iris_platform_data=&platform,
  .hfi_sys_ops=&hfi,.iris_firmware_data=&firmware};
 current=&core; const char *scenario=argv[1]; pthread_t thread;
 bool pending=strstr(scenario,"pending")!=NULL;
 if (pending) {
  irq_active=true; depth=1;
  assert(!pthread_create(&thread,NULL,irq_thread,NULL));
 }
 int ret;
 if (!strcmp(scenario,"cycles")) {
  for (int i=0;i<10000;i++) {assert(iris_pm_suspend(&dev)==0); assert(depth==1 && !powered); assert(iris_pm_resume(&dev)==0); assert(depth==0 && powered);}
  ret=0;
 } else if (!strncmp(scenario,"remove",6)) {
  if (strstr(scenario,"cold")) {core.state=IRIS_CORE_DEINIT; powered=false; queues=false; depth=1;}
  if (strstr(scenario,"pm-fail")) {pm_status=-EIO; powered=false; depth=1;}
  struct platform_device platform_device={&core};
  iris_remove(&platform_device); ret=0; assert(!queues && depth==2 && dev.refs==0);
  assert(get_count==2 && put_count==(pm_status < 0 ? 0 : 2));
 } else if (!strcmp(scenario,"init-pending")) {
  ret=iris_core_init(&core); assert(ret==0);
 } else if (!strncmp(scenario,"init-",5)) {
  core.state=IRIS_CORE_DEINIT; powered=false; queues=false; depth=1;
  fail_stage=atoi(scenario+5); ret=iris_core_init(&core);
  assert(ret==(fail_stage ? -EIO : 0));
  assert(depth==(fail_stage ? 1 : 0));
 } else if (!strncmp(scenario,"deinit",6)) {
  if (strstr(scenario,"pm-fail")) {pm_status=-EIO; powered=false; depth=1;}
  if (strstr(scenario,"cold")) {core.state=IRIS_CORE_DEINIT; powered=false; queues=false; depth=1;}
  iris_core_deinit(&core); ret=0;
  assert(core.state==IRIS_CORE_DEINIT && !queues);
  assert(dev.refs==0 && get_count==1 && put_count==(pm_status < 0 ? 0 : 1));
  assert(depth==1);
 } else if (!strncmp(scenario,"resume",6)) {
  if (!pending) {depth=1; powered=false;}
  if (strstr(scenario,"boot-fail")) fail_stage=4;
  if (strstr(scenario,"power-fail")) fail_stage=2;
  if (strstr(scenario,"hw-fail")) hw_status=-EIO;
  ret=iris_pm_resume(&dev);
  assert(ret==(pending || fail_stage || hw_status ? -EBUSY : 0));
  if (!pending) assert(depth==(ret ? 1 : 0));
 } else {
  if (strstr(scenario,"pc-fail")) pc_status=-EIO;
  if (strstr(scenario,"hw-fail")) hw_status=-EIO;
  if (strstr(scenario,"inactive")) core.state=IRIS_CORE_DEINIT;
  if (strstr(scenario,"watchdog")) {core.intr_status=1; depth=1;}
  ret=iris_pm_suspend(&dev);
  assert(ret==(pending ? -EBUSY : pc_status ? -EAGAIN : hw_status));
  if (!pending) assert(depth==(ret || core.state!=IRIS_CORE_INIT ? 0 : 1));
 }
 if (pending) {
  if (!strncmp(scenario,"suspend",7) || !strncmp(scenario,"resume",6)) {
   assert(powered && queues && power_offs==0 && depth==1);
   mutex_lock(&gate); permit_irq=true; assert(!pthread_cond_broadcast(&condition)); mutex_unlock(&gate);
  }
  assert(!pthread_join(thread,NULL));
  assert(depth==(!strncmp(scenario,"remove",6) ? 2 : !strncmp(scenario,"deinit",6) ? 1 : 0));
 }
 printf("iris_pm_%s=pass ret=%d depth=%d power_offs=%d irq_syncs=%d\n",scenario,ret,depth,power_offs,syncs);
 return 0;
}
'''


def source_functions(tree):
    root = tree / 'drivers/media/platform/qcom/iris'
    chunks = []
    for file, names in (
        ('iris_vpu_common.c', ('iris_vpu_power_off',)),
        ('iris_hfi_common.c', ('iris_hfi_pm_suspend', 'iris_hfi_pm_resume')),
        ('iris_core.c', ('iris_core_deinit', 'iris_core_init')),
        ('iris_probe.c', ('iris_pm_suspend', 'iris_pm_resume', 'iris_remove')),
    ):
        source = (root / file).read_text()
        chunks.extend(METADATA.function(source, name) for name in names)
    return '\n'.join(chunks)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline', type=Path)
    parser.add_argument('candidate', type=Path)
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix='iris-pm-model-') as tmp:
        work = Path(tmp)
        for label, tree in (('baseline', args.baseline), ('candidate', args.candidate)):
            source = work / (label + '.c')
            source.write_text(('#define BASELINE\n' if label == 'baseline' else '') + MODEL + source_functions(tree) + MAIN)
            executable = work / label
            subprocess.run(['cc', '-std=c11', '-pthread', '-Wall', '-Wextra',
                            '-Werror', '-Wno-unused-function', '-fsanitize=undefined',
                            '-fno-sanitize-recover=undefined', str(source), '-o',
                            str(executable)], check=True)
            if label == 'baseline':
                try:
                    subprocess.run([str(executable), 'suspend-pending'], check=True, timeout=1)
                except subprocess.TimeoutExpired:
                    print('iris_pm_baseline=deadlock_reproduced', flush=True)
                else:
                    raise SystemExit('baseline did not reproduce expected deadlock')
                continue
            scenarios = ('suspend-pending', 'resume-pending', 'suspend',
                         'suspend-pc-fail', 'suspend-hw-fail', 'suspend-inactive',
                         'suspend-watchdog', 'resume', 'resume-boot-fail',
                         'resume-power-fail', 'resume-hw-fail', 'init-pending', 'deinit-pending',
                         'deinit', 'deinit-cold', 'deinit-pm-fail', 'remove-pending',
                         'remove', 'remove-cold', 'remove-pm-fail', 'cycles')
            for scenario in (*scenarios, *(f'init-{stage}' for stage in range(7))):
                subprocess.run([str(executable), scenario], check=True, timeout=5)


if __name__ == '__main__':
    main()
