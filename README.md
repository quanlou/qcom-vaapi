# qcom-vaapi

Hardware video decoding for **Snapdragon X Elite (X1E80100)** on Linux.

## What it can do

| Capability | RC10 support |
| --- | --- |
| Hardware decoding | H.264, HEVC / Main10, VP9 Profile 0; experimental AV1 |
| Zero-copy frame output | H.264 / HEVC with compatible DMA-BUF layouts |
| GPU frame transfers | Avoid CPU pixel copies for supported NV12 / P010 layouts |
| Resolution | Up to 4K; experimental 8K |
| Apps | VA-API integration for Chrome, Firefox, FFmpeg and GStreamer |

GPU transfers still move pixels. Unsupported layouts and CPU image requests
can require CPU copies. Apps must enable hardware decoding.
AV1 supports 8-bit Profile 0 without film grain; ordinary FFmpeg AV1 inputs
can still fail. [Copy paths](docs/gpu-copy.md).

## Current status

**RC10 is a development build**, not yet released.
Full codec / buffer-reuse checks remain pending. VP9 session reuse and Iris
faults remain unresolved. Sustained 4K60 / 8K30, 8K60, Firefox 8K and
suspend/resume reliability are unverified. [RC10 details](docs/releases/0.1.1-rc.10.md).

The published package is **[RC6](https://github.com/quanlou/qcom-vaapi/releases/tag/v0.1.1-rc.6)**;
RC10's GPU transfers and experimental 8K require building from `main`.

## Install

Requires ARM64 / X1E80100, compatible Iris kernel and firmware,
`libc6 >= 2.44` and `libva2 >= 2.24`. Tested on Ubuntu 26.10 development with
patched `7.3.0-15-qcom-x1e`; other chips are untested.

Download the RC6 `.deb` from the release above, then:

```sh
sudo apt install ./qcom-vaapi_0.1.1.rc.6_arm64.deb
vainfo
```

Restart video apps after installation. The package installs userspace libraries;
kernel / firmware setup is separate. [Setup details](docs/releases/0.1.1-rc.6.md).
Remove with `sudo apt remove qcom-vaapi`.

## Build RC10

```sh
V4L2_VA_BUILD_FEATURES='system-av1 gpu-copy experimental-8k' \
  ./tools/build-rust-driver.sh /tmp/qcom-vaapi-rc10
```

AV1 needs the companion library; GPU transfers need Adreno, EGL, GLES 3.1 and GBM.
See [build, package and test instructions](docs/build.md).
