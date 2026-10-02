"""Exercise the real 4K shell argv with host-only fake decode commands."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class FourKThreadScopeTests(unittest.TestCase):
    def test_input_and_output_thread_limits_cover_every_native_and_va_leg(self):
        for codec in ("h264", "hevc", "hevc10", "vp9"):
            with self.subTest(codec=codec), tempfile.TemporaryDirectory(prefix="4k-argv-host-only-") as tmp:
                directory = Path(tmp)
                tools = directory / "tools"
                tools.mkdir()
                for name in ("verify-4k-decode.sh", "compare-frame-repeats.py", "check-playback-performance.py"):
                    shutil.copy2(ROOT / "tools" / name, tools / name)
                wrapper = tools / "capture-iris-kernel-log.sh"
                wrapper.write_text("#!" + sys.executable + "\nimport subprocess,sys\n"
                                   "status=subprocess.call(sys.argv[2:])\n"
                                   "print('summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0 "
                                   "power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0')\n"
                                   "sys.exit(status)\n")
                wrapper.chmod(0o755)
                fakebin = directory / "fakebin"
                fakebin.mkdir()
                scripts = {
                    "flock": "# No real hardware lease is taken by the mock.\n",
                    "ffprobe": "import os,sys\nprint('30' if '-count_frames' in sys.argv else os.environ['MOCK_CODEC']+',3840,2160')\n",
                    "ffmpeg": "import json,os,sys,time\n"
                    "args=sys.argv[1:]\n"
                    "with open(os.environ['MOCK_ARGV'], 'a') as log: log.write(json.dumps(args)+'\\n')\n"
                    "frames=int(args[args.index('-frames:v')+1]) if '-frames:v' in args else 30\n"
                    "if '-frames:v' not in args and '-stream_loop' in args: frames *= int(args[args.index('-stream_loop')+1])+1\n"
                    "with open(args[-1], 'w') as output:\n"
                    " output.write('#tb 0: 1/30\\n')\n"
                    " for i in range(frames): output.write(f'0,{i},{i},1,384,hash{i%30}\\n')\n"
                    "time.sleep(0.03)\n",
                }
                for name, script in scripts.items():
                    executable = fakebin / name
                    executable.write_text("#!" + sys.executable + "\n" + script)
                    executable.chmod(0o755)
                argv_log = directory / "argv.jsonl"
                result = subprocess.run(["bash", str(tools / "verify-4k-decode.sh")], env={
                    **os.environ, "PATH": str(fakebin) + os.pathsep + os.environ["PATH"],
                    "MOCK_CODEC": "hevc" if codec == "hevc10" else codec, "MOCK_ARGV": str(argv_log),
                    "V4L2_VA_4K_CODEC": codec, "V4L2_VA_4K_LOG_DIR": str(directory / "results"),
                    "V4L2_VA_4K_SAMPLE": str(directory / "mock-media-never-decoded.mp4"),
                    "V4L2_VA_4K_LOOPS": "2", "V4L2_VA_4K_MIN_FPS": "0", "V4L2_VA_4K_MAX_RSS_KIB": "0",
                    "V4L2_VA_4K_MIN_SECONDS": "0", "V4L2_VA_4K_STRICT": "0", "LIBVA_DRIVER_NAME": "msm",
                }, capture_output=True, text=True, timeout=20)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                invocations = [json.loads(line) for line in argv_log.read_text().splitlines()]
                self.assertEqual(len(invocations), 6)  # Native and VA for 1/30/full.
                for args in invocations:
                    boundary = args.index("-i")
                    input_threads = [args[i + 1] for i, value in enumerate(args[:boundary]) if value == "-threads:v"]
                    output_threads = [args[i + 1] for i, value in enumerate(args) if value == "-threads:v" and i > boundary]
                    self.assertEqual(input_threads, ["1"], f"unbounded input decoder: {args}")
                    self.assertEqual(output_threads, ["1"], f"unbounded output checksum encoder: {args}")


if __name__ == "__main__":
    unittest.main()
