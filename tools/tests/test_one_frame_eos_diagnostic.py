"""EOS diagnostics must stop at faults and never certify zero VA output."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class OneFrameDiagnosticTests(unittest.TestCase):
    def run_diagnostic(self, native_fault=False, va_frames=1):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            tools = base / "tools"
            tools.mkdir()
            for name in ("diagnose-one-frame-eos.sh", "compare-frame-repeats.py"):
                shutil.copy2(ROOT / "tools" / name, tools / name)
            wrapper = tools / "capture-iris-kernel-log.sh"
            wrapper.write_text('''#!/usr/bin/env bash
shift
"$@"
status=$?
if [[ "$*" == *h264_v4l2m2m* && "$NATIVE_FAULT" == 1 ]]; then
 echo 'summary: session-fatal(0x4000003)=1  system-fatal(0x5000003)=0  power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0'
 exit 1
fi
echo 'summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0  power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0'
exit "$status"
''')
            wrapper.chmod(0o755)
            bin_dir = base / "bin"
            bin_dir.mkdir()
            programs = {
                "ffprobe": "print('1')\n",
                "flock": "pass\n",  # Never acquire the real device lock in host tests.
                "strace": "import os,sys\na=sys.argv; i=a.index('ffmpeg'); os.execvp('ffmpeg',a[i:])\n",
                "ffmpeg": '''import json,os,sys
from pathlib import Path
args=sys.argv[1:]
with Path(os.environ['CALLS']).open('a') as stream: stream.write(json.dumps(args)+'\\n')
count=0 if 'h264_v4l2m2m' in args else int(os.environ['VA_FRAMES']) if '-hwaccel' in args else 1
Path(args[-1]).write_text('#format: frame checksums\\n'+''.join(f'0,0,0,1,115200,31357095c263c15fdce9a108daf8abbf\\n' for i in range(count)))
''',
            }
            for name, program in programs.items():
                executable = bin_dir / name
                executable.write_text("#!" + sys.executable + "\n" + program)
                executable.chmod(0o755)
            driver = base / "driver"
            driver.mkdir()
            (driver / "msm_drv_video.so").write_bytes(b"testdriver")
            fixture = base / "one.mp4"
            fixture.touch()
            calls = base / "calls"
            result = subprocess.run(["bash", str(tools / "diagnose-one-frame-eos.sh"),
                                     str(driver), str(base / "results")],
                                    env={**os.environ, "PATH": str(bin_dir) + os.pathsep + os.environ['PATH'],
                                         "V4L2_VA_DEVICE": "/dev/null", "V4L2_VA_DRM_DEVICE": "/dev/null",
                                         "V4L2_VA_ONE_FRAME_SAMPLE": str(fixture),
                                         "V4L2_VA_EOS_EXPECTED_DRIVER_SHA256": hashlib.sha256(b"testdriver").hexdigest(),
                                         "NATIVE_FAULT": str(int(native_fault)), "VA_FRAMES": str(va_frames),
                                         "CALLS": str(calls)}, capture_output=True, text=True, timeout=10)
            return result, [json.loads(line) for line in calls.read_text().splitlines()]

    def test_native_zero_output_is_diagnostic_not_va_failure_or_release_pass(self):
        result, calls = self.run_diagnostic()
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)
        self.assertIn("va_frames=1 native_frames=0 qualification=single_fixture_only", result.stdout)
        self.assertEqual(len(calls), 3)
        self.assertIn("hwdownload,format=nv12", calls[-1])
        self.assertIn("-hwaccel_output_format", calls[-1])

    def test_native_kernel_fault_stops_before_va_probe_without_retries(self):
        result, calls = self.run_diagnostic(native_fault=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn("phase=native", result.stdout)
        self.assertEqual(len(calls), 2)
        self.assertFalse(any("-hwaccel" in args for args in calls))

    def test_zero_va_output_cannot_pass(self):
        result, calls = self.run_diagnostic(va_frames=0)
        self.assertEqual(result.returncode, 1)
        self.assertIn("driver_zero_or_incorrect_output", result.stdout)
        self.assertEqual(len(calls), 3)


if __name__ == "__main__":
    unittest.main()
