#!/usr/bin/env python3
"""Check an explicitly authorized recovered-session diagnostic baseline."""
import importlib.util
import json
from pathlib import Path
import sys

spec = importlib.util.spec_from_file_location("gpu_guard", Path(__file__).with_name("qualify-gpu-copy.py"))
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)
try:
    baseline = json.loads(Path(sys.argv[1]).read_text())
    guard.preflight(baseline["loaded_build_ids"], baseline["kernel"], baseline)
except Exception as error:
    print(json.dumps({"status": "stopped", "reason": str(error), "no_retry": True}))
    sys.exit(1)
