# Iris timestamp metadata bounds fix

`0001-iris-wrap-timestamp-metadata-index.patch` is a candidate kernel fix,
not part of the Rust VA driver and not installed on this host.

During a 600-frame 4K H.264 VAAPI-copy run on `7.3.0-15-qcom-x1e`, UBSAN
reported `iris_buffer.c:869` and `:870`: index 32 exceeds `iris_ts_metadata[32]`.
The source explains the boundary: input metadata advances the next-write index
without wrapping until another input arrives; capture's unmatched-timestamp
fallback accesses that index directly. Wrap immediately after incrementing.
The patch preserves the existing next-slot fallback behavior. Whether that
fallback selects the appropriate metadata is a separate unresolved question.

Sources inspected on 2026-10-01:
[metadata writer](https://github.com/torvalds/linux/blob/master/drivers/media/platform/qcom/iris/iris_common.c),
[capture reader](https://github.com/torvalds/linux/blob/master/drivers/media/platform/qcom/iris/iris_buffer.c).

Test against a Linux source tree without modifying it:

```sh
python3 tools/verify-iris-metadata.py /path/to/linux
```

The runner copies the source into a temporary directory, applies this exact
patch without fuzz, extracts the original metadata functions, and compiles them
with UBSAN and minimal stand-in kernel types. It requires the original code to
reproduce the index-32 fault, then checks 4,096 patched inputs with matching and
missing output timestamps. This isolates the bounds defect; it is not a kernel
build, concurrency test, or proof of hardware behavior.

To prepare the change in the matching distro kernel source tree:

```sh
git -C /path/to/linux apply --check /path/to/libva-v4l2/kernel/0001-iris-wrap-timestamp-metadata-index.patch
git -C /path/to/linux apply /path/to/libva-v4l2/kernel/0001-iris-wrap-timestamp-metadata-index.patch
```

Build and boot that kernel using the distro's kernel workflow. Afterward rerun
the full decode/lifecycle matrix and the 600-frame 4K test with kernel logging.
This environment has no noninteractive sudo access, so kernel installation and
boot verification remain pending. A later warning-free run on the old kernel
does not close this defect: UBSAN reporting can suppress repeated occurrences.
