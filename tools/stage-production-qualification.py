#!/usr/bin/env python3
"""Freeze current source and build an immutable, independently qualified release packet."""
import argparse
import hashlib
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_files(root):
    files = [root / "rust/Cargo.toml", root / "rust/Cargo.lock"]
    for relative in ("rust/src", "tools", "kernel"):
        files.extend(path for path in (root / relative).rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts)
    return {str(path.relative_to(root)): digest(path) for path in sorted(files)}


def validate_source(root):
    """Report invalid captured scripts before spending time on a release build."""
    for path in sorted((root / "tools").rglob("*")):
        if not path.is_file() or "__pycache__" in path.parts:
            continue
        if path.suffix == ".py":
            compile(path.read_bytes(), str(path), "exec")
        elif path.suffix == ".sh":
            subprocess.run(["bash", "-n", str(path)], check=True)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def executable(path, text):
    path.write_text(text)
    path.chmod(0o755)


def stage(root, output_root):
    root = root.resolve()
    output_root.mkdir(parents=True, exist_ok=True)
    p = Path(tempfile.mkdtemp(prefix="packet.", dir=output_root))
    before = source_files(root)
    for relative in before:
        destination = p / "source" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(root / relative, destination)
    if source_files(root) != before or source_files(p / "source") != before:
        raise SystemExit("final_packet=fail reason=source_changed_during_capture path=" + str(p))
    write_json(p / "source-sha256.json", before)
    validate_source(p / "source")
    (p / "scratch").mkdir()
    environment = {**os.environ, "TMPDIR": str(p / "scratch"),
                   "CARGO_TARGET_DIR": str(p / "cargo-target")}
    with (p / "build.log").open("w") as log:
        subprocess.run(["cargo", "build", "--manifest-path", str(p / "source/rust/Cargo.toml"),
                        "--locked", "--release"], env=environment, stdout=log,
                       stderr=subprocess.STDOUT, check=True)
    (p / "driver").mkdir()
    shutil.copy2(p / "cargo-target/release/libmsm_drv_video.so", p / "driver/msm_drv_video.so")
    sha = digest(p / "driver/msm_drv_video.so")
    write_json(p / "identity.json", {
        "source_root": str(root), "packet": str(p), "driver_sha256": sha,
        "source_files": before, "qualification": "pending; previous GL passes do not qualify this build",
    })
    shutil.copytree(p / "source", p / "harness")
    build_script="""#!/usr/bin/env bash
    set -euo pipefail
    packet_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
    out_dir="${1:?driver output directory required}"
    expected=EXPECTED_SHA
    actual="$(sha256sum "$packet_dir/driver/msm_drv_video.so")"
    [[ "${actual%% *}" == "$expected" ]] || { echo 'final_driver=fail reason=hash_changed'; exit 1; }
    mkdir -p "$out_dir"
    staged="$(mktemp "$out_dir/.msm_drv_video.so.XXXXXX")"
    trap 'rm -f "$staged"' EXIT
    cp "$packet_dir/driver/msm_drv_video.so" "$staged"
    staged_hash="$(sha256sum "$staged")"
    [[ "${staged_hash%% *}" == "$expected" ]] || { echo 'final_driver=fail reason=staged_hash_changed'; exit 1; }
    chmod 755 "$staged"
    mv -f "$staged" "$out_dir/msm_drv_video.so"
    echo "final_driver=pass sha256=$expected path=$out_dir/msm_drv_video.so"
    """.replace('EXPECTED_SHA',sha)
    executable(p / 'harness/tools/build-rust-driver.sh', build_script)
    write_json(p / 'harness-sha256.json', source_files(p / 'harness'))
    verify="""#!/usr/bin/env python3
import hashlib,json
from pathlib import Path
p=Path(__file__).resolve().parent
for manifest,sub in [('source-sha256.json','source'),('harness-sha256.json','harness')]:
    expected=json.loads((p/manifest).read_text())
    actual={str(path.relative_to(p/sub)):hashlib.sha256(path.read_bytes()).hexdigest()
            for path in sorted((p/sub).rglob('*'))
            if path.is_file() and '__pycache__' not in path.parts}
    if actual!=expected:
        raise SystemExit('final_identity=fail source_membership_or_content_changed='+sub)
want=json.loads((p/'identity.json').read_text())['driver_sha256']
if hashlib.sha256((p/'driver/msm_drv_video.so').read_bytes()).hexdigest()!=want:
    raise SystemExit('final_identity=fail changed=driver')
print('final_identity=pass driver_sha256='+want)
"""
    (p/'verify-identity.py').write_text(verify)
    header="""#!/usr/bin/env bash
    set -euo pipefail
    packet_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    python3 "$packet_dir/verify-identity.py"
    export TMPDIR="$packet_dir/scratch" CARGO_TARGET_DIR="$packet_dir/cargo-target"
    run_dir="$(mktemp -d "$packet_dir/RUN_LABEL.XXXXXX")"
    echo "Results: $run_dir"
    cp "$packet_dir/identity.json" "$packet_dir/source-sha256.json" "$packet_dir/harness-sha256.json" "$run_dir/"
    """
    gate=header.replace('RUN_LABEL','production')+"""V4L2_VA_PRODUCTION_DIR="$run_dir/results" timeout -k 10s 3600s bash "$packet_dir/harness/tools/verify-production.sh" "$run_dir/driver" 2>&1 | tee "$run_dir/qualification.log"
    python3 "$packet_dir/verify-identity.py"
    """
    executable(p / 'run-gate.sh', gate)
    lifecycle=header.replace('RUN_LABEL','lifecycle')+"""# Independent evidence only: cannot override a failed production gate.
    exec 9>/tmp/libva-v4l2-hardware.lock
    flock -n 9 || { echo 'lifecycle=fail reason=hardware_in_use'; exit 1; }
    export LIBVA_DRIVER_NAME=msm V4L2_VA_NATIVE_DECODER=h264_v4l2m2m V4L2_VA_STRICT=1
    export V4L2_VA_CHURN_DIR="$run_dir/churn" V4L2_VA_EOS_DIR="$run_dir/eos" V4L2_VA_SEEK_DIR="$run_dir/seek"
    "$packet_dir/harness/tools/build-rust-driver.sh" "$run_dir/driver"
    for probe in session-churn eos-drain seek-storm; do
      status=0
      timeout -k 10s 1200s "$packet_dir/harness/tools/capture-iris-kernel-log.sh" -- "$packet_dir/harness/tools/verify-$probe.sh" "$run_dir/driver" > "$run_dir/$probe.log" 2>&1 || status=$?
      if [[ "$status" != 0 ]]; then echo "lifecycle=fail probe=$probe status=$status log=$run_dir/$probe.log"; exit 1; fi
      if ! python3 "$packet_dir/harness/tools/check-playback-performance.py" kernel --log "$run_dir/$probe.log"; then
        echo "lifecycle=fail probe=$probe reason=kernel_errors_or_missing_observation"; exit 1
      fi
      python3 "$packet_dir/verify-identity.py"
      echo "lifecycle_probe=pass probe=$probe log=$run_dir/$probe.log"
    done
    """
    executable(p / 'run-lifecycle.sh', lifecycle)
    for script in ('run-gate.sh', 'run-lifecycle.sh', 'harness/tools/build-rust-driver.sh'):
        subprocess.run(['bash', '-n', str(p / script)], check=True)
    subprocess.run(['python3',str(p/'verify-identity.py')],check=True)
    print('final_packet=ready path=' + str(p))
    print('gate_command=bash ' + str(p / 'run-gate.sh'))
    print('independent_lifecycle_command=bash ' + str(p / 'run-lifecycle.sh'))
    return p


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output-root", type=Path, default=Path.home() / ".cache/libva-v4l2-qualification/final")
    args = parser.parse_args()
    stage(args.root, args.output_root)


if __name__ == "__main__":
    main()
