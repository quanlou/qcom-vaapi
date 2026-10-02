import importlib.util
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]

def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT/(name+'.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

HOST = load('verify-av1-visible-reference-host')
NATIVE = load('qualify-av1-visible-reference')


class VisibleReferenceTests(unittest.TestCase):
    def test_hidden_reference_alias_retains_exact_generation(self):
        units = [dict(type=6, show_existing=0, show_frame=0, resolved_refresh=1),
                 dict(type=6, show_existing=0, show_frame=1, resolved_refresh=2),
                 dict(type=3, show_existing=1, show_slot=0, resolved_refresh=0)]
        self.assertEqual(HOST.project(units, ['hidden-pixels', 'shown-pixels']),
                         ['shown-pixels', 'hidden-pixels'])
        with self.assertRaises(ValueError):
            HOST.project(units, ['missing-coded-frame'])

    def test_missing_and_refreshing_aliases_are_refused(self):
        for refresh in (0, 255):
            with self.subTest(refresh=refresh), self.assertRaises(ValueError):
                HOST.project([dict(type=3, show_existing=1, show_slot=0,
                                   resolved_refresh=refresh)], [])

    def test_one_coded_picture_per_packet_preserves_payload(self):
        units = [dict(type=1, bytes=1), dict(type=6, bytes=2, show_existing=0, show_frame=1),
                 dict(type=2, bytes=1), dict(type=6, bytes=2, show_existing=0, show_frame=1)]
        self.assertEqual(HOST.coded_packets(b'SaaTbb', units), [b'Saa', b'Tbb'])

    def test_superseded_sequence_before_first_frame_is_not_transported(self):
        units = [dict(type=1, bytes=1), dict(type=1, bytes=1),
                 dict(type=6, bytes=2, show_existing=0, show_frame=1)]
        self.assertEqual(HOST.coded_packets(b'ONaa', units), [b'Naa'])

    def test_unmodeled_header_layout_and_truncation_fail(self):
        for body, units in [(b'aa', [dict(type=3, bytes=2)]),
                            (b'a', [dict(type=6, bytes=2, show_existing=0, show_frame=1)]),
                            (b'T', [dict(type=2, bytes=1)])]:
            with self.subTest(units=units), self.assertRaises(ValueError):
                HOST.coded_packets(body, units)

    def test_native_command_preserves_all_frames_and_avoids_probe_decode(self):
        command = NATIVE.native_command(Path('/packet'), Path('/evidence'))
        self.assertIn('-nofind_stream_info', command[:command.index('-i')])
        self.assertEqual(command[command.index('-fps_mode')+1], 'passthrough')
        self.assertEqual(command[command.index('-i')+1], '/packet/coded-frames.ivf')
        self.assertNotIn('-r', command)


if __name__ == '__main__':
    unittest.main()
