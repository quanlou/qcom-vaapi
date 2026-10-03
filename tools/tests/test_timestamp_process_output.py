import importlib.util
import io
from pathlib import Path
import sys
import unittest


spec = importlib.util.spec_from_file_location(
    'timestamp_process', Path(__file__).resolve().parents[1] / 'timestamp-process-output.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class TimestampProcessTests(unittest.TestCase):
    def test_timestamps_preserve_lines_and_elapsed_gap(self):
        output = io.BytesIO()
        status = module.run([sys.executable, '-u', '-c',
                             'import time; print("first"); time.sleep(.03); print("second")'], output)
        self.assertEqual(status, 0)
        lines = output.getvalue().splitlines()
        self.assertEqual([line.split(b' ', 1)[1] for line in lines], [b'first', b'second'])
        times = [int(line.split(b' ', 1)[0].split(b'=')[1]) for line in lines]
        self.assertGreaterEqual(times[1] - times[0], 20_000_000)

    def test_failure_and_merged_stderr_stay_visible(self):
        output = io.BytesIO()
        status = module.run([sys.executable, '-c',
                             'import sys; print("failure", file=sys.stderr); sys.exit(7)'], output)
        self.assertEqual(status, 7)
        self.assertTrue(output.getvalue().endswith(b' failure\n'))

    def test_silent_child_does_not_invent_evidence(self):
        output = io.BytesIO()
        self.assertEqual(module.run([sys.executable, '-c', 'pass'], output), 0)
        self.assertEqual(output.getvalue(), b'')


if __name__ == '__main__':
    unittest.main()
