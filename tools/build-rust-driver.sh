#!/usr/bin/env sh
set -eu
repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
out_dir=${1:-"$repo_dir/build-rust"}
mkdir -p "$out_dir"
cargo build --manifest-path "$repo_dir/rust/Cargo.toml" --locked --release --target-dir "$repo_dir/rust/target"
# Replace the inode atomically: truncating an already-loaded shared object can
# crash clients still executing its mapped pages.
staged_driver=$(mktemp "$out_dir/.msm_drv_video.so.XXXXXX")
trap 'if [ -f "$staged_driver" ]; then unlink "$staged_driver"; fi' 0
trap 'exit 1' HUP INT TERM
cp "$repo_dir/rust/target/release/libmsm_drv_video.so" "$staged_driver"
chmod 755 "$staged_driver"
mv -f -- "$staged_driver" "$out_dir/msm_drv_video.so"
printf '%s\n' "$out_dir/msm_drv_video.so"
