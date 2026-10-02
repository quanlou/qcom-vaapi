#!/usr/bin/env bash
# Host-only defensive and concurrent tests in an isolated source copy.
# Includes regression assertions that deliberately fail while defects remain.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
work_dir="${1:-$(mktemp -d /tmp/libva-host-stress.XXXXXX)}"
mkdir -p "$work_dir"
python3 - "$repo_root" "$work_dir" <<'PYTHON'
import pathlib, shutil, sys
root, dest = map(pathlib.Path, sys.argv[1:])
if (dest / 'rust').exists():
    raise SystemExit('destination already contains rust/: choose a fresh directory')
shutil.copytree(root / 'rust', dest / 'rust', ignore=shutil.ignore_patterns('target'))
shutil.copyfile(root / 'tools/host-stress.rs', dest / 'rust/src/stress_tests.rs')
lib = dest / 'rust/src/lib.rs'
lib.write_text(lib.read_text() + '\n#[cfg(test)]\nmod stress_tests;\n')
PYTHON
printf 'host_stress_artifacts=%s\n' "$work_dir"
set +e
cargo test --locked --manifest-path "$work_dir/rust/Cargo.toml" stress_tests:: -- --test-threads=4 > "$work_dir/stress.log" 2>&1
stress_status=$?
cargo test --locked --manifest-path "$work_dir/rust/Cargo.toml" -- --test-threads=16 > "$work_dir/parallel-suite.log" 2>&1
suite_status=$?
set -e
cat "$work_dir/stress.log"
tail -n 25 "$work_dir/parallel-suite.log"
printf 'host_stress_status=%s parallel_suite_status=%s\n' "$stress_status" "$suite_status"
if [[ "$stress_status" -ne 0 || "$suite_status" -ne 0 ]]; then
    exit 1
fi
