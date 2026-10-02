"""An incomplete kernel summary must not advance the production gate."""
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
CLEAN = ("summary: session-fatal(0x4000003)=0  system-fatal(0x5000003)=0  "
         "power-cycles=0  vb2-warns=0  other-session=0  other-system=0  kernel-bugs=0")


class ProductionKernelEvidenceTests(unittest.TestCase):
    def run_gate(self, summary):
        source = (ROOT / "tools/verify-production.sh").read_text()
        loop = source[source.index("for probe in rust-driver"):source.index("# This gate covers")]
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            tools = base / "tools"
            tools.mkdir()
            shutil.copy(ROOT / "tools/check-playback-performance.py", tools)
            (tools / "production-provenance.py").write_text("raise SystemExit(0)\n")
            wrapper = tools / "capture-iris-kernel-log.sh"
            wrapper.write_text("#!"+sys.executable+"\n"+'''import os,sys
from pathlib import Path
with Path(os.environ['CALLS']).open('a') as f: f.write(sys.argv[2]+'\\n')
print(os.environ['SUMMARY'])
''')
            wrapper.chmod(0o755)
            result = subprocess.run(["bash", "-c",
                'set -euo pipefail\nrepo_root="$ROOT"\nwork_dir="$ROOT"\n'
                'driver_dir="$ROOT/driver"\nprovenance="$ROOT/provenance"\n'+loop],
                env={**os.environ, "ROOT": tmp, "CALLS": str(base / "calls"), "SUMMARY": summary},
                capture_output=True, text=True, timeout=10)
            return result, len((base / "calls").read_text().splitlines())

    def test_all_clean_probes_advance(self):
        result, calls = self.run_gate(CLEAN)
        self.assertEqual(result.returncode, 0, result.stdout+result.stderr)
        self.assertEqual(calls, 4)

    def test_incomplete_or_malformed_observation_stops_next_probe(self):
        for summary in ("", "summary: session-fatal(0x4000003)=0",
                        CLEAN.replace("kernel-bugs=0", "kernel-bugs=unknown"),
                        CLEAN.replace("power-cycles=0", "power-cycles=1")):
            with self.subTest(summary=summary):
                result, calls = self.run_gate(summary)
                self.assertEqual(result.returncode, 1, result.stdout+result.stderr)
                self.assertEqual(calls, 1)
                self.assertIn("kernel_errors_or_missing_observation", result.stdout)


if __name__ == "__main__":
    unittest.main()
