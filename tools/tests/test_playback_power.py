import importlib.util
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location('power', Path(__file__).parents[1] / 'playback-power-profile.py')
power = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(power)


class PowerPolicy(unittest.TestCase):
    def test_paused_and_closed_decoders_do_not_hold_a_vote(self):
        self.assertFalse(power.decoder_active('1', 'suspended\n'))
        self.assertFalse(power.decoder_active('0', 'active\n'))
        self.assertTrue(power.decoder_active('1', 'active\n'))

    def test_original_floor_restored_and_higher_floor_preserved(self):
        with tempfile.TemporaryDirectory() as folder:
            gpu = Path(folder)
            (gpu / 'available_frequencies').write_text('300000000 550000000 925000000')
            for initial in (300_000_000, 925_000_000):
                (gpu / 'min_freq').write_text(str(initial))
                profile = power.Profile(gpu)
                profile.update(True)
                self.assertEqual(int((gpu / 'min_freq').read_text()), max(initial, power.FLOOR))
                profile.update(False)
                self.assertEqual(int((gpu / 'min_freq').read_text()), initial)
                profile.update(True)
                profile.restore()
                self.assertEqual(int((gpu / 'min_freq').read_text()), initial)

    def test_unknown_frequency_fails_before_writing(self):
        with tempfile.TemporaryDirectory() as folder:
            gpu = Path(folder)
            (gpu / 'available_frequencies').write_text('300000000 900000000')
            (gpu / 'min_freq').write_text('300000000')
            with self.assertRaises(ValueError):
                power.Profile(gpu)
            self.assertEqual((gpu / 'min_freq').read_text(), '300000000')
