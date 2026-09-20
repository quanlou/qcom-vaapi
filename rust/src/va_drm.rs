//! DRM PRIME descriptors used by `vaExportSurfaceHandle`.
//!
//! The build host's libva headers may omit `va_drmcommon.h`, but modern clients
//! still pass a `VADRMPRIMESurfaceDescriptor` pointer when `mem_type` is
//! `VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2`. These local `repr(C)` structs mirror
//! libva's ABI so the rest of the driver can work with typed Rust values.

use std::ffi::c_int;

use crate::bindings::{
    VA_EXPORT_SURFACE_COMPOSED_LAYERS, VA_EXPORT_SURFACE_READ_ONLY, VA_EXPORT_SURFACE_READ_WRITE,
    VA_EXPORT_SURFACE_SEPARATE_LAYERS, VA_EXPORT_SURFACE_WRITE_ONLY,
};
use crate::pixel_format::DecodedFormat;
use crate::v4l2::CaptureExport;

pub(crate) const VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2: u32 = 0x4000_0000;

pub(crate) fn exported_surface_memory_types(internal_va: u32) -> u32 {
    internal_va | VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2
}

const DRM_FORMAT_NV12: u32 = fourcc(b'N', b'V', b'1', b'2');
const DRM_FORMAT_P010: u32 = fourcc(b'P', b'0', b'1', b'0');
const DRM_FORMAT_R8: u32 = fourcc(b'R', b'8', b' ', b' ');
const DRM_FORMAT_GR88: u32 = fourcc(b'G', b'R', b'8', b'8');
const DRM_FORMAT_R16: u32 = fourcc(b'R', b'1', b'6', b' ');
const DRM_FORMAT_GR1616: u32 = fourcc(b'G', b'R', b'3', b'2');
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DrmPrimeLayout {
    Composed,
    Separate,
}

