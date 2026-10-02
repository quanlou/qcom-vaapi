"""Scoped qualification must record exclusions without changing default coverage."""
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location(
    'real_use', Path(__file__).resolve().parents[1] / 'run-real-use-qualification.py')
REAL_USE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REAL_USE)


class RealUseScopeTests(unittest.TestCase):
    def test_default_scope_keeps_both_browsers(self):
        scope = REAL_USE.browser_scope('all')
        self.assertEqual(scope['browsers'], ['chromium', 'firefox'])
        self.assertEqual(scope['deferred_unsupported'], [])

    def test_scoped_release_explicitly_records_unsupported_firefox(self):
        scope = REAL_USE.browser_scope('chromium-headless')
        self.assertEqual(scope['browsers'], ['chromium'])
        self.assertEqual(scope['deferred_unsupported'], ['firefox'])
        self.assertEqual(scope['codecs'], ['h264', 'hevc', 'vp9'])

    def test_unknown_scope_cannot_silently_skip_checks(self):
        with self.assertRaisesRegex(ValueError, 'unknown_qualification_scope'):
            REAL_USE.browser_scope('none')


if __name__ == '__main__':
    unittest.main()
