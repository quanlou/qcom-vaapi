"""Host regressions for release identity and provenance rejection."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class ProvenanceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / "source"
        (self.root / "rust/src").mkdir(parents=True)
        (self.root / "tools").mkdir()
        for name in ("Cargo.toml", "Cargo.lock", "src/lib.rs"):
            (self.root / "rust" / name).write_text("original")
        self.fixture = Path(self.tmp.name) / "fixture.mp4"
        self.fixture.write_bytes(b"pixels")
        self.manifest = Path(self.tmp.name) / "manifest.json"
        self.driver = Path(self.tmp.name) / "msm_drv_video.so"
        self.driver.write_bytes(b"driver")
        self.assertEqual(self.run_check("snapshot", "--fixture", str(self.fixture)).returncode, 0)

    def run_check(self, mode, *extra):
        return subprocess.run([sys.executable, str(ROOT / "tools/production-provenance.py"),
                               mode, str(self.manifest), "--root", str(self.root), *extra],
                              capture_output=True, text=True, timeout=10)

    def test_unchanged_source_and_binary_pass(self):
        self.assertEqual(self.run_check("bind-driver", "--driver", str(self.driver)).returncode, 0)
        self.assertEqual(self.run_check("check").returncode, 0)

    def test_modified_source_rejected(self):
        (self.root / "rust/src/lib.rs").write_text("new implementation")
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_added_source_rejected(self):
        (self.root / "rust/src/new.rs").write_text("new implementation")
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_changed_fixture_rejected(self):
        self.fixture.write_bytes(b"different pixels")
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_replaced_binary_rejected(self):
        self.assertEqual(self.run_check("bind-driver", "--driver", str(self.driver)).returncode, 0)
        self.driver.write_bytes(b"other build")
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_duplicate_manifest_and_binary_binding_rejected(self):
        self.assertNotEqual(self.run_check("snapshot").returncode, 0)
        self.assertEqual(self.run_check("bind-driver", "--driver", str(self.driver)).returncode, 0)
        self.assertNotEqual(self.run_check("bind-driver", "--driver", str(self.driver)).returncode, 0)

    def test_missing_input_rejected(self):
        self.fixture.unlink()
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_codec_directory_additions_and_modifications_rejected(self):
        codec_dir = Path(self.tmp.name) / "codecs"
        codec_dir.mkdir()
        clip = codec_dir / "hevc.mp4"
        clip.write_bytes(b"codec pixels")
        self.manifest.unlink()
        self.assertEqual(self.run_check("snapshot", "--fixture", str(codec_dir)).returncode, 0)
        self.assertEqual(self.run_check("check").returncode, 0)
        clip.write_bytes(b"modified pixels")
        self.assertNotEqual(self.run_check("check").returncode, 0)
        clip.write_bytes(b"codec pixels")
        (codec_dir / "vp9.webm").write_bytes(b"new fixture")
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_python_cache_is_not_a_source_change(self):
        cache = self.root / "tools/__pycache__"
        cache.mkdir()
        (cache / "generated.pyc").write_bytes(b"cache")
        self.assertEqual(self.run_check("check").returncode, 0)

    def test_different_kernel_rejected(self):
        record = json.loads(self.manifest.read_text())
        record["kernel"] = "unqualified-kernel"
        self.manifest.write_text(json.dumps(record))
        self.assertNotEqual(self.run_check("check").returncode, 0)

    def test_reboot_or_module_replacement_with_same_release_rejected(self):
        original = self.manifest.read_text()
        for field in ('boot_id', 'iris_srcversion', 'iris_build_id_note_sha256'):
            with self.subTest(field=field):
                record = json.loads(original)
                record['running_kernel'][field] = 'another-boot-or-module'
                self.manifest.write_text(json.dumps(record))
                self.assertNotEqual(self.run_check('check').returncode, 0)


class ProductionGateTests(unittest.TestCase):
    def gate(self, directory, **environment):
        return subprocess.run(["bash", str(ROOT / "tools/verify-production.sh")],
                              env={**os.environ, "V4L2_VA_PRODUCTION_DIR": str(directory),
                                   **environment}, capture_output=True, text=True, timeout=10)

    def test_existing_results_cannot_be_reused(self):
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / "rust-driver.log").write_text("historical pass")
            result = self.gate(directory)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("results_directory_not_empty", result.stdout)

    def test_other_va_driver_cannot_be_qualified(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.gate(directory, LIBVA_DRIVER_NAME="other")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("incorrect_driver_or_native_decoder", result.stdout)

    def test_software_native_reference_cannot_be_qualified(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.gate(directory, V4L2_VA_NATIVE_DECODER="h264")
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("incorrect_driver_or_native_decoder", result.stdout)


if __name__ == "__main__":
    unittest.main()
