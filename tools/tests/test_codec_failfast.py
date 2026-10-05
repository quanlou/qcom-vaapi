"""Failed codec legs stop before another decoder open and retain attribution."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CLEAN = ('summary: session-fatal(0x4000003)=0 system-fatal(0x5000003)=0 '
         'power-cycles=0 vb2-warns=0 other-session=0 other-system=0 kernel-bugs=0')


class CodecFailFastTests(unittest.TestCase):
    def check_failed_leg(self, frames):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / 'tools').mkdir()
            (root / 'bin').mkdir()
            (root / 'samples').mkdir()
            for name in ('verify-codec-expansion.sh', 'hardware-session.sh',
                         'check-playback-performance.py'):
                shutil.copy2(ROOT / 'tools' / name, root / 'tools' / name)
            for name in ('hevc-main-720p.mp4', 'vp9-720p.webm'):
                (root / 'samples' / name).touch()
            wrapper = root / 'tools' / 'capture-iris-kernel-log.sh'
            wrapper.write_text('#!/bin/bash\n[[ "${1:-}" == -- ]] && shift\n"$@"\nstatus=$?\nprintf "%s\\n" "$CLEAN"\nexit "$status"\n')
            wrapper.chmod(0o755)
            for name, body in {
                'ffprobe': 'pass',
                'journalctl': "print('Linux mock clean boot')",
                'vainfo': "print('VAProfileHEVCMain : VAEntrypointVLD\\nVAProfileVP9Profile0 : VAEntrypointVLD')",
                'ffmpeg': '''import json,os,sys
from pathlib import Path
args=sys.argv[1:]
if '-decoders' in args:
 print(' hevc_v4l2m2m decoder\\n vp9_v4l2m2m decoder');sys.exit(0)
with Path(os.environ['CALLS']).open('a') as f:f.write(json.dumps(args)+'\\n')
count=int(args[args.index('-frames:v')+1])
if '-hwaccel' in args:
 if '-xerror' not in args:sys.exit(91)
 if count==int(os.environ['FAIL_FRAMES']):sys.exit(9)
Path(args[-1]).write_text(''.join('0, 0, 0, 1, 16, '+('a'*32)+'\\n' for _ in range(count)))
''',
            }.items():
                path = root / 'bin' / name
                path.write_text('#!' + sys.executable + '\n' + body + '\n')
                path.chmod(0o755)
            result = subprocess.run([str(root / 'tools' / 'verify-codec-expansion.sh'), tmp],
                env={**os.environ, 'PATH': str(root / 'bin') + os.pathsep + os.environ['PATH'],
                     'CLEAN': CLEAN, 'CALLS': str(root / 'calls'), 'FAIL_FRAMES': str(frames),
                     'V4L2_VA_CODEC5_DIR': str(root / 'samples'),
                     'V4L2_VA_CODEC5_LOG_DIR': str(root / 'logs')},
                capture_output=True, text=True, timeout=10)
            calls = [json.loads(line) for line in (root / 'calls').read_text().splitlines()]
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertNotIn('codec_vp9=', result.stdout)
            self.assertEqual(len(calls), 2 if frames == 1 else 3)
            self.assertIn('status=9', result.stdout)
            self.assertIn(CLEAN, (root / 'logs' / ('hevc-1f.log' if frames == 1 else 'hevc-30.log')).read_text())

    def test_first_frame_failure_does_not_submit_longer_leg(self):
        self.check_failed_leg(1)

    def test_longer_leg_failure_does_not_open_next_codec(self):
        self.check_failed_leg(30)


if __name__ == '__main__':
    unittest.main()
