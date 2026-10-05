#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
test_dir="$(mktemp -d)"
trap 'rm -rf -- "$test_dir"' EXIT
"${CC:-cc}" -std=c11 -Wall -Wextra -Werror -O2 \
    "$repo_root/rust/native/gpu_copy_test.c" -ldl -o "$test_dir/gpu-copy-test"
"$test_dir/gpu-copy-test"
