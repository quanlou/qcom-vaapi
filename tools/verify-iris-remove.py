#!/usr/bin/env python3
"""Reproduce removal's IRQ/mutex deadlock using actual functions and pthreads.

Tests synchronization order and PM-reference balancing in an isolated model,
not hardware power management, firmware, or device unbind with open handles.
"""
import argparse
import importlib.util
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('metadata', ROOT / 'tools/verify-iris-metadata.py')
METADATA = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(METADATA)

MODEL = r'''
#include <assert.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define IRIS_CORE_DEINIT 0
#define IRIS_CORE_INIT 1
struct device { int refs; };
struct iris_core {
    pthread_mutex_t lock;
    struct device *dev;
    int state, irq, sys_error_handler, v4l2_dev;
    void *vdev_dec, *vdev_enc;
};
struct platform_device { struct iris_core *core; };
static pthread_mutex_t gate = PTHREAD_MUTEX_INITIALIZER;
static pthread_cond_t condition = PTHREAD_COND_INITIALIZER;
static struct iris_core *current;
static bool permit_irq, irq_finished, pending_irq, queues_freed, powered = true;
static int pm_status, gets, puts, syncs, cancelled;
static struct iris_core *platform_get_drvdata(struct platform_device *dev) { return dev->core; }
static void mutex_lock(pthread_mutex_t *lock) { assert(pthread_mutex_lock(lock) == 0); }
static void mutex_unlock(pthread_mutex_t *lock) { assert(pthread_mutex_unlock(lock) == 0); }
static void mutex_destroy(pthread_mutex_t *lock) { assert(pthread_mutex_destroy(lock) == 0); }
static int pm_runtime_resume_and_get(struct device *dev)
{ gets++; if (pm_status >= 0) dev->refs++; return pm_status; }
static void pm_runtime_put_sync(struct device *dev)
{ assert(dev->refs > 0); dev->refs--; puts++; }
static void *irq_thread(void *unused)
{
    (void)unused;
    mutex_lock(&gate);
    while (!permit_irq) assert(pthread_cond_wait(&condition, &gate) == 0);
    mutex_unlock(&gate);
    mutex_lock(&current->lock);
    assert(powered && !queues_freed);
    mutex_unlock(&current->lock);
    mutex_lock(&gate);
    irq_finished = true;
    assert(pthread_cond_broadcast(&condition) == 0);
    mutex_unlock(&gate);
    return NULL;
}
static void disable_irq(int irq)
{
    assert(irq == 232);
    syncs++;
    mutex_lock(&gate);
    permit_irq = true;
    assert(pthread_cond_broadcast(&condition) == 0);
    while (!irq_finished) assert(pthread_cond_wait(&condition, &gate) == 0);
    mutex_unlock(&gate);
}
static void cancel_delayed_work_sync(int *work)
{ (void)work; assert(irq_finished); cancelled++; }
static void iris_fw_unload(struct iris_core *core) { (void)core; }
static void iris_vpu_power_off(struct iris_core *core)
{ disable_irq(core->irq); powered = false; }
static void iris_hfi_queues_deinit(struct iris_core *core)
{ (void)core; assert(irq_finished); queues_freed = true; }
static void video_unregister_device(void *dev) { (void)dev; assert(irq_finished); }
static void v4l2_device_unregister(int *dev) { (void)dev; assert(irq_finished); }
'''
MAIN = r'''
int main(int argc, char **argv)
{
    assert(argc == 2);
    struct device dev = {0};
    struct iris_core core = { .lock = PTHREAD_MUTEX_INITIALIZER, .dev = &dev,
        .state = IRIS_CORE_INIT, .irq = 232 };
    struct platform_device platform = { .core = &core };
    pthread_t thread;
    current = &core;
    if (!strcmp(argv[1], "null")) {
        platform.core = NULL;
        iris_remove(&platform);
        assert(gets == 0 && puts == 0 && syncs == 0);
        puts("iris_remove_null=pass");
        return 0;
    }
    pending_irq = strcmp(argv[1], "cold") != 0;
    if (!pending_irq) { irq_finished = true; core.state = IRIS_CORE_DEINIT; }
    if (!strcmp(argv[1], "pm-failure")) pm_status = -5;
    if (pending_irq) assert(pthread_create(&thread, NULL, irq_thread, NULL) == 0);
    iris_remove(&platform);
    if (pending_irq) assert(pthread_join(thread, NULL) == 0);
    assert(irq_finished && cancelled == 1 && dev.refs == 0);
    assert(gets == 2 && puts == (pm_status < 0 ? 0 : 2));
    assert(core.state == IRIS_CORE_DEINIT);
    printf("iris_remove_%s=pass gets=%d puts=%d irq_syncs=%d\n", argv[1], gets, puts, syncs);
    return 0;
}
'''
# Avoid shadowing the libc puts() function in the source-level model.
MODEL = MODEL.replace('puts', 'pm_puts')
MAIN = MAIN.replace('puts ==', 'pm_puts ==').replace('gets, puts,', 'gets, pm_puts,')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('linux_source', type=Path)
    args = parser.parse_args()
    source = args.linux_source / 'drivers/media/platform/qcom/iris'
    original = (source / 'iris_probe.c').read_text()
    deinit = METADATA.function((source / 'iris_core.c').read_text(), 'iris_core_deinit')
    with tempfile.TemporaryDirectory(prefix='iris-remove-') as tmp:
        work = Path(tmp)
        target = work / 'drivers/media/platform/qcom/iris'
        target.mkdir(parents=True)
        shutil.copy2(source / 'iris_probe.c', target / 'iris_probe.c')
        subprocess.run(['patch', '--batch', '--fuzz=0', '-p1', '-d', str(work), '-i',
                        str(ROOT / 'kernel/0003-iris-quiesce-irq-before-remove.patch')], check=True)
        for label, text in [('baseline', original), ('patched', (target / 'iris_probe.c').read_text())]:
            code = work / (label + '.c')
            code.write_text(MODEL + deinit + METADATA.function(text, 'iris_remove') + MAIN)
            executable = work / label
            subprocess.run(['cc', '-std=c11', '-pthread', '-Wall', '-Wextra', '-Werror',
                            '-Wno-unused-function', '-g', '-fsanitize=undefined',
                            '-fno-sanitize-recover=undefined', str(code), '-o', str(executable)], check=True)
            if label == 'baseline':
                try:
                    subprocess.run([str(executable), 'active'], check=True, timeout=1)
                except subprocess.TimeoutExpired:
                    print('iris_remove_baseline=deadlock_reproduced queued_irq_waits_for_core_lock', flush=True)
                else:
                    raise SystemExit('iris_remove_baseline=fail expected_deadlock_not_reproduced')
            else:
                for scenario in ('active', 'cold', 'pm-failure', 'null'):
                    subprocess.run([str(executable), scenario], check=True, timeout=5)


if __name__ == '__main__':
    main()
