#!/usr/bin/env python3
"""Compare actual Iris clock calculations in an isolated C model.

Checks scope, resolution scaling and rate scaling; hardware validation remains
separate. Arguments are the baseline and candidate Iris source directories.
"""
import argparse
import importlib.util
import subprocess
import tempfile
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('metadata', Path(__file__).with_name('verify-iris-metadata.py'))
metadata = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata)

PRELUDE = r'''
#include <assert.h>
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
typedef uint32_t u32;
typedef uint64_t u64;
#define DECODER 0
#define ENCODER 1
#define V4L2_PIX_FMT_VP9 9
#define STAGE_1 1
#define STAGE_2 2
enum { PIPE, STAGE, DISPLAY_DELAY_ENABLE, DISPLAY_DELAY };
#define max(a,b) ((a) > (b) ? (a) : (b))
#define max3(a,b,c) max(max(a,b),c)
#define mult_frac(a,b,c) ((u64)(a)*(b)/(c))
#define div_u64(a,b) ((a)/(b))
#define NUM_MBS_PER_FRAME(h,w) (((h)+15)/16 * (((w)+15)/16))
struct platform_inst_caps { u32 mb_cycles_vpp, mb_cycles_fw, mb_cycles_fw_vpp; };
struct platform_data { struct platform_inst_caps *inst_caps; };
struct iris_core { struct platform_data *iris_platform_data; };
struct v4l2_format { struct { struct { u32 width, height; } pix_mp; } fmt; };
struct iris_inst {
 struct iris_core *core;
 struct v4l2_format *fmt_src;
 struct { u32 width, height; } crop;
 struct { u32 value; } fw_caps[4];
 u32 frame_rate, domain, codec;
};
'''
MAIN = r'''
int main(void) {
 struct platform_inst_caps caps = {200, 489583, 66234};
 struct platform_data pdata = {&caps};
 struct iris_core core = {&pdata};
 struct v4l2_format fmt = {.fmt.pix_mp = {3840,2160}};
 struct iris_inst inst = {.core=&core,.fmt_src=&fmt,.frame_rate=60,.codec=9};
 inst.fw_caps[PIPE].value=4; inst.fw_caps[STAGE].value=STAGE_2;
 for (u32 domain=0;domain<=1;domain++)
  for (u32 codec=8;codec<=9;codec++)
   for (u32 enable=0;enable<=1;enable++)
    for (u32 delay=0;delay<=1;delay++) {
     inst.domain=domain; inst.codec=codec;
     inst.fw_caps[DISPLAY_DELAY_ENABLE].value=enable;
     inst.fw_caps[DISPLAY_DELAY].value=delay;
     u64 a=baseline(&inst,10000), b=candidate(&inst,10000);
     if (domain==DECODER && codec==9 && enable && !delay) {
      assert(b>a*3 && b<a*5);
     } else assert(a==b);
    }
 inst.domain=DECODER; inst.codec=9;
 inst.fw_caps[DISPLAY_DELAY_ENABLE].value=1; inst.fw_caps[DISPLAY_DELAY].value=0;
 u64 full=candidate(&inst,10000);
 inst.frame_rate=30; assert(candidate(&inst,10000)==full/2);
 inst.frame_rate=60; fmt.fmt.pix_mp.width=1920;fmt.fmt.pix_mp.height=1088;
 assert(candidate(&inst,10000)<full/2);
 puts("iris_playback_clock=pass scope=isolated_actual_function_model");
}
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('baseline', type=Path)
    parser.add_argument('candidate', type=Path)
    args = parser.parse_args()
    name = 'iris_vpu3x_vpu4x_calculate_frequency'
    functions = []
    for label, tree in [('baseline', args.baseline), ('candidate', args.candidate)]:
        body = metadata.function((tree / 'iris_vpu_common.c').read_text(), name)
        functions.append(body.replace(name, label, 1))
    with tempfile.TemporaryDirectory(prefix='iris-clock-model-') as folder:
        source = Path(folder) / 'model.c'
        executable = Path(folder) / 'model'
        source.write_text(PRELUDE + '\n'.join(functions) + MAIN)
        subprocess.run(['cc', '-std=c11', '-Wall', '-Wextra', '-Werror',
                        '-fsanitize=undefined', str(source), '-o', str(executable)], check=True)
        subprocess.run([str(executable)], check=True)


if __name__ == '__main__':
    main()
