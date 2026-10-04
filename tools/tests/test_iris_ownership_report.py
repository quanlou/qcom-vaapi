import importlib.util
import json
from pathlib import Path
import unittest


SPEC = importlib.util.spec_from_file_location(
    'ownership', Path(__file__).resolve().parents[1] / 'trace-iris-buffer-ownership.py')
ownership = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ownership)


class OwnershipReportTests(unittest.TestCase):
    def report(self, lines, stats='', kernel=''):
        return ownership.ownership_report('\n'.join(lines), stats, kernel, 'reviewed-build')

    def test_rebound_slot_preserves_relationships_without_identifiers(self):
        report = self.report([
            'private-process-91: submit: sid=987654 type=2 index=0 fd=123 cached=0xda000000 current=0xda000000',
            'private-process-91: complete: sid=987654 index=0 address=0xda000000 bytes=1024',
            'private-process-91: submit: sid=987654 type=2 index=0 cached=0xbe000000 current=0xbe000000',
            'private-process-91: complete: sid=987654 index=0 address=0xbe000000 bytes=1024',
        ])
        self.assertEqual(report['status'], 'submitted_addresses_match')
        self.assertEqual(report['slot_address_changes'], 1)
        self.assertEqual(report['slot_change_examples'][0]['previous'], 'buffer-1')
        self.assertEqual(report['slot_change_examples'][0]['current'], 'buffer-2')
        serialized = json.dumps(report)
        for private in ['987654', '0xda000000', '0xbe000000', 'private-process', 'fd=123']:
            self.assertNotIn(private, serialized)

    def test_stale_submission_and_completion_identify_previous_buffer(self):
        report = self.report([
            'x: submit: sid=42 type=2 index=0 cached=0xabc000 current=0xabc000',
            'x: complete: sid=42 index=0 address=0xabc000 bytes=1024',
            'x: submit: sid=42 type=2 index=0 cached=0xabc000 current=0xdef000',
            'x: complete: sid=42 index=0 address=0xabc000 bytes=1024',
        ])
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(report['mismatch_count'], 2)
        self.assertEqual(report['mismatches'][0]['submitted'], 'buffer-1')
        self.assertEqual(report['mismatches'][0]['attached'], 'buffer-2')
        self.assertEqual(report['mismatches'][1]['returned'], 'buffer-1')

    def test_faulted_fetch_and_kernel_fault_are_not_clean_results(self):
        report = self.report(['x: submit: sid=42 type=2 index=0 cached=0xabc000 current=(fault)'])
        self.assertEqual(report['status'], 'failed')
        self.assertEqual(report['mismatches'][0]['attached'], 'unavailable')
        report = self.report([], kernel='BUG: Bad page map private-process addr:0xffff1234')
        self.assertEqual(report['status'], 'failed')
        self.assertNotIn('private-process', json.dumps(report))
        self.assertNotIn('0xffff1234', json.dumps(report))

    def test_empty_and_lossy_traces_cannot_pass(self):
        self.assertEqual(self.report([])['status'], 'incomplete')
        lines = [
            'x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000',
            'x: complete: sid=42 index=0 address=0x1000 bytes=1024',
            'x: submit: sid=42 type=2 index=0 cached=0x2000 current=0x2000',
        ]
        self.assertEqual(self.report(lines, stats='overrun: 1')['status'], 'incomplete')

    def test_different_objects_at_same_address_are_distinguished(self):
        report = self.report([
            'x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000 object=0xffffabc1 mapping=0xffffdef1',
            'x: complete: sid=42 index=0 address=0x1000 bytes=1024',
            'x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000 object=0xffffabc2 mapping=0xffffdef2',
            'x: complete: sid=42 index=0 address=0x1000 bytes=1024',
        ])
        self.assertEqual(report['slot_address_changes'], 0)
        self.assertEqual(report['slot_object_changes'], 1)
        self.assertEqual(report['status'], 'submitted_addresses_match')
        self.assertEqual(report['same_address_object_changes'], 1)
        self.assertEqual(report['object_identity_coverage'], 'complete')
        example = report['object_change_examples'][0]
        self.assertEqual(example['previous_object'], 'dmabuf-1')
        self.assertEqual(example['current_object'], 'dmabuf-2')
        self.assertTrue(example['same_dma_address'])
        for pointer in ['0xffffabc1', '0xffffabc2', '0xffffdef1', '0xffffdef2']:
            self.assertNotIn(pointer, json.dumps(report))

    def test_same_object_requeue_is_not_a_replacement(self):
        line = 'x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000 object=0xffffabc1 mapping=0xffffdef1'
        report = self.report([line, line])
        self.assertEqual(report['slot_object_changes'], 0)
        self.assertEqual(report['object_labels_observed'], 1)

    def test_old_trace_does_not_claim_object_coverage(self):
        report = self.report(['x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000'])
        self.assertEqual(report['object_identity_coverage'], 'incomplete')
        self.assertEqual(report['submissions_with_object_identity'], 0)

    def test_empty_completions_are_accounted_for_separately(self):
        report = self.report([
            'x: submit: sid=42 type=2 index=0 cached=0x1000 current=0x1000',
            'x: complete: sid=42 index=0 address=0x1000 bytes=0',
            'x: submit: sid=42 type=2 index=0 cached=0x2000 current=0x2000',
            'x: complete: sid=42 index=0 address=0x2000 bytes=1024',
        ])
        self.assertEqual(report['all_completion_events'], 2)
        self.assertEqual(report['empty_completion_events'], 1)
        self.assertEqual(report['capture_completions'], 1)


if __name__ == '__main__':
    unittest.main()
