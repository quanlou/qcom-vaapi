"""Every churn session must stop before another open after unsafe evidence."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
COUNTERS = ["session-fatal(0x4000003)", "system-fatal(0x5000003)", "power-cycles",
            "vb2-warns", "other-session", "other-system", "kernel-bugs"]


class ChurnFailFastTests(unittest.TestCase):
    def probe(self, phase=1, failure="", counter=COUNTERS[0]):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            tools = base / "tools"
            tools.mkdir()
            for name in ("verify-session-churn.sh", "check-playback-performance.py", "compare-frame-repeats.py"):
                shutil.copy(ROOT / "tools" / name, tools / name)
            programs = {
                "ffprobe": "print(300)\n",
                "ffmpeg": '''import os,sys
from pathlib import Path
count=1 if os.environ.get('FAILURE')=='partial' and int(os.environ.get('LEG','0'))==int(os.environ['PHASE']) else 300
Path(sys.argv[-1]).write_text('#tb 0: 1/30\\n'+''.join(f'0,{i},{i},1,384,aaa\\n' for i in range(count)))
''',
                "mpv": "print('Using hardware decoding (vaapi-copy)')\n",
                "gst-launch-1.0": "pass\n",
                "timeout": '''import os,sys
args=sys.argv[1:]
i=args.index('env') if 'env' in args else args.index('ffmpeg')
status=os.spawnvp(os.P_WAIT,args[i],args[i:])
if 'KILL' in args: status=137
elif 'TERM' in args: status=124
raise SystemExit(status)
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
p=Path(os.environ['CALLS'])
leg=len(p.read_text().splitlines())+1 if p.exists() else 1
with p.open('a') as stream: stream.write(str(leg)+'\\n')
status=subprocess.run(sys.argv[2:],env={**os.environ,'LEG':str(leg)}).returncode
failed=leg==int(os.environ['PHASE'])
failure=os.environ['FAILURE'] if failed else ''
if failure!='missing':
 print(' summary: '+'  '.join(c+'='+str(int(failure=='fault' and c==os.environ['COUNTER'])) for c in os.environ['COUNTERS'].split('|')))
if failure=='command': status=1
if failure=='wrong_kill_status': status=0
raise SystemExit(status)
''')
            wrapper.chmod(0o755)
            sample = base / "sample.mp4"
            sample.touch()
            result = subprocess.run(["bash", str(tools / "verify-session-churn.sh"), str(base / "driver")],
                                    env={**os.environ, "PATH": str(base)+os.pathsep+os.environ['PATH'],
                                         "V4L2_VA_SAMPLE": str(sample), "V4L2_VA_CHURN_DIR": str(base / "results"),
                                         "COUNTERS": '|'.join(COUNTERS), "COUNTER": counter, "CALLS": str(base / "calls"),
                                         "PHASE": str(phase), "FAILURE": failure},
                                    capture_output=True, text=True, timeout=15)
            return result, len((base / "calls").read_text().splitlines())

    def test_complete_clean_run_preserves_seven_required_checks(self):
        result, opens = self.probe()
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual(opens, 13)
        self.assertIn('session-churn: pass=7 fail=0', result.stdout)

    def test_fault_in_normal_or_expected_kill_session_stops_immediately(self):
        for phase in (2, 8, 10, 12):
            for counter in COUNTERS:
                with self.subTest(phase=phase, counter=counter):
                    result, opens = self.probe(phase, 'fault', counter)
                    self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
                    self.assertEqual(opens, phase)

    def test_missing_kernel_evidence_stops_even_after_expected_kill(self):
        for phase in (1, 8, 10, 12):
            result, opens = self.probe(phase, 'missing')
            self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
            self.assertEqual(opens, phase)

    def test_wrong_command_status_or_partial_recovery_stops(self):
        for phase, failure in ((2, 'command'), (8, 'wrong_kill_status'), (9, 'partial')):
            result, opens = self.probe(phase, failure)
            self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
            self.assertEqual(opens, phase)


if __name__ == '__main__':
    unittest.main()
