"""Exercise the complete EOS runner without opening hardware devices."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
COUNTERS = ["session-fatal(0x4000003)", "system-fatal(0x5000003)",
            "power-cycles", "vb2-warns", "other-session", "other-system", "kernel-bugs"]


class EosFailFastTests(unittest.TestCase):
    def run_probe(self, phase=1, failure="", counter=COUNTERS[0]):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            tools = base / "tools"
            tools.mkdir()
            for name in ["verify-eos-drain.sh", "check-playback-performance.py", "compare-frame-repeats.py"]:
                shutil.copy(ROOT / "tools" / name, tools / name)
            programs = {
                "ffprobe": "print(300)\n",
                "ffmpeg": '''import os,sys
from pathlib import Path
args=sys.argv[1:]
hardware='-hwaccel' in args
count=int(args[args.index('-frames:v')+1]) if '-frames:v' in args else 300
if hardware and count==1:
    with Path(os.environ['SANITY_CALLS']).open('a') as f: f.write('sanity\\n')
if hardware and '-frames:v' not in args and os.environ['FAILURE']=='partial' and int(os.environ.get('LEG','0'))==int(os.environ['FAIL_PHASE']): count=1
Path(args[-1]).write_text('#tb 0: 1/30\\n'+''.join(f'0, {i}, {i}, 1, 384, '+ 'a'*32 +'\\n' for i in range(count)))
''',
                "mpv": '''import sys
from pathlib import Path
for arg in sys.argv[1:]:
    if arg.startswith('--log-file='): Path(arg.split('=',1)[1]).write_text('Using hardware decoding (vaapi-copy)\\n')
''',
            }
            for name, program in programs.items():
                path = base / name
                path.write_text("#!" + sys.executable + "\n" + program)
                path.chmod(0o755)
            wrapper = tools / "capture-iris-kernel-log.sh"
            wrapper.write_text("#!" + sys.executable + '''
import os,sys,subprocess
from pathlib import Path
p=Path(os.environ['LEG_CALLS'])
sanity='-frames:v' in sys.argv
leg=0 if sanity else (len(p.read_text().splitlines())+1 if p.exists() else 1)
if not sanity:
    with p.open('a') as f: f.write(str(leg)+'\\n')
env={**os.environ,'LEG':str(leg)}
status=subprocess.run(sys.argv[2:],env=env).returncode
failed=leg==int(os.environ['FAIL_PHASE'])
failure=os.environ['FAILURE'] if failed else ''
if failure!='missing':
    counters=os.environ['COUNTERS'].split('|')
    print('  summary: '+'  '.join(c+'='+str(int(failure=='fault' and c==os.environ['COUNTER'])) for c in counters))
raise SystemExit(1 if failure=='command' else status)
''')
            wrapper.chmod(0o755)
            sample = base / "sample.mp4"
            sample.touch()
            result = subprocess.run(["bash", str(tools / "verify-eos-drain.sh"), str(base / "driver")],
                env={**os.environ, "PATH": str(base)+os.pathsep+os.environ["PATH"],
                     "V4L2_VA_SAMPLE": str(sample), "V4L2_VA_EOS_DIR": str(base / "results"),
                     "LEG_CALLS": str(base / "legs"), "SANITY_CALLS": str(base / "sanity"),
                     "FAIL_PHASE": str(phase), "FAILURE": failure, "COUNTER": counter,
                     "COUNTERS": "|".join(COUNTERS)}, capture_output=True, text=True, timeout=15)
            legs = (base / "legs").read_text().splitlines() if (base / "legs").exists() else []
            sanity = (base / "sanity").read_text().splitlines()
            return result, len(legs), len(sanity)

    def test_clean_complete_run_reaches_all_legs_and_closing_sanity(self):
        result, legs, sanity = self.run_probe()
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual((legs, sanity), (3, 2))
        self.assertIn("eos_drain=pass", result.stdout)

    def test_unhealthy_initial_sanity_never_opens_next_decoder(self):
        for failure in ("fault", "missing", "command"):
            with self.subTest(failure=failure):
                result, legs, sanity = self.run_probe(0, failure)
                self.assertEqual(result.returncode, 77, result.stdout+result.stderr)
                self.assertEqual((legs, sanity), (0, 1))

    def test_every_kernel_fault_stops_before_next_decoder(self):
        for phase in (1, 2, 3):
            for counter in COUNTERS:
                with self.subTest(phase=phase, counter=counter):
                    result, legs, sanity = self.run_probe(phase, "fault", counter)
                    self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
                    self.assertEqual((legs, sanity), (phase, 1))

    def test_command_failure_or_missing_evidence_stops_before_next_decoder(self):
        for phase in (1, 2, 3):
            for failure in ("command", "missing"):
                with self.subTest(phase=phase, failure=failure):
                    result, legs, sanity = self.run_probe(phase, failure)
                    self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
                    self.assertEqual((legs, sanity), (phase, 1))

    def test_incomplete_output_stops_before_next_decoder(self):
        for phase in (1, 3):
            with self.subTest(phase=phase):
                result, legs, sanity = self.run_probe(phase, "partial")
                self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
                self.assertEqual((legs, sanity), (phase, 1))


if __name__ == "__main__":
    unittest.main()
