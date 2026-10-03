import importlib.util
from pathlib import Path
import unittest


spec = importlib.util.spec_from_file_location('memory_trace', Path(__file__).resolve().parents[1] / 'prepare-iris-buffer-trace.py')
m = importlib.util.module_from_spec(spec)
spec.loader.exec_module(m)


def buffer(time=10, base=4096, size=256):
    return (f'IRIS_MEMORY_BUFFER op=queue time={time} tid=2 session=3 codec=825251393 '
            f'type=7 index=0 base={base} size={size} offset=0 data=0 timestamp=0 attr=4')


def returned(entry=10, time=11, retval=0):
    return f'IRIS_MEMORY_RETURN op=queue time={time} tid=2 entry={entry} retval={retval}'


def trace(*events):
    return '\n'.join(['IRIS_MEMORY_READY version=1', *events, 'IRIS_MEMORY_STOPPED'])


class MemoryTraceTest(unittest.TestCase):
    def test_range_end_exclusive_and_failed_submission_not_attributed(self):
        content = trace(buffer(), returned())
        result = m.analyze(content, '', 4096+255, 30)
        self.assertEqual(result['matching_reported_ranges'][0]['fault_offset'], 255)
        self.assertTrue(result['matching_reported_ranges'][0]['submission_returned_zero'])
        self.assertEqual(m.analyze(content, '', 4352, 30)['matching_reported_ranges'], [])
        self.assertEqual(m.analyze(trace(buffer(), returned(retval=-5)), '', 4100, 30)['matching_reported_ranges'], [])

    def test_dma_free_and_reuse_keep_separate_observations(self):
        content = trace(buffer(), returned(), 'IRIS_MEMORY_FREE time=15 base=4096 size=256',
                        buffer(time=20), returned(entry=20, time=21))
        rows = m.analyze(content, '', 4100, 30)['matching_reported_ranges']
        self.assertEqual(len(rows), 2)
        self.assertTrue(rows[0]['exact_dma_free_after_submission'])
        self.assertFalse(rows[1]['exact_dma_free_after_submission'])

    def test_pending_call_is_ambiguous_and_later_submission_excluded(self):
        result = m.analyze(trace(buffer(), buffer(time=40), returned(entry=40, time=41)), '', 4100, 30)
        self.assertEqual(result['unreturned_calls'], 1)
        self.assertEqual(len(result['matching_reported_ranges']), 1)
        self.assertFalse(result['matching_reported_ranges'][0]['submission_returned_zero'])

    def test_truncation_loss_diagnostics_and_ambiguous_records_refused(self):
        good = trace(buffer(), returned())
        for content in [good.replace('IRIS_MEMORY_STOPPED', ''), trace(buffer(), 'IRIS_MEMORY_LIMIT'),
                        trace(buffer(), 'Lost 5 events'), trace(buffer(), returned(), returned()),
                        trace(returned()), trace(buffer(size=2**64)), trace(buffer(), returned(time=9)),
                        trace(buffer(), buffer()), trace(''),
                        trace('IRIS_MEMORY_FREE time=-1 base=4096 size=256'),
                        trace('IRIS_MEMORY_FREE time=1 base=18446744073709551615 size=256')]:
            with self.assertRaises(ValueError):
                m.analyze(content, '', 4100, 30)
        with self.assertRaises(ValueError):
            m.analyze(good, 'probe_read failed', 4100, 30)

    def test_generated_program_only_observes_and_binds_offsets(self):
        offsets = dict(zip(m.FIELDS, range(0, len(m.FIELDS)*8, 8)))
        program = m.program(offsets)
        self.assertIn('nsecs(monotonic)', program)
        self.assertIn('kprobe:dma_free_attrs /@devices[arg0]/', program)
        self.assertIn('interval:s:120', program)
        self.assertIn(f'*(uint64*)(arg1+{offsets["base"]})', program)
        for unsafe in ['override(', 'signal(', 'system(', 'poke(', 'write(']:
            self.assertNotIn(unsafe, program)


if __name__ == '__main__':
    unittest.main()
