"""Release packet identity, immutable staging and lifecycle evidence regressions."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("stage_packet", ROOT / "tools/stage-production-qualification.py")
STAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(STAGE)


class StageQualificationTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.root = self.base / "workspace"
        for sub in ("rust/src", "tools", "kernel"):
            (self.root / sub).mkdir(parents=True)
        for name in ("Cargo.toml", "Cargo.lock", "src/lib.rs"):
            (self.root / "rust" / name).write_text("original")
        (self.root / "tools/build-rust-driver.sh").write_text("original builder")
        shutil.copy2(ROOT / "tools/check-playback-performance.py", self.root / "tools/check-playback-performance.py")
        self.builds = 0
        self.syntax_checked = []
        run = subprocess.run

        def build_once(command, **kwargs):
            if command[0] == "cargo":
                self.builds += 1
                self.assertIn("--locked", command)
                self.assertIn("--release", command)
                target = Path(kwargs["env"]["CARGO_TARGET_DIR"]) / "release"
                target.mkdir(parents=True)
                (target / "libmsm_drv_video.so").write_bytes(b"immutable driver")
                return subprocess.CompletedProcess(command, 0)
            if command[:2] == ["bash", "-n"]:
                self.assertEqual(len(command), 3)
                self.syntax_checked.append(Path(command[2]).name)
            return run(command, **kwargs)

        with patch.object(STAGE.subprocess, "run", side_effect=build_once), contextlib.redirect_stdout(io.StringIO()):
            self.packet = STAGE.stage(self.root, self.base / "packets")

    def identity(self):
        return subprocess.run([sys.executable, str(self.packet / "verify-identity.py")],
                              capture_output=True, text=True, timeout=10)

    def test_build_once_and_stage_same_binary_without_rebuild(self):
        self.assertEqual(self.builds, 1)
        self.assertCountEqual(self.syntax_checked[-3:], ["run-gate.sh", "run-lifecycle.sh", "build-rust-driver.sh"])
        self.assertEqual(self.identity().returncode, 0)
        for index in range(2):
            target = self.base / f"stage{index}"
            subprocess.run(["bash", str(self.packet / "harness/tools/build-rust-driver.sh"), str(target)],
                           check=True, capture_output=True, timeout=10)
            self.assertEqual((target / "msm_drv_video.so").read_bytes(), b"immutable driver")
        self.assertEqual(self.builds, 1)
        self.assertIn("qualification", json.loads((self.packet / "identity.json").read_text()))

    def test_added_deleted_and_changed_source_or_harness_files_fail(self):
        for tree in ("source", "harness"):
            with self.subTest(tree=tree, mutation="added"):
                extra = self.packet / tree / "new-file.txt"
                extra.write_text("new input")
                self.assertNotEqual(self.identity().returncode, 0)
                extra.unlink()
            path = self.packet / tree / "rust/src/lib.rs"
            original = path.read_bytes()
            for mutation in ("deleted", "changed"):
                with self.subTest(tree=tree, mutation=mutation):
                    if mutation == "deleted":
                        path.unlink()
                    else:
                        path.write_bytes(b"new implementation")
                    self.assertNotEqual(self.identity().returncode, 0)
                    path.write_bytes(original)
            self.assertEqual(self.identity().returncode, 0)

    def test_python_cache_does_not_change_identity(self):
        for tree in ("source", "harness"):
            cache = self.packet / tree / "tools/__pycache__"
            cache.mkdir()
            (cache / "generated.pyc").write_bytes(b"generated")
        self.assertEqual(self.identity().returncode, 0)

    def test_changed_driver_fails_identity_and_staging(self):
        (self.packet / "driver/msm_drv_video.so").write_bytes(b"other build")
        self.assertNotEqual(self.identity().returncode, 0)
        result = subprocess.run(["bash", str(self.packet / "harness/tools/build-rust-driver.sh"), str(self.base / "rejected")],
                                capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("hash_changed", result.stdout)
        self.assertFalse((self.base / "rejected/msm_drv_video.so").exists())

    def test_live_workspace_changes_do_not_change_frozen_packet(self):
        (self.root / "rust/src/lib.rs").write_text("subsequent edit")
        self.assertEqual(self.identity().returncode, 0)
        self.assertEqual((self.packet / "source/rust/src/lib.rs").read_text(), "original")

    def test_captured_invalid_python_or_shell_fails_before_build(self):
        for suffix, content, error in (("py", "def broken(\n", SyntaxError),
                                       ("sh", "if then\n", subprocess.CalledProcessError)):
            with self.subTest(suffix=suffix):
                broken = self.root / ("tools/broken." + suffix)
                broken.write_text(content)
                with self.assertRaises(error), patch.object(STAGE.subprocess, "run", wraps=subprocess.run) as run:
                    STAGE.stage(self.root, self.base / "invalid-packets")
                self.assertFalse(any(call.args[0][0] == "cargo" for call in run.call_args_list))
                broken.unlink()

    def test_lifecycle_requires_complete_clean_kernel_summary_before_next_probe(self):
        # Isolated stub commands: no hardware, kernel journal or shared lease.
        wrapper = self.packet / "harness/tools/capture-iris-kernel-log.sh"
        wrapper.write_text("#!/bin/sh\necho 'summary: session-fatal(0x4000003)=0'\n")
        wrapper.chmod(0o755)
        runner = self.packet / "run-lifecycle.sh"
        runner.write_text(runner.read_text().replace("/tmp/libva-v4l2-hardware.lock", str(self.base / "local.lock")))
        manifest = {rel: STAGE.digest(self.packet / "harness" / rel)
                    for rel in json.loads((self.packet / "harness-sha256.json").read_text())}
        manifest["tools/capture-iris-kernel-log.sh"] = STAGE.digest(wrapper)
        (self.packet / "harness-sha256.json").write_text(json.dumps(manifest))
        result = subprocess.run(["bash", str(runner)], capture_output=True, text=True, timeout=10)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("kernel_errors_or_missing_observation", result.stdout)
        self.assertNotIn("lifecycle_probe=pass", result.stdout)
        lifecycle = next(self.packet.glob("lifecycle.*"))
        self.assertFalse((lifecycle / "eos-drain.log").exists())


if __name__ == "__main__":
    unittest.main()
