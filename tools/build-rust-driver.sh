#!/usr/bin/env sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out_dir=${1:-"$repo_dir/build-rust"}
mkdir -p "$out_dir"
cargo build --manifest-path "$repo_dir/rust/Cargo.toml" --release
cp "$repo_dir/rust/target/release/libmsm_drv_video.so" "$out_dir/msm_drv_video.so"
printf '%s\n' "$out_dir/msm_drv_video.so"