impl DrmPrimeLayout {
    pub(crate) fn from_export_flags(flags: u32) -> Result<Self, ()> {
        const ACCESS_MASK: u32 = VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_WRITE_ONLY;
        const SUPPORTED_FLAGS: u32 = VA_EXPORT_SURFACE_READ_WRITE
            | VA_EXPORT_SURFACE_SEPARATE_LAYERS
            | VA_EXPORT_SURFACE_COMPOSED_LAYERS;
        if flags & !SUPPORTED_FLAGS != 0 {
            return Err(());
        }
        if flags & ACCESS_MASK == VA_EXPORT_SURFACE_WRITE_ONLY {
            return Err(());
        }
        let separate = flags & VA_EXPORT_SURFACE_SEPARATE_LAYERS != 0;
        let composed = flags & VA_EXPORT_SURFACE_COMPOSED_LAYERS != 0;
        match (separate, composed) {
            (true, true) => Err(()),
            (true, false) => Ok(Self::Separate),
            _ => Ok(Self::Composed),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DrmPrimeObject {
    pub(crate) fd: c_int,
    pub(crate) size: u32,
    pub(crate) drm_format_modifier: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DrmPrimeLayer {
    pub(crate) drm_format: u32,
    pub(crate) num_planes: u32,
    pub(crate) object_index: [u32; 4],
    pub(crate) offset: [u32; 4],
    pub(crate) pitch: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct DrmPrimeDescriptor {
    pub(crate) fourcc: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) num_objects: u32,
    pub(crate) objects: [DrmPrimeObject; 4],
    pub(crate) num_layers: u32,
    pub(crate) layers: [DrmPrimeLayer; 4],
}

impl DrmPrimeDescriptor {
    pub(crate) fn from_capture(capture: CaptureExport, layout: DrmPrimeLayout) -> Self {
        let mut desc = Self {
            fourcc: capture.format.va_fourcc(),
            width: capture.width,
            height: capture.height,
            num_objects: 1,
            objects: [DrmPrimeObject::default(); 4],
            num_layers: 0,
            layers: [DrmPrimeLayer::default(); 4],
        };
        desc.objects[0] = DrmPrimeObject {
            fd: capture.fd,
            size: capture.size,
            drm_format_modifier: DRM_FORMAT_MOD_LINEAR,
        };

        match layout {
            DrmPrimeLayout::Composed => {
                desc.num_layers = 1;
                desc.layers[0] = DrmPrimeLayer {
                    drm_format: match capture.format {
                        DecodedFormat::Nv12 => DRM_FORMAT_NV12,
                        DecodedFormat::P010 => DRM_FORMAT_P010,
                    },
                    num_planes: 2,
                    object_index: [0, 0, 0, 0],
                    offset: [capture.y_offset, capture.uv_offset, 0, 0],
                    pitch: [capture.stride, capture.stride, 0, 0],
                };
            }
            DrmPrimeLayout::Separate => {
                desc.num_layers = 2;
                desc.layers[0] = DrmPrimeLayer {
                    drm_format: match capture.format {
                        DecodedFormat::Nv12 => DRM_FORMAT_R8,
                        DecodedFormat::P010 => DRM_FORMAT_R16,
                    },
                    num_planes: 1,
                    object_index: [0, 0, 0, 0],
                    offset: [capture.y_offset, 0, 0, 0],
                    pitch: [capture.stride, 0, 0, 0],
                };
                desc.layers[1] = DrmPrimeLayer {
                    drm_format: match capture.format {
                        DecodedFormat::Nv12 => DRM_FORMAT_GR88,
                        DecodedFormat::P010 => DRM_FORMAT_GR1616,
                    },
                    num_planes: 1,
                    object_index: [0, 0, 0, 0],
                    offset: [capture.uv_offset, 0, 0, 0],
                    pitch: [capture.stride, 0, 0, 0],
                };
            }
        }
        desc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capture() -> CaptureExport {
        CaptureExport {
            fd: 17,
            size: 1024,
            width: 16,
            height: 16,
            stride: 128,
            y_offset: 0,
            uv_offset: 2048,
            format: DecodedFormat::Nv12,
        }
    }

    #[test]
    fn defaults_to_composed_layout() {
        assert_eq!(
            DrmPrimeLayout::from_export_flags(0),
            Ok(DrmPrimeLayout::Composed)
        );
    }

    #[test]
    fn accepts_gstreamer_read_write_separate_flags() {
        assert_eq!(
            DrmPrimeLayout::from_export_flags(
                VA_EXPORT_SURFACE_READ_WRITE | VA_EXPORT_SURFACE_SEPARATE_LAYERS,
            ),
            Ok(DrmPrimeLayout::Separate)
        );
    }

    #[test]
    fn rejects_write_only_flags() {
        assert_eq!(
            DrmPrimeLayout::from_export_flags(VA_EXPORT_SURFACE_WRITE_ONLY),
            Err(())
        );
    }

    #[test]
    fn rejects_conflicting_layer_flags() {
        assert_eq!(
            DrmPrimeLayout::from_export_flags(
                VA_EXPORT_SURFACE_SEPARATE_LAYERS | VA_EXPORT_SURFACE_COMPOSED_LAYERS,
            ),
            Err(())
        );
    }

    #[test]
    fn builds_composed_nv12_descriptor() {
        let desc = DrmPrimeDescriptor::from_capture(capture(), DrmPrimeLayout::Composed);
        assert_eq!(desc.fourcc, DecodedFormat::Nv12.va_fourcc());
        assert_eq!(desc.num_objects, 1);
        assert_eq!(desc.objects[0].fd, 17);
        assert_eq!(desc.num_layers, 1);
        assert_eq!(desc.layers[0].drm_format, DRM_FORMAT_NV12);
        assert_eq!(desc.layers[0].num_planes, 2);
        assert_eq!(desc.layers[0].offset[1], 2048);
    }

    #[test]
    fn builds_separate_nv12_descriptor() {
        let desc = DrmPrimeDescriptor::from_capture(capture(), DrmPrimeLayout::Separate);
        assert_eq!(desc.num_layers, 2);
        assert_eq!(desc.layers[0].drm_format, DRM_FORMAT_R8);
        assert_eq!(desc.layers[1].drm_format, DRM_FORMAT_GR88);
        assert_eq!(desc.layers[1].offset[0], 2048);
    }

    #[test]
    fn builds_p010_descriptor() {
        let mut cap = capture();
        cap.format = DecodedFormat::P010;
        let desc = DrmPrimeDescriptor::from_capture(cap, DrmPrimeLayout::Separate);
        assert_eq!(desc.fourcc, DecodedFormat::P010.va_fourcc());
        assert_eq!(desc.layers[0].drm_format, DRM_FORMAT_R16);
        assert_eq!(desc.layers[1].drm_format, DRM_FORMAT_GR1616);
    }
}
