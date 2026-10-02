#!/usr/bin/env python3
"""Host regressions for privileged Iris probe output acceptance."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('iris_probe', ROOT / 'tools/check-iris-probe-output.py')
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class IrisProbeTests(unittest.TestCase):
    def check(self, actual, reference=None, expected=2):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'actual.md5'
            path.write_text(actual)
            baseline = None
            if reference is not None:
                baseline = Path(directory) / 'reference.md5'
                baseline.write_text(reference)
            return probe.check(path, expected, baseline)

    good = ('#format: frame checksums\n'
            '0,0,0,1,384,0123456789abcdef0123456789abcdef\n'
            '0,1,1,1,384,abcdef0123456789abcdef0123456789\n')

    def test_complete_native_and_driver_pixels_pass(self):
        self.assertEqual(self.check(self.good, self.good), 2)

    def test_header_only_success_cannot_pass(self):
        with self.assertRaisesRegex(ValueError, 'frames=0 expected=2'):
            self.check('#format: frame checksums\n')

    def test_partial_and_extra_frames_fail(self):
        for text in (self.good.splitlines()[1] + '\n', self.good + self.good):
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, 'frames='):
                self.check(text)

    def test_wrong_pixels_or_order_fail_parity(self):
        for text in (self.good.replace('abcdef0123456789abcdef0123456789', 'f' * 32),
                     '\n'.join(reversed(self.good.splitlines()))):
            with self.subTest(text=text), self.assertRaisesRegex(ValueError, 'parity mismatch'):
                self.check(text, self.good)

    def test_malformed_or_empty_records_fail(self):
        for text in ('0,0,bad\n', self.good.replace(',384,', ',0,'),
                     self.good.replace('0,0,0,1,', '0,x,0,1,'),
                     self.good.replace('0,0,0,1,', '1,0,0,1,')):
            with self.subTest(text=text), self.assertRaises(ValueError):
                self.check(text)

    def test_reference_count_mismatch_fails(self):
        with self.assertRaisesRegex(ValueError, 'parity mismatch'):
            self.check(self.good, self.good.splitlines()[1] + '\n')


if __name__ == '__main__':
    unittest.main()
