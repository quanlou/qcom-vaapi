#!/usr/bin/env python3
"""Bind release qualification to source, fixtures, and the staged driver."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import sys


def digest(path):
    with path.open("rb") as stream:
        result = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
        return result.hexdigest()


def source_files(root):
    paths = [root / "rust/Cargo.toml", root / "rust/Cargo.lock"]
    for directory in (root / "rust/src", root / "tools"):
        paths.extend(path for path in directory.rglob("*")
                     if path.is_file() and "__pycache__" not in path.parts)
    return sorted(paths)


def kernel_identity():
    module = Path('/sys/module/qcom_iris')
    return {
        'boot_id': Path('/proc/sys/kernel/random/boot_id').read_text().strip(),
        'iris_srcversion': (module / 'srcversion').read_text().strip() if module.exists() else None,
        'iris_build_id_note_sha256': digest(module / 'notes/.note.gnu.build-id') if module.exists() else None,
    }


def record(root, fixtures):
    fixture_roots = sorted(str(Path(path).resolve()) for path in fixtures)
    fixture_paths = set()
    for entry in fixture_roots:
        path = Path(entry)
        if path.is_dir():
            fixture_paths.update(child for child in path.rglob("*") if child.is_file())
        else:
            fixture_paths.add(path)
    return {"source": {str(path.relative_to(root)): digest(path)
                       for path in source_files(root)},
            "fixture_roots": fixture_roots,
            "fixtures": {str(path): digest(path) for path in sorted(fixture_paths)},
            "kernel": platform.release(), "machine": platform.machine(),
            "running_kernel": kernel_identity()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("snapshot", "check", "bind-driver"))
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--fixture", action="append", default=[])
    parser.add_argument("--driver", type=Path)
    args = parser.parse_args()
    try:
        if args.mode == "snapshot":
            # Exclusive creation prevents stale evidence from being overwritten.
            with args.manifest.open("x") as stream:
                json.dump(record(args.root.resolve(), args.fixture), stream, indent=2)
        else:
            expected = json.loads(args.manifest.read_text())
            actual = record(args.root.resolve(), expected["fixture_roots"])
            if any(actual[key] != expected[key] for key in actual):
                raise ValueError("source_fixture_or_kernel_changed")
            if args.mode == "bind-driver":
                if "driver" in expected or args.driver is None:
                    raise ValueError("invalid_driver_binding")
                expected["driver"] = {"path": str(args.driver.resolve()),
                                      "sha256": digest(args.driver)}
                args.manifest.write_text(json.dumps(expected, indent=2) + "\n")
            elif "driver" in expected:
                if digest(Path(expected["driver"]["path"])) != expected["driver"]["sha256"]:
                    raise ValueError("driver_changed")
        print("production_provenance=pass mode=" + args.mode)
        return 0
    except (OSError, ValueError, KeyError) as error:
        print("production_provenance=fail reason=" + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
