"""Regression tests for checks that previously certified incomplete playback."""
import os
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import shutil
import unittest

ROOT = Path(__file__).resolve().parents[2]


class EdgeReferenceTests(unittest.TestCase):
    def run_probe(self, label="one-frame-eos", expected=1, software=1, hardware=1, status=0):
        source = (ROOT / "tools/verify-rust-driver.sh").read_text()
        functions = source[source.index("run_native_framemd5() {"):source.index("\nverify_framemd5 sample-1")]
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            (base / "input").touch()
            tools = base / "tools"
            tools.mkdir()
            for name in ('hardware-session.sh', 'check-playback-performance.py'):
                shutil.copy2(ROOT / 'tools' / name, tools / name)
            modules = base / 'modules'
            modules.write_text('')
            helper = tools / 'hardware-session.sh'
            helper.write_text(helper.read_text().replace('/proc/modules', str(modules)))
            wrapper = tools / 'capture-iris-kernel-log.sh'
            wrapper.write_text('#!/bin/bash\nstatus=0\n"${@:2}" || status=$?\n'
                               "echo 'summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0 "
                               "power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0'\n"
                               'exit "$status"\n')
            wrapper.chmod(0o755)
            programs = {
                "ffprobe": "import os\nprint(os.environ['EXPECTED'])\n",
                "ffmpeg": """import os, sys, json
from pathlib import Path
args = sys.argv[1:]
hw = '-hwaccel' in args
with Path(os.environ['CALLS']).open('a') as stream: stream.write(json.dumps(args)+'\\n')
count = int(os.environ['HARDWARE'] if hw else os.environ['SOFTWARE'])
if '-frames:v' in args: count = min(count, int(args[args.index('-frames:v')+1]))
Path(args[-1]).write_text('#tb 0: 1/30\\n'+''.join(f'0,{i},{i},1,384,aaa\\n' for i in range(count)))
raise SystemExit(int(os.environ['STATUS']) if hw else 0)
""",
            }
            for name, program in programs.items():
                executable = base / name
                executable.write_text("#!" + sys.executable + "\n" + program)
                executable.chmod(0o755)
            result = subprocess.run(["bash", "-c",
                'set -euo pipefail\nwork_dir="$WORK_DIR"\ndriver_dir="$WORK_DIR/driver"\n'
                'repo_root="$WORK_DIR"\nsource "$repo_root/tools/hardware-session.sh"\n'
                'drm_device=/dev/fake\nnative_decoder=h264_v4l2m2m\n' + functions +
                '\nverify_framemd5 "$1" "$WORK_DIR/input" "$2" required', "_", label,
                "30" if label == "sample-30" else ""],
                env={**os.environ, "PATH": str(base)+os.pathsep+os.environ["PATH"],
                     "WORK_DIR": tmp, "CALLS": str(base / "calls"), "EXPECTED": str(expected),
                     "SOFTWARE": str(software), "HARDWARE": str(hardware), "STATUS": str(status)},
                capture_output=True, text=True, timeout=10)
            calls = [json.loads(line) for line in (base / "calls").read_text().splitlines()]
            return result, calls

    def test_edge_requires_complete_independent_reference_and_real_va_output(self):
        result, calls = self.run_probe()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("reference=software", result.stdout)
        self.assertIn("hwdownload,format=nv12", calls[-1])
        self.assertIn("-hwaccel_output_format", calls[-1])
        self.assertIn("vaapi", calls[-1])

    def test_equally_partial_reference_and_driver_cannot_pass(self):
        result, calls = self.run_probe(label="bframes-240p", expected=3, software=2, hardware=2)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("incomplete_software_reference", result.stdout)
        self.assertEqual(len(calls), 1)

    def test_failed_hardware_decode_cannot_pass_with_complete_output(self):
        result, _ = self.run_probe(status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("framemd5_ok", result.stdout)

    def test_required_matrix_keeps_native_reference(self):
        result, calls = self.run_probe(label="sample-30", expected=30, software=30, hardware=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("h264_v4l2m2m", calls[0])
        self.assertIn("reference=native", result.stdout)


class VerificationTests(unittest.TestCase):
    def compare(self, gl_frames, ref_frames, requested=0, trailing=b"", ordered=False):
        with tempfile.TemporaryDirectory() as tmp:
            gl = Path(tmp) / "gl.raw"
            ref = Path(tmp) / "ref.raw"
            gl.write_bytes(b"".join(gl_frames) + trailing)
            ref.write_bytes(b"".join(ref_frames))
            return subprocess.run([
                sys.executable, str(ROOT / "tools/gst_gl_roundtrip.py"),
                str(gl), str(ref), "--width", "16", "--height", "16",
                "--stride", "16", "--ref-stride", "16", "--frames", str(requested),
            ] + (["--ordered"] if ordered else []), capture_output=True, text=True, timeout=10)

    def test_identical_complete_pixels_pass(self):
        frames = [bytes([i]) * 384 for i in (1, 2, 3)]
        self.assertEqual(self.compare(frames, frames, 3).returncode, 0)

    def test_strict_display_order_rejects_reordered_complete_pixels(self):
        frames = [bytes([i]) * 384 for i in (1, 2, 3)]
        self.assertEqual(self.compare(frames, frames, 3, ordered=True).returncode, 0)
        self.assertNotEqual(self.compare(frames[::-1], frames, 3, ordered=True).returncode, 0)

    def test_strict_display_order_rejects_extra_corrupt_frames(self):
        frames = [bytes([i]) * 384 for i in (1, 2, 3)]
        self.assertNotEqual(self.compare(frames + [bytes([99]) * 384], frames, 3, ordered=True).returncode, 0)

    def test_production_gate_refuses_experimental_av1(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run(["bash", str(ROOT / "tools/verify-production.sh")],
                env={**os.environ, "V4L2_VA_PRODUCTION_DIR": tmp, "V4L2_VA_EXPERIMENTAL_AV1": "1"},
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 1)
            self.assertIn("experimental_av1_not_qualified", result.stdout)

    def test_missing_frame_fails_by_default(self):
        frames = [bytes([i]) * 384 for i in (1, 2, 3)]
        result = self.compare(frames[:2], frames, 3)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("missing=1", result.stdout)

    def test_one_gl_frame_cannot_satisfy_two_identical_reference_frames(self):
        frame = bytes([1]) * 384
        self.assertNotEqual(self.compare([frame], [frame, frame], 2).returncode, 0)

    def test_short_reference_does_not_pass_requested_frame_count(self):
        frame = bytes([1]) * 384
        self.assertNotEqual(self.compare([frame], [frame], 2).returncode, 0)

    def test_forced_stride_does_not_hide_a_truncated_dump(self):
        frame = bytes([1]) * 384
        self.assertNotEqual(self.compare([frame], [frame], 1, b"x").returncode, 0)

    def test_required_media_missing_fails_before_hardware_access(self):
        with tempfile.TemporaryDirectory() as tmp:
            result = subprocess.run([
                "bash", str(ROOT / "tools/verify-rust-driver.sh"),
            ], env={**os.environ, "V4L2_VA_SAMPLE": str(Path(tmp) / "missing.mp4")},
                capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("missing_required_sample", result.stdout)

    def test_failed_gl_pipeline_cannot_pass_with_correct_partial_pixels(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            tools = base / 'tools'
            tools.mkdir()
            for name in ('verify-gl-roundtrip.sh', 'hardware-session.sh',
                         'check-playback-performance.py', 'gst_gl_roundtrip.py'):
                shutil.copy2(ROOT / 'tools' / name, tools / name)
            modules = base / 'modules'
            modules.write_text('')
            helper = tools / 'hardware-session.sh'
            helper.write_text(helper.read_text().replace('/proc/modules', str(modules)))
            wrapper = tools / 'capture-iris-kernel-log.sh'
            wrapper.write_text('#!/bin/bash\nstatus=0\n"${@:2}" || status=$?\n'
                               "echo 'summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0 "
                               "power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0'\n"
                               'exit "$status"\n')
            wrapper.chmod(0o755)
            fixture = base / "fixture.raw"
            fixture.write_bytes(bytes([1]) * 384)
            fakebin = base / "bin"
            fakebin.mkdir()
            programs = {
                "ffmpeg": "import os, pathlib, sys\npathlib.Path(sys.argv[-1]).write_bytes(pathlib.Path(os.environ['FIXTURE']).read_bytes())\n",
                "ffprobe": "print('16,16')\n",
                "gst-inspect-1.0": "pass\n",
                "gst-launch-1.0": "import os, pathlib, sys\nlocations = [x.split('=', 1)[1] for x in sys.argv if x.startswith('location=')]\npathlib.Path(locations[-1]).write_bytes(pathlib.Path(os.environ['FIXTURE']).read_bytes())\nprint('msm_drv_video_rs: ExportSurfaceHandle succeeded')\nsys.exit(1)\n",
            }
            for name, program in programs.items():
                executable = fakebin / name
                executable.write_text("#!" + sys.executable + "\n" + program)
                executable.chmod(0o755)
            result = subprocess.run([
                "bash", str(tools / "verify-gl-roundtrip.sh"), str(base / "driver"),
            ], env={**os.environ, "PATH": str(fakebin) + os.pathsep + os.environ['PATH'],
                    "FIXTURE": str(fixture), "V4L2_VA_SAMPLE": str(fixture),
                    "V4L2_VA_GL_RT_DIR": str(base / "results"), "V4L2_VA_GL_RT_FRAMES": "1"},
                capture_output=True, text=True, timeout=10)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("partial_layout_pass", result.stdout)


if __name__ == "__main__":
    unittest.main()
