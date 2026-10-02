# Documentation — learn the stack, then write your own driver

Welcome. This directory is a self-contained "book" about qcom-vaapi: a
Rust VA-API driver that turns standard video-decode requests into V4L2
requests for Qualcomm's **Iris** hardware decoder (the Snapdragon X1E80100 / X
Elite VPU).

It is written for someone who knows **nothing** about Linux drivers or
graphics. No prior kernel or multimedia knowledge is assumed.

## How to read this book

Read the chapters in order — each one assumes only the previous ones.

| Chapter | Question it answers |
|---|---|
| [01 · Background primer](01-primer.md) | What is video decoding? Why hardware? How does Linux talk to hardware at all? |
| [02 · The big picture](02-stack.md) | What is the full stack (app → libva → this driver → kernel iris → hardware)? How do the pieces find each other? |
| [03 · VA-API crash course](03-vaapi-tutorial.md) | What is the VA-API object model and the life of a decoded frame? |
| [04 · Tour of this code](04-code-tour.md) | How does this driver actually work, file by file, state machine by state machine? |
| [05 · Upstream & downstream](05-upstream-downstream.md) | Who do we depend on, who depends on us, where does this work "live" in the ecosystem? |
| [06 · Roadmap, explained](06-roadmap.md) | What is left to do, why, and what each milestone teaches? |
| [07 · Write your own driver](07-write-your-own-driver.md) | What is the reusable recipe to build a driver for a *different* device or API? |

Quick reference cards (cheat sheets, no reading order required):

- [Glossary](01-primer.md#6-glossary-the-jargon-decoder-ring)
- [V4L2 stateful decoder flow](02-stack.md#4-what-stateful-v4l2-m2m-means)
- [VA-API call → driver function map](03-vaapi-tutorial.md#5-what-a-driver-must-implement-the-vtable)
- [Build & test walkthrough](04-code-tour.md#7-build-test-and-debug)

## The one-paragraph summary

When you play a video, something must turn compressed bytes (H.264 etc.)
into pixels. A dedicated hardware block — the VPU — does this cheaply,
but every vendor's hardware is different. Linux solves this with a
standard kernel interface, **V4L2** (here, the `iris` driver exposes the
Qualcomm decoder at `/dev/video16`). Desktop apps, however, don't speak
V4L2; they speak **VA-API** (via the `libva` library). This repository is
the missing middle: a **translator** that speaks VA-API on one side and
V4L2 on the other, shipped as `msm_drv_video.so` and loaded by libva at
 runtime.

```plantuml
@startuml
skinparam defaultTextAlignment center
rectangle "App\n(FFmpeg, mpv, browser)" as app
rectangle "libva\n(VA-API client library)" as libva
rectangle "**this repo**\nmsm_drv_video.so\n(Rust translator)" as drv, #DDEEFF
rectangle "kernel: V4L2 core + iris driver" as v4l2
rectangle "Iris video hardware\n(VPU in the SoC)" as hw

app -> libva : VA-API calls
libva -> drv : driver vtable calls
drv -> v4l2 : ioctls on /dev/video16
v4l2 -> hw : commands / DMA
@enduml
```

## Reading the diagrams

All diagrams are [PlantUML](https://plantuml.com) sources inside
` ```plantuml ``` ` fences. To actually *see* them:

- VS Code: install the *PlantUML* extension (Alt+D previews).
- Command line: `plantuml file.md` won't work on markdown directly;
  use `plantuml -o out docs/*.md` with the markdown plugin, or copy a
  diagram into the online server: <https://www.plantuml.com/plantuml>.
- Many Git forges render PlantUML fenced blocks automatically.

## Where the code lives (quick map)

| Path | What it is | Detailed in |
|---|---|---|
| `rust/src/lib.rs` | VA-API entry point and shared driver helpers | [Chapter 4, §2–4](04-code-tour.md#2-the-ffi-entry-point) |
| `rust/src/vtable.rs` + `rust/src/vtable/unsupported.rs` | Vtable installation and typed unsupported callbacks | [Chapter 4, §2–4](04-code-tour.md#2-the-ffi-entry-point) |
| `rust/src/state.rs` | VA object tables, IDs, surface state machine | [Chapter 4, §3](04-code-tour.md#3-object-tables-and-the-surface-state-machine) |
| `rust/src/h264.rs` + `rust/src/h264/bitstream.rs` | Synthesizes H.264 SPS/PPS headers and writes Annex-B bitstream bytes | [Chapter 4, §5](04-code-tour.md#5-h264rs-the-header-synthesis-problem) |
| `rust/src/buffer.rs` + `rust/src/buffer/handles.rs` | VA buffer allocation/map lifecycle plus metadata and handle callbacks | [Chapter 4, §4](04-code-tour.md#4-the-submission-path-begin--render--end) |
| `rust/src/v4l2.rs` | The V4L2 session: queues, buffers, polling, drain, export | [Chapter 4, §6](04-code-tour.md#6-v4l2rs-driving-the-kernel-decoder) |
| `rust/src/va_drm.rs` | DRM PRIME descriptors for `vaExportSurfaceHandle` | [Chapter 6, phase 3](06-roadmap.md#3-the-phases) |
| `rust/src/bindings.rs` | Generated C bindings (bindgen) — don't edit by hand | [Chapter 4, §1](04-code-tour.md#1-file-map) |
| `rust/wrapper.h` | Headers bindgen reads to generate bindings | [Chapter 4, §1](04-code-tour.md#1-file-map) |
| `tools/build-rust-driver.sh` | Build + install into a libva driver directory | [Chapter 4, §7](04-code-tour.md#7-build-test-and-debug) |
| `ROADMAP.md` | The project's own milestone list | [Chapter 6](06-roadmap.md) |
