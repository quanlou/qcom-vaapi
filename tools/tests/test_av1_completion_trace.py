import importlib.util
from pathlib import Path
import unittest

SOURCE = Path(__file__).resolve().parents[1] / 'qualify-iris-av1-completion-trace.py'
SPEC = importlib.util.spec_from_file_location('av1_trace', SOURCE)
TRACE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(TRACE)


class TraceEvidenceTests(unittest.TestCase):
    def event(self, **changes):
        values = dict(session=7, index=1, size=100, timestamp=33, flags=0,
                      picture=64, no_output=0, corrupt=0, overflow=0, retval=0)
        values.update(changes)
        return 'AV1_RAW ' + ' '.join(f'{key}={value}' for key, value in values.items())

    def window(self, event):
        return 'AV1_TRACE_READY\n' + event + '\nAV1_TRACE_STOPPED\n'

    def test_hidden_raw_size_is_reported_without_claiming_pixels(self):
        report = TRACE.inspect_trace(self.window(self.event()))
        self.assertEqual(report['hidden_nonzero_size'], 1)
        self.assertNotIn('pixel_parity', report)

    def test_corrupt_overflow_and_rejected_completion_fail(self):
        for changes in ({'corrupt': 1}, {'overflow': 1}, {'retval': -22}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                TRACE.inspect_trace(self.window(self.event(**changes)))

    def test_no_hidden_or_incomplete_trace_fails(self):
        for text in (self.window(self.event(picture=4)), self.event(),
                     'AV1_TRACE_READY\nAV1_TRACE_STOPPED\n'):
            with self.subTest(text=text), self.assertRaises(ValueError):
                TRACE.inspect_trace(text)

    def test_each_codec_context_requires_complete_verified_control_pair(self):
        def pair(context):
            return (f'[{context}] IRIS_AV1_ORDER control=10029965 value=0 verified\n'
                    f'[{context}] IRIS_AV1_ORDER control=10029966 value=1 verified\n')
        a, b = pair('av1_v4l2m2m @ 0x1'), pair('av1_v4l2m2m @ 0x2')
        self.assertTrue(TRACE.controls_valid(a + b))
        self.assertFalse(TRACE.controls_valid(a + b.splitlines()[0] + '\n'))
        self.assertFalse(TRACE.controls_valid(a + b.replace(' verified', '')))
        self.assertFalse(TRACE.controls_valid(a + a))


class IdleSettlingTests(unittest.TestCase):
    def run_wait(self, outcomes, fault=False):
        self.elapsed = 0
        self.calls = 0
        expected = {'boot_id': 'boot', 'build_id': 'build'}
        owner = self
        class Cycle:
            def journal(self, *args): return 'fault' if fault else 'clean'
            def require_clean_messages(self, text):
                if text == 'fault': raise ValueError('kernel fault')
            def inspect(self, helper):
                owner.calls += 1
                value = outcomes[min(owner.calls - 1, len(outcomes) - 1)]
                if isinstance(value, Exception): raise value
                return value
        def sleep(seconds): self.elapsed += seconds
        return TRACE.wait_for_idle(Cycle(), None, expected, 'cursor', seconds=.3,
                                   clock=lambda: self.elapsed, sleep=sleep)

    def test_autosuspend_settles_without_decoder_retry(self):
        busy = ValueError('decoder is busy or not safely runtime-suspended')
        expected = {'boot_id': 'boot', 'build_id': 'build'}
        self.assertEqual(self.run_wait([busy, busy, expected]), expected)
        self.assertEqual(self.calls, 3)
        self.assertAlmostEqual(self.elapsed, .2)

    def test_permanent_busy_is_bounded_failure(self):
        with self.assertRaisesRegex(ValueError, 'deadline'):
            self.run_wait([ValueError('decoder is busy or not safely runtime-suspended')])
        self.assertAlmostEqual(self.elapsed, .3)

    def test_identity_and_kernel_faults_never_wait(self):
        for outcomes, fault in [([ValueError('boot/kernel/loaded/selected identity refused')], False),
                                ([{'boot_id': 'changed', 'build_id': 'build'}], False),
                                ([{'boot_id': 'boot', 'build_id': 'build'}], True)]:
            with self.subTest(fault=fault, outcomes=outcomes), self.assertRaises(ValueError):
                self.run_wait(outcomes, fault)
            self.assertEqual(self.elapsed, 0)


if __name__ == '__main__':
    unittest.main()
