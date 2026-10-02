#!/usr/bin/env python3
"""Exercise actual force-sleep and Iris callbacks in an isolated host model.

No device opens, system sleep, module operations or hardware support claims.
The generic PM helpers and IRQ/firmware are modeled; callback bodies are read
from the supplied source trees without modifying those trees.
"""
import argparse
import hashlib
import importlib.util
from pathlib import Path
import subprocess
import tempfile


def load_model():
    spec = importlib.util.spec_from_file_location(
        "iris_pm_model", Path(__file__).with_name("verify-iris-pm.py"))
    model = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(model)
    return model


HELPERS = r'''
#define RPM_ACTIVE 0
#define RPM_SUSPENDED 1
#define GET_CALLBACK(dev, name) name##_callback
#define runtime_suspend_callback iris_pm_suspend
#define runtime_resume_callback iris_pm_resume
static void pm_runtime_disable(struct device *dev) {
 assert(dev->power.enabled); dev->power.enabled=false;
}
static void pm_runtime_enable(struct device *dev) {
 assert(!dev->power.enabled); dev->power.enabled=true;
}
static bool pm_runtime_status_suspended(struct device *dev) {
 return dev->power.runtime_status==RPM_SUSPENDED;
}
static bool pm_runtime_need_not_resume(struct device *dev) { return !dev->refs; }
static bool dev_pm_smart_suspend(struct device *dev) { return dev->power.smart_suspend; }
static void pm_runtime_set_suspended(struct device *dev) {
 dev->power.runtime_status=RPM_SUSPENDED;
}
static void dev_pm_enable_wake_irq_check(struct device *dev, bool on) {(void)dev;(void)on;}
static void dev_pm_enable_wake_irq_complete(struct device *dev) {(void)dev;}
static void dev_pm_disable_wake_irq_check(struct device *dev, bool on) {(void)dev;(void)on;}
'''

MAIN = r'''
int main(int argc, char **argv) {
 assert(argc==2);
 struct device dev={.power={.enabled=true,.runtime_status=RPM_ACTIVE}};
 struct vpu_ops vpu={off_hw,off_controller};
 struct platform_data platform={&vpu};
 struct iris_hfi_sys_ops hfi={ifpc}; struct firmware_data firmware={init_ops};
 struct iris_core core={.dev=&dev,.lock=PTHREAD_MUTEX_INITIALIZER,
  .state=IRIS_CORE_INIT,.irq=232,.iris_platform_data=&platform,
  .hfi_sys_ops=&hfi,.iris_firmware_data=&firmware};
 current=&core; const char *scenario=argv[1]; int ret;
 if (!strcmp(scenario,"already-idle")) {
  dev.power.runtime_status=RPM_SUSPENDED; powered=false; depth=1;
  assert(pm_runtime_force_suspend(&dev)==0 && !dev.power.enabled && syncs==0);
  assert(pm_runtime_force_resume(&dev)==0 && dev.power.enabled && !powered && depth==1);
  assert(iris_pm_resume(&dev)==0 && powered && depth==0);
 } else if (!strcmp(scenario,"pending-irq")) {
  irq_active=true; depth=1;
  assert(pm_runtime_force_suspend(&dev)==-EBUSY);
  assert(dev.power.enabled && powered && queues && depth==1 && !power_offs);
  irq_active=false; depth=0;
 } else if (!strcmp(scenario,"suspend-error")) {
  pc_status=-EIO;
  assert(pm_runtime_force_suspend(&dev)==-EAGAIN);
  assert(dev.power.enabled && powered && queues && depth==0);
 } else if (!strcmp(scenario,"unused-active")) {
  assert(pm_runtime_force_suspend(&dev)==0 && !dev.power.enabled && !powered && depth==1);
  assert(dev.power.runtime_status==RPM_SUSPENDED && !dev.power.needs_force_resume);
  assert(pm_runtime_force_resume(&dev)==0 && dev.power.enabled && !powered && depth==1);
  assert(iris_pm_resume(&dev)==0 && powered && depth==0);
 } else {
  dev.refs=1;
  int cycles=!strcmp(scenario,"cycles") ? 10000 : 1;
  for (int i=0;i<cycles;i++) {
   assert(pm_runtime_force_suspend(&dev)==0 && !dev.power.enabled && !powered && depth==1);
   assert(dev.power.needs_force_resume);
   if (!strcmp(scenario,"resume-error")) fail_stage=4;
   ret=pm_runtime_force_resume(&dev);
   assert(dev.power.enabled && !dev.power.needs_force_resume && !dev.power.smart_suspend);
   if (fail_stage) {
    assert(ret==-EBUSY && !powered && depth==1 && dev.power.runtime_status==RPM_SUSPENDED);
   } else assert(ret==0 && powered && queues && depth==0);
  }
 }
 printf("iris_system_sleep_model_%s=pass (not hardware qualification)\n",scenario);
 return 0;
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kernel", type=Path, help="Linux tree with drivers/base/power/runtime.c")
    parser.add_argument("candidate", type=Path, help="Exact candidate Iris source tree")
    args = parser.parse_args()
    model = load_model()
    runtime = args.kernel / "drivers/base/power/runtime.c"
    source = runtime.read_text()
    callbacks = model.source_functions(args.candidate)
    probe = (args.candidate / "drivers/media/platform/qcom/iris/iris_probe.c").read_text()
    if "SET_SYSTEM_SLEEP_PM_OPS(pm_runtime_force_suspend," not in probe:
        raise SystemExit("candidate does not use the modeled force-sleep wiring")
    prelude = model.MODEL.replace(
        "struct device { int refs; };",
        "struct device { int refs; struct {bool enabled,needs_force_resume,smart_suspend; int runtime_status;} power; };")
    functions = "\n".join(model.METADATA.function("\n" + source[source.index("int " + name + "("):], name)
                          for name in ("pm_runtime_force_suspend", "pm_runtime_force_resume"))
    print("pm_framework_source_sha256=" + hashlib.sha256(runtime.read_bytes()).hexdigest())
    print("iris_callback_source_sha256=" + hashlib.sha256(callbacks.encode()).hexdigest())
    with tempfile.TemporaryDirectory(prefix="iris-system-sleep-model-") as tmp:
        cfile = Path(tmp) / "model.c"
        executable = Path(tmp) / "model"
        cfile.write_text(prelude + callbacks + HELPERS + functions + MAIN)
        subprocess.run(["cc", "-std=c11", "-pthread", "-Wall", "-Wextra", "-Werror",
                        "-Wno-unused-function", "-fsanitize=undefined",
                        "-fno-sanitize-recover=undefined", str(cfile), "-o", str(executable)], check=True)
        for scenario in ("already-idle", "pending-irq", "suspend-error", "unused-active",
                         "in-use", "resume-error", "cycles"):
            subprocess.run([str(executable), scenario], check=True, timeout=5)


if __name__ == "__main__":
    main()
