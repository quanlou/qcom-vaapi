# Build, package and test RC10

Use a recent Rust toolchain. GPU transfers require Adreno, EGL, GLES 3.1 and GBM.
AV1 requires `libiris_av1_complete.so`; follow the
[companion build instructions](releases/0.1.1-rc.5.md#build-the-av1-companion).

## Build

Keep the same features enabled when building and testing:

```sh
export V4L2_VA_BUILD_FEATURES='system-av1 gpu-copy experimental-8k'
./tools/build-rust-driver.sh /tmp/qcom-vaapi-rc10
```

Omit `experimental-8k` for the normal 4096-pixel maximum side. Omit `gpu-copy`
for the CPU fallback build. See [RC10 limits and current evidence](releases/0.1.1-rc.10.md).

## Package

Package the built libraries on ARM64:

```sh
python3 tools/package-deb.py \
  --driver /tmp/qcom-vaapi-rc10/msm_drv_video.so \
  --companion /path/to/libiris_av1_complete.so \
  --ffmpeg-source /path/to/ffmpeg-source \
  --output /tmp/qcom-vaapi-package
```

The package includes license notices and detected dependencies. GPU variants
also depend on EGL, GLES and GBM runtime libraries. Build on the oldest target
distribution and qualify the resulting binary on its actual hardware.
The package does not install firmware or activate kernel patches; its power
helper is disabled by default.

## Test

Close video apps and use a clean boot with the documented fixtures. Keep the
build features above enabled:

```sh
./tools/verify-rust-driver.sh /tmp/qcom-vaapi-rc10
./tools/verify-session-churn.sh /tmp/qcom-vaapi-rc10
```

Keep the required 1 / 30 / full-frame comparisons and export checks. Hardware
faults stop testing before another decoder session. The full deployment gate
is `tools/verify-production.sh`; GitHub Actions runs the host checks.
