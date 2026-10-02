#!/usr/bin/env python3
"""Host model of the exact kernel suspend_enter platform-test boundary.

Extracts the actual function; models callback results only. This is not hardware
qualification, concurrency proof or a guarantee that device callbacks return.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess

SPEC = importlib.util.spec_from_file_location('metadata', Path(__file__).with_name('verify-iris-metadata.py'))
metadata = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata)
PRELUDE = r'''
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stdio.h>
typedef int suspend_state_t;
enum { TEST_PLATFORM = 3, TEST_CPUS = 2, TEST_CORE = 1,
       PM_SUSPEND_TO_IDLE = 1, SYSTEM_SUSPEND = 2, SYSTEM_RUNNING = 3 };
#define PMSG_SUSPEND 1
#define PMSG_RESUME 2
#define BUG_ON(x) assert(!(x))
#define TPS(x) (x)
#define pr_err(...) ((void)0)
static int fail_step, step, tests, cpu_off, cpu_on, machine_enter, idle_enter;
static int noirq_resume, early_resume, finish, sys_suspended, system_state;
static bool irq_disabled;
static int callback(void) { return ++step == fail_step ? -5 : 0; }
static int platform_suspend_prepare(int state) { (void)state; return callback(); }
static int dpm_suspend_late(int msg) { (void)msg; return callback(); }
static int platform_suspend_prepare_late(int state) { (void)state; return callback(); }
static int dpm_suspend_noirq(int msg) { (void)msg; return callback(); }
static int platform_suspend_prepare_noirq(int state) { (void)state; return callback(); }
static int suspend_test(int level) { tests++; return level == TEST_PLATFORM; }
static void s2idle_loop(void) { idle_enter++; }
static int pm_sleep_disable_secondary_cpus(void) { cpu_off++; return 0; }
static void pm_sleep_enable_secondary_cpus(void) { cpu_on++; }
static void arch_suspend_disable_irqs(void) { irq_disabled = true; }
static void arch_suspend_enable_irqs(void) { irq_disabled = false; }
static bool irqs_disabled(void) { return irq_disabled; }
static int syscore_suspend(void) { sys_suspended++; return 0; }
static void syscore_resume(void) {}
static bool pm_wakeup_pending(void) { return false; }
static void trace_suspend_resume(const char *name, int state, bool begin)
{ (void)name; (void)state; (void)begin; }
static int enter(int state) { (void)state; machine_enter++; return 0; }
static struct { int (*enter)(int); } ops = { enter }, *suspend_ops = &ops;
static void platform_resume_noirq(int state) { (void)state; }
static void dpm_resume_noirq(int msg) { (void)msg; noirq_resume++; }
static void platform_resume_early(int state) { (void)state; }
static void dpm_resume_early(int msg) { (void)msg; early_resume++; }
static void platform_resume_finish(int state) { (void)state; finish++; }
'''
MAIN = r'''
int main(void)
{
    for (int error = 0; error <= 5; error++) {
        fail_step = error; step = tests = cpu_off = cpu_on = machine_enter = 0;
        idle_enter = noirq_resume = early_resume = finish = sys_suspended = 0;
        system_state = SYSTEM_RUNNING; irq_disabled = false;
        bool wakeup = false;
        int result = suspend_enter(3, &wakeup);
        assert(result == (error ? -5 : 0));
        assert(!cpu_off && !cpu_on && !machine_enter && !idle_enter && !sys_suspended);
        assert(!irq_disabled && system_state == SYSTEM_RUNNING && finish == 1);
        assert(noirq_resume == (error == 0 || error == 5));
        assert(early_resume == (error == 0 || error >= 3));
        assert(tests == (error == 0));
    }
    puts("platform_boundary=pass cases=6 machine_sleep=0 cpu_offline=0 rollback_paths=5");
    return 0;
}
'''


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('suspend_source', type=Path)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    a.output.mkdir(parents=True, exist_ok=False)
    source = a.suspend_source.read_text()
    harness = a.output / 'actual-suspend-enter.c'
    harness.write_text(PRELUDE + metadata.function(source, 'suspend_enter') + MAIN)
    with (a.output / 'model.log').open('x') as log:
        binary = a.output / 'model'
        subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                        '-fsanitize=undefined', '-fno-sanitize-recover=undefined',
                        str(harness), '-o', str(binary)], check=True, stdout=log, stderr=log)
        subprocess.run([str(binary)], check=True, timeout=10, stdout=log, stderr=log)
    result = {'status': 'pass', 'cases': 6,
              'source_sha256': hashlib.sha256(a.suspend_source.read_bytes()).hexdigest(),
              'scope': 'host callback-flow model only; platform dry run avoids CPU offlining and machine sleep; not hardware proof'}
    (a.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
