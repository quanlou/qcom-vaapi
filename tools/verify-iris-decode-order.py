#!/usr/bin/env python3
"""Check the candidate's actual decode-order setter in an isolated C model.

This verifies property selection, defaults and error propagation, not firmware
support, decoded pixels, performance or V4L2 control locking on real hardware.
"""
import argparse
import importlib.util
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('metadata', ROOT / 'tools/verify-iris-metadata.py')
metadata = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata)

PRELUDE = r'''
#include <assert.h>
#include <errno.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdio.h>
typedef unsigned int u32;
enum platform_inst_fw_cap_type { DISPLAY_DELAY_ENABLE = 1, DISPLAY_DELAY = 2 };
#define CAP_FLAG_CLIENT_SET 8
#define HFI_PROP_DECODE_ORDER_OUTPUT 0x0300015b
#define HFI_HOST_FLAGS_NONE 0
#define HFI_PORT_BITSTREAM 1
#define HFI_PAYLOAD_U32 3
struct iris_inst;
struct iris_hfi_session_ops {
 int (*session_set_property)(struct iris_inst *, u32, u32, u32, u32, void *, u32);
};
struct platform_inst_fw_cap { u32 flags, value; };
struct iris_inst {
 const struct iris_hfi_session_ops *hfi_session_ops;
 struct platform_inst_fw_cap fw_caps[3];
};
static int calls, last_order, result;
static int set_property(struct iris_inst *inst, u32 id, u32 flags, u32 port,
                        u32 payload, void *data, u32 size)
{
 (void)inst;
 assert(id == HFI_PROP_DECODE_ORDER_OUTPUT && flags == 0);
 assert(port == HFI_PORT_BITSTREAM && payload == HFI_PAYLOAD_U32);
 assert(size == sizeof(u32));
 last_order = *(u32 *)data; calls++; return result;
}
'''
MAIN = r'''
int main(void)
{
 const struct iris_hfi_session_ops ops = { .session_set_property = set_property };
 struct iris_inst inst = { .hfi_session_ops = &ops };
 assert(iris_set_decode_order(&inst, DISPLAY_DELAY_ENABLE) == 0 && calls == 0);
 inst.fw_caps[DISPLAY_DELAY_ENABLE].flags = CAP_FLAG_CLIENT_SET;
 for (int enable = 0; enable < 2; enable++) {
  for (int delay = 0; delay < 2; delay++) {
   inst.fw_caps[DISPLAY_DELAY_ENABLE].value = enable;
   inst.fw_caps[DISPLAY_DELAY].value = delay;
   int before = calls;
   assert(iris_set_decode_order(&inst, DISPLAY_DELAY_ENABLE) == 0);
   assert(calls == before + 1 && last_order == (enable && !delay));
   assert(iris_set_decode_order(&inst, DISPLAY_DELAY) == 0);
   assert(calls == before + 2 && last_order == (enable && !delay));
  }
 }
 result = -EIO;
 assert(iris_set_decode_order(&inst, DISPLAY_DELAY_ENABLE) == -EIO);
 assert(iris_set_decode_order(&inst, (enum platform_inst_fw_cap_type)0) == -EINVAL);
 puts("iris_decode_order=pass scope=source_model defaults=preserved modes=4 error=propagated");
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('linux_source', type=Path)
    args = parser.parse_args()
    source = args.linux_source / 'drivers/media/platform/qcom/iris'
    controls = (source / 'iris_ctrls.c').read_text()
    table = (source / 'iris_hfi_gen2.c').read_text().split('inst_fw_cap_sm8550_dec[]', 1)[1].split('};', 1)[0]
    for cap in ('DISPLAY_DELAY_ENABLE', 'DISPLAY_DELAY'):
        assert f'.cap_id = {cap},' in table
    assert table.count('.hfi_id = HFI_PROP_DECODE_ORDER_OUTPUT,') == 2
    assert table.count('.set = iris_set_decode_order,') == 2
    assert '!V4L2_TYPE_IS_OUTPUT(plane)' in metadata.function(controls, 'iris_set_properties')
    assert 'return ret;' in metadata.function(controls, 'iris_set_properties')
    assert 'vb2_is_streaming(q)' in metadata.function(controls, 'iris_op_s_ctrl')
    with tempfile.TemporaryDirectory(prefix='iris-order-model-') as tmp:
        work = Path(tmp)
        test = work / 'test.c'
        test.write_text(PRELUDE + metadata.function(controls, 'iris_set_decode_order') + MAIN)
        subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror', '-fsanitize=undefined',
                        '-fno-sanitize-recover=undefined', str(test), '-o', str(work / 'test')], check=True)
        subprocess.run([str(work / 'test')], check=True, timeout=10)


if __name__ == '__main__':
    main()
