"""Exercise strict GL shell orchestration without VA hardware."""
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class GlProductionTests(unittest.TestCase):
    def test_strict_reference_matches_complete_pipeline_despite_prefix_setting(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            tools = work / 'tools'
            tools.mkdir()
            for name in ('verify-gl-roundtrip.sh', 'hardware-session.sh',
                         'check-playback-performance.py', 'gst_gl_roundtrip.py'):
                shutil.copy2(ROOT / 'tools' / name, tools / name)
            modules = work / 'modules'
            modules.write_text('')
            helper = tools / 'hardware-session.sh'
            helper.write_text(helper.read_text().replace('/proc/modules', str(modules)))
            wrapper = tools / 'capture-iris-kernel-log.sh'
            wrapper.write_text('#!/bin/bash\nstatus=0\n"${@:2}" || status=$?\n'
                               "echo 'summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0 "
                               "power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0'\n"
                               'exit "$status"\n')
            wrapper.chmod(0o755)
            sample = work / "sample.mp4"
            sample.touch()
            mock = work / "mock"
            mock.write_text("#!" + sys.executable + "\n" + '''
import os
from pathlib import Path
import sys
name = Path(sys.argv[0]).name
frames = b''.join(bytes([i]) * 384 for i in (1, 2, 3))
if name == 'ffprobe':
    print('16,16')
elif name == 'df':
    print('Avail\\n10G')
elif name == 'ffmpeg':
    Path(os.environ['MOCK_FFMPEG_ARGS']).write_text(' '.join(sys.argv))
    Path(sys.argv[-1]).write_bytes(frames)
elif name == 'gst-launch-1.0':
    path = next(a.split('=', 1)[1] for a in sys.argv if a.startswith('location=') and a.endswith('gl.raw'))
    Path(path).write_bytes(frames)
    print('msm_drv_video_rs: ExportSurfaceHandle succeeded')
''')
            mock.chmod(0o755)
            for name in ('ffmpeg', 'ffprobe', 'gst-launch-1.0', 'gst-inspect-1.0', 'df'):
                (work / name).symlink_to(mock)
            args_path = work / 'args'
            result = subprocess.run(['bash', str(tools / 'verify-gl-roundtrip.sh'), str(work)],
                env={**os.environ, 'PATH': str(work) + os.pathsep + os.environ['PATH'],
                     'V4L2_VA_SAMPLE': str(sample), 'V4L2_VA_GL_RT_DIR': str(work),
                     'V4L2_VA_GL_RT_FRAMES': '2', 'V4L2_VA_STRICT': '1',
                     'MOCK_FFMPEG_ARGS': str(args_path)},
                capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertNotIn('-frames:v', args_path.read_text())
            self.assertIn('ref_frames=3', result.stdout)


if __name__ == '__main__':
    unittest.main()
