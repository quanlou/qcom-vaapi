import importlib.util
from pathlib import Path
import unittest
import tempfile
import json

ROOT = Path(__file__).resolve().parents[1]

def load(name):
    spec = importlib.util.spec_from_file_location(name, ROOT/(name+'.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

HOST = load('verify-av1-visible-reference-host')
NATIVE = load('qualify-av1-visible-reference')
VA = load('qualify-av1-va-transport')


class VisibleReferenceTests(unittest.TestCase):
    def test_complete_buffer_pixels_require_all_coded_and_original_display_order(self):
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory)
            (packet/'va.md5').write_text('hidden\nshown\n')
            (packet/'coded-software.md5').write_text('hidden\nshown\n')
            (packet/'original-software.md5').write_text('shown\nhidden\n')
            (packet/'display-indices.json').write_text(json.dumps([1, 0]))
            plan = {'mode': 'va_replay', 'decoded_frames': 2, 'displayed_frames': 2}
            rows = lambda p: p.read_text().splitlines()
            self.assertEqual(VA.verify_pixels(packet, packet, plan, rows), 2)
            for indices in ([0, 1], [1, 2], [1, True]):
                (packet/'display-indices.json').write_text(json.dumps(indices))
                with self.subTest(indices=indices), self.assertRaises(ValueError):
                    VA.verify_pixels(packet, packet, plan, rows)
            (packet/'display-indices.json').write_text(json.dumps([1, 0]))
            (packet/'coded-software.md5').write_text('wrong\nshown\n')
            with self.assertRaises(ValueError):
                VA.verify_pixels(packet, packet, plan, rows)

    def test_complete_buffer_replay_is_an_explicit_mode(self):
        self.assertEqual(VA.va_command(Path('/packet'), Path('/evidence'), {'mode':'va_replay'}),
                         ['/packet/producer/replay', '/dev/dri/renderD128',
                          '/packet/original-captures.bin', '/evidence/va.md5'])
        with self.assertRaises(ValueError):
            VA.va_command(Path('/packet'), Path('/evidence'),
                          {'mode':'va_replay', 'input_seek_seconds':6})

    def test_va_and_software_seek_use_same_input_boundary(self):
        plan = {'input_seek_seconds': 6}
        for command in [VA.va_command(Path('/packet'), Path('/evidence'), plan),
                        VA.software_reference_command(Path('/packet'), plan)]:
            self.assertEqual(command[command.index('-ss')+1], '6')
            self.assertLess(command.index('-ss'), command.index('-i'))
        for bad in [0, -1, 3601, float('nan'), True, '6']:
            with self.subTest(bad=bad), self.assertRaises(ValueError):
                VA.input_seek_args({'input_seek_seconds': bad})

    def test_va_reference_requires_matching_explicit_pixel_layout(self):
        command = VA.software_reference_command(Path('/packet'))
        self.assertEqual(command[command.index('-vf')+1], 'format=nv12')
        with tempfile.TemporaryDirectory() as directory:
            packet = Path(directory)
            (packet/'software-reference.log').write_text('rawvideo (I420')
            with self.assertRaises(ValueError):
                VA.require_reference_format(packet, {'reference_format': 'nv12'})
            (packet/'software-reference.log').write_text('Video: rawvideo (NV12 / format), nv12(progressive)')
            with self.assertRaises(ValueError):
                VA.require_reference_format(packet, {})
            VA.require_reference_format(packet, {'reference_format': 'nv12'})

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

    def test_paired_va_command_forces_hardware_frames_and_original_input(self):
        command = VA.va_command(Path('/packet'), Path('/evidence'))
        self.assertEqual(command[command.index('-hwaccel')+1], 'vaapi')
        self.assertEqual(command[command.index('-hwaccel_output_format')+1], 'vaapi')
        self.assertIn('-nofind_stream_info', command[:command.index('-i')])
        self.assertEqual(command[command.index('-i')+1], '/packet/sample.ivf')
        self.assertEqual(command[command.index('-vf')+1], 'hwdownload,format=nv12')
        self.assertEqual(command[command.index('-fps_mode')+1], 'passthrough')
        self.assertNotIn('-frames:v', command)

    def test_native_command_preserves_all_frames_and_avoids_probe_decode(self):
        command = NATIVE.native_command(Path('/packet'), Path('/evidence'))
        self.assertIn('-nofind_stream_info', command[:command.index('-i')])
        self.assertEqual(command[command.index('-fps_mode')+1], 'passthrough')
        self.assertEqual(command[command.index('-i')+1], '/packet/coded-frames.ivf')
        self.assertNotIn('-r', command)


if __name__ == '__main__':
    unittest.main()
