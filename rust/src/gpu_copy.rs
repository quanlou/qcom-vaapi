//! Optional GPU byte-plane transfers. Reuse direct decode whenever possible;
//! this handles linear layouts that Iris itself cannot address. Publication
//! waits a GPU fence before CAPTURE recycling. No CPU readback occurs here.

use crate::pixel_format::DecodedFormat;
use std::os::fd::RawFd;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub(crate) struct Plane {
    pub offset: u32,
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct Request {
    source_size: u32,
    destination_size: u32,
    source: [Plane; 2],
    destination: [Plane; 2],
    clear_destination: u32,
    storage_stride: u32,
    storage_rows: u32,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Layout {
    pub size: u32,
    pub width: u32,
    pub height: u32,
    pub format: DecodedFormat,
    pub pitches: [u32; 2],
    pub offsets: [u32; 2],
    /// Owned surfaces zero padding/tail. Caller imports retain guard bytes.
    pub owned_storage: Option<(u32, u32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Completed,
    Unavailable,
    /// GPU submitted work or thread binding was lost: never CPU-fallback.
    Failed,
}

impl Layout {
    fn planes(self) -> Option<[Plane; 2]> {
        if self.width == 0 || self.height == 0 || self.size == 0 {
            return None;
        }
        let widths = [self.width, self.width.div_ceil(2).checked_mul(2)?];
        let rows = [self.height, self.height.div_ceil(2)];
        let mut planes = [Plane::default(); 2];
        let mut ends = [0; 2];
        for i in 0..2 {
            let width = widths[i].checked_mul(self.format.bytes_per_sample())?;
            if width > self.pitches[i] {
                return None;
            }
            let end = self.offsets[i]
                .checked_add((rows[i] - 1).checked_mul(self.pitches[i])?)?
                .checked_add(width)?;
            if end > self.size || end > i32::MAX as u32 {
                return None;
            }
            ends[i] = end;
            planes[i] = Plane {
                offset: self.offsets[i],
                pitch: self.pitches[i],
                width,
                height: rows[i],
            };
        }
        if self.offsets[1] < ends[0] && self.offsets[0] < ends[1] {
            return None;
        }
        Some(planes)
    }
}

fn request(source: Layout, destination: Layout) -> Option<Request> {
    if source.format != destination.format
        || source.width < destination.width
        || source.height < destination.height
    {
        return None;
    }
    // Views cover only visible bytes, with independent pitches and offsets.
    let src = Layout {
        width: destination.width,
        height: destination.height,
        ..source
    };
    let (stride, rows) = destination.owned_storage.unwrap_or_default();
    if destination.owned_storage.is_some()
        && (stride == 0 || rows == 0 || stride.checked_mul(rows)? > destination.size)
    {
        return None;
    }
    Some(Request {
        source_size: source.size,
        destination_size: destination.size,
        source: src.planes()?,
        destination: destination.planes()?,
        clear_destination: u32::from(destination.owned_storage.is_some()),
        storage_stride: stride,
        storage_rows: rows,
    })
}

#[cfg(feature = "gpu-copy")]
mod backend {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::os::fd::{AsRawFd, BorrowedFd};
    use std::os::unix::fs::MetadataExt;
    use std::sync::{Mutex, OnceLock};

    unsafe extern "C" {
        fn qcom_gpu_create(drm_fd: i32) -> *mut c_void;
        fn qcom_gpu_copy(
            context: *mut c_void,
            source: i32,
            destination: i32,
            request: *const Request,
        ) -> i32;
    }
    struct Context(*mut c_void);
    // All EGL calls are serialized; each call restores its caller's binding.
    // Contexts remain process-owned, retaining imports on an uncertain timeout.
    unsafe impl Send for Context {}
    static CONTEXTS: OnceLock<Mutex<HashMap<u64, Option<Context>>>> = OnceLock::new();

    pub(super) fn copy(
        drm_fd: RawFd,
        source: RawFd,
        destination: RawFd,
        request: &Request,
    ) -> Outcome {
        let Ok(fd) = (unsafe { BorrowedFd::borrow_raw(drm_fd) }).try_clone_to_owned() else {
            return Outcome::Unavailable;
        };
        let file = std::fs::File::from(fd);
        let Ok(metadata) = file.metadata() else {
            return Outcome::Unavailable;
        };
        let Ok(mut contexts) = CONTEXTS.get_or_init(|| Mutex::new(HashMap::new())).lock() else {
            return Outcome::Failed;
        };
        let context = contexts.entry(metadata.rdev()).or_insert_with(|| {
            let ptr = unsafe { qcom_gpu_create(file.as_raw_fd()) };
            (!ptr.is_null()).then_some(Context(ptr))
        });
        let Some(context) = context else {
            return Outcome::Unavailable;
        };
        match unsafe { qcom_gpu_copy(context.0, source, destination, request) } {
            0 => Outcome::Completed,
            1 => Outcome::Unavailable,
            _ => Outcome::Failed,
        }
    }
}

pub(crate) fn copy(
    drm_fd: Option<RawFd>,
    source: RawFd,
    source_layout: Layout,
    destination: RawFd,
    destination_layout: Layout,
) -> Outcome {
    let Some(plan) = request(source_layout, destination_layout) else {
        return Outcome::Unavailable;
    };
    if source < 0 || destination < 0 {
        return Outcome::Unavailable;
    }
    #[cfg(feature = "gpu-copy")]
    if let Some(fd) = drm_fd.filter(|&fd| fd >= 0) {
        return backend::copy(fd, source, destination, &plan);
    }
    let _ = (drm_fd, plan);
    Outcome::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(format: DecodedFormat) -> Layout {
        Layout {
            size: 8192,
            width: 17,
            height: 17,
            format,
            pitches: [128, 64],
            offsets: [32, 4096],
            owned_storage: None,
        }
    }

    #[test]
    fn byte_views_preserve_nv12_p010_odd_chroma_prefixes_gaps_and_tail() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let l = layout(format);
            let p = request(l, l).unwrap();
            assert_eq!(
                p.destination[0],
                Plane {
                    offset: 32,
                    pitch: 128,
                    width: 17 * format.bytes_per_sample(),
                    height: 17
                }
            );
            assert_eq!(
                p.destination[1],
                Plane {
                    offset: 4096,
                    pitch: 64,
                    width: 18 * format.bytes_per_sample(),
                    height: 9
                }
            );
            assert_eq!(p.clear_destination, 0);
        }
    }

    #[test]
    fn rejects_bounds_overlap_wrong_format_and_undersized_source_before_gpu_access() {
        let l = layout(DecodedFormat::Nv12);
        for bad in [
            Layout { size: 4096, ..l },
            Layout {
                pitches: [16, 64],
                ..l
            },
            Layout {
                offsets: [32, 48],
                ..l
            },
            Layout {
                offsets: [u32::MAX, 4096],
                ..l
            },
            Layout { width: 0, ..l },
            Layout {
                height: u32::MAX,
                ..l
            },
        ] {
            assert!(request(l, bad).is_none());
        }
        assert!(request(l, layout(DecodedFormat::P010)).is_none());
        assert!(request(Layout { width: 16, ..l }, l).is_none());
    }

    #[cfg(feature = "gpu-copy")]
    #[test]
    #[ignore = "real GPU; requires tools/qualify-gpu-copy.py with a clean boot and shared lease"]
    fn gpu_dma_buf_byte_parity() {
        use crate::surface_backing::SurfaceBacking;
        use crate::surface_import::ImportLayout;
        use crate::va_drm::DrmPrimeLayout;
        use std::fs::File;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        unsafe extern "C" {
            fn flock(fd: i32, operation: i32) -> i32;
        }
        let inherited_fd = |name: &str| -> i32 {
            std::env::var(name)
                .expect("use the guarded GPU qualifier")
                .parse()
                .unwrap()
        };
        let lease_fd = inherited_fd("V4L2_VA_GPU_LEASE_FD");
        let drm_fd = inherited_fd("V4L2_VA_GPU_DRM_FD");
        assert_eq!(
            unsafe { flock(lease_fd, 2 | 4) },
            0,
            "exclusive inherited lease required"
        );
        assert_eq!(
            std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .unwrap()
                .trim(),
            std::env::var("V4L2_VA_GPU_BOOT_ID").unwrap()
        );
        let file_for = |backing: &SurfaceBacking| {
            let descriptor = backing.descriptor(DrmPrimeLayout::Composed).unwrap();
            File::from(unsafe { OwnedFd::from_raw_fd(descriptor.objects[0].fd) })
        };
        let check_bytes = |actual: &[u8], expected: &[u8], message: &str| {
            assert_eq!(actual.len(), expected.len(), "{message}: size mismatch");
            if let Some(i) = actual.iter().zip(expected).position(|(a, b)| a != b) {
                panic!(
                    "{message}: first mismatch at byte {i}: actual={} expected={}; mismatches={}",
                    actual[i],
                    expected[i],
                    actual.iter().zip(expected).filter(|(a, b)| a != b).count()
                );
            }
        };
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            for (width, height) in [(17u32, 17u32), (128, 64), (1920, 1080), (3840, 2160)] {
                let stride = width.div_ceil(128) * 128 * format.bytes_per_sample();
                let rows = height.div_ceil(32) * 32;
                let mut source =
                    SurfaceBacking::allocate_with_drm(width, height, format, Some(drm_fd)).unwrap();
                let pixels: Vec<u8> = (0..stride * rows * 3 / 2)
                    .map(|i| (i as u8).wrapping_mul(73).wrapping_add((i / stride) as u8))
                    .collect();
                source.copy_decoded(&pixels, stride, rows, format).unwrap();
                let source_file = file_for(&source);
                let mut reference =
                    SurfaceBacking::allocate_with_drm(width, height, format, Some(drm_fd)).unwrap();
                reference
                    .copy_decoded(&pixels, stride, rows, format)
                    .unwrap();
                let mut destination =
                    SurfaceBacking::allocate_with_drm(width, height, format, Some(drm_fd)).unwrap();
                destination.fill_storage_for_gpu_test(0xa5).unwrap();
                for _ in 0..3 {
                    assert!(
                        destination
                            .copy_dma_buf(
                                Some(drm_fd),
                                source_file.as_raw_fd(),
                                source.size() as u32,
                                stride,
                                rows,
                                format
                            )
                            .unwrap(),
                        "GPU transfer required; CPU fallback cannot pass parity"
                    );
                    check_bytes(
                        &destination.download().unwrap().data,
                        &reference.download().unwrap().data,
                        &format!("owned parity/padding: {format:?} {width}x{height}"),
                    );
                }
                // A caller import with unequal pitches, nonzero prefix,
                // inter-plane gap and tail must preserve all guard bytes.
                let pitch_y = stride + 128;
                let pitch_uv = stride + 256;
                let offsets = [128, 128 + pitch_y * height + 256];
                let required = offsets[1] + pitch_uv * height.div_ceil(2) + 256;
                let allocation_width = stride / format.bytes_per_sample();
                let allocation_rows =
                    ((required.div_ceil(stride).div_ceil(3) * 2).div_ceil(32) * 32).max(rows + 32);
                let mut allocation = SurfaceBacking::allocate_with_drm(
                    allocation_width,
                    allocation_rows,
                    format,
                    Some(drm_fd),
                )
                .unwrap();
                let imported_file = file_for(&allocation);
                let layout = ImportLayout {
                    width,
                    height,
                    format,
                    size: allocation.size() as u32,
                    pitches: [pitch_y, pitch_uv],
                    offsets,
                };
                assert!(required <= layout.size);
                let mut expected = vec![0xa5; allocation.size()];
                allocation.fill_storage_for_gpu_test(0xa5).unwrap();
                let mut imported =
                    SurfaceBacking::import(imported_file.try_clone().unwrap().into(), layout)
                        .unwrap();
                for (plane, count) in [height, height.div_ceil(2)].into_iter().enumerate() {
                    let bytes = if plane == 0 {
                        width
                    } else {
                        width.div_ceil(2) * 2
                    } * format.bytes_per_sample();
                    for row in 0..count {
                        let src = (plane as u32 * stride * rows + row * stride) as usize;
                        let dst = (offsets[plane] + row * layout.pitches[plane]) as usize;
                        expected[dst..dst + bytes as usize]
                            .copy_from_slice(&pixels[src..src + bytes as usize]);
                    }
                }
                for _ in 0..3 {
                    assert!(
                        imported
                            .copy_dma_buf(
                                Some(drm_fd),
                                source_file.as_raw_fd(),
                                source.size() as u32,
                                stride,
                                rows,
                                format
                            )
                            .unwrap(),
                        "GPU transfer required for caller imports"
                    );
                    let actual = imported.storage_for_gpu_test().unwrap();
                    check_bytes(
                        &actual,
                        &expected,
                        &format!("import parity/guards: {format:?} {width}x{height}"),
                    );
                }
                println!(
                    "gpu_parity format={format:?} dimensions={width}x{height} owned=3 imported=3 cpu_copy_bytes=0"
                );
            }
        }
    }
}
