//! Driver-owned VA objects and handle decoding.
//!
//! libva passes integer IDs back to the driver. We reserve disjoint ID ranges for
//! each object kind and convert those IDs back to table indices before touching
//! driver state. Keeping this separate from the entrypoint module makes the FFI
//! boundary easier to audit.

use std::os::fd::OwnedFd;
use std::sync::Mutex;

use crate::bindings::*;
use crate::h264::{H264Slice, H264Synth};
use crate::v4l2::V4l2Session;

pub(crate) const DRV_ID_BASE_CONFIG: u32 = 0x0100_0000;
pub(crate) const DRV_ID_BASE_SURFACE: u32 = 0x4000_0000;
pub(crate) const DRV_ID_BASE_CONTEXT: u32 = 0x0200_0000;
pub(crate) const DRV_ID_BASE_BUFFER: u32 = 0x6000_0000;
pub(crate) const DRV_ID_BASE_IMAGE: u32 = 0x7000_0000;
pub(crate) const DRV_MAX_CONFIGS: usize = 16;
pub(crate) const DRV_MAX_CONTEXTS: usize = 16;
pub(crate) const DRV_MAX_SURFACES: usize = 1024;
pub(crate) const DRV_MAX_BUFFERS: usize = 4096;
pub(crate) const DRV_MAX_IMAGES: usize = 256;
pub(crate) const DRV_MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
pub(crate) const DRV_MAX_ATTRIBUTE_LIST: usize = 64;
pub(crate) const DRV_MAX_RENDER_BUFFERS: usize = 256;
pub(crate) const DRV_MAX_SURFACE_EXPORTS: usize = 64;
pub(crate) const DRV_MAX_SLICES_PER_FRAME: usize = 64;
pub(crate) const DRV_MIN_DIM: i32 = 16;
pub(crate) const DRV_MAX_DIM: i32 = 4096;

pub(crate) const SUPPORTED_PROFILES: [VAProfile; 3] = [
    VAProfile::VAProfileH264ConstrainedBaseline,
    VAProfile::VAProfileH264Main,
    VAProfile::VAProfileH264High,
];

pub(crate) const VENDOR: &[u8] =
    b"msm_drv_video_rs: Qualcomm Iris (X1E80100) stateful V4L2 M2M, H264 Rust rewrite MVP\0";

#[derive(Clone)]
pub(crate) struct Config {
    pub(crate) profile: VAProfile,
    pub(crate) entrypoint: VAEntrypoint,
    pub(crate) attribs: Vec<VAConfigAttrib>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SurfaceState {
    Empty,
    InProgress,
    Pending,
    Ready,
    Dead,
}

/// Pixel snapshot a published surface reads from. CAPTURE slots are recycled
/// by the decoder as soon as they are requeued, so a client that reads a
/// surface late must observe the frame as it was at dequeue time, not
/// whatever the recycled slot holds by then.
#[derive(Clone)]
pub(crate) struct SurfaceFrame {
    pub(crate) data: Vec<u8>,
    pub(crate) stride: u32,
    pub(crate) height: u32,
}

pub(crate) struct Surface {
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) state: SurfaceState,
    pub(crate) cap_idx: Option<usize>,
    /// Dequeued frame bytes backing the CPU-copy read path (vaGetImage /
    /// vaDeriveImage). Replaced on every publish.
    pub(crate) frame: Option<SurfaceFrame>,
    pub(crate) owner: VAContextID,
    pub(crate) exported: bool,
    pub(crate) export_count: u64,
    /// Owned dup() of every fd handed out by vaExportSurfaceHandle. The
    /// descriptor fd belongs to the client; our dup keeps the underlying
    /// dma-buf alive until the surface is retired, re-rendered, or dropped, and
    /// gives leak-safe accounting. Content may legally be overwritten from the
    /// moment the surface is re-used (VA-API export contract).
    pub(crate) export_fds: Vec<OwnedFd>,
}

pub(crate) struct Context {
    pub(crate) config_id: VAConfigID,
    pub(crate) profile: VAProfile,
    pub(crate) entrypoint: VAEntrypoint,
    pub(crate) width: i32,
    pub(crate) height: i32,
    pub(crate) render_targets: Vec<VASurfaceID>,
    pub(crate) frame_open: bool,
    pub(crate) render_target: VASurfaceID,
    pub(crate) slices: Vec<H264Slice>,
    pub(crate) syn: H264Synth,
    pub(crate) out_seq: u64,
    pub(crate) first_poc: Option<i32>,
    pub(crate) poc_epoch_usec: u64,
    pub(crate) max_timestamp_usec: u64,
    pub(crate) v4l2: Option<V4l2Session>,
}

#[derive(Clone)]
pub(crate) struct Buffer {
    pub(crate) owner: VAContextID,
    pub(crate) type_: VABufferType,
    pub(crate) elem_size: u32,
    pub(crate) num_elements: u32,
    pub(crate) data: Vec<u8>,
    pub(crate) mapped: bool,
}

#[derive(Clone)]
pub(crate) struct Image {
    pub(crate) image: VAImage,
}

pub(crate) struct DriverState {
    pub(crate) configs: Vec<Option<Config>>,
    pub(crate) contexts: Vec<Option<Context>>,
    pub(crate) surfaces: Vec<Option<Surface>>,
    pub(crate) buffers: Vec<Option<Buffer>>,
    pub(crate) images: Vec<Option<Image>>,
}

pub(crate) struct DriverBox {
    pub(crate) lock: Mutex<DriverState>,
}

fn empty_slots<T>(len: usize) -> Vec<Option<T>> {
    let mut v = Vec::with_capacity(len);
    v.resize_with(len, || None);
    v
}

impl DriverBox {
    pub(crate) fn new() -> Self {
        Self {
            lock: Mutex::new(DriverState {
                configs: empty_slots(DRV_MAX_CONFIGS),
                contexts: empty_slots(DRV_MAX_CONTEXTS),
                surfaces: empty_slots(DRV_MAX_SURFACES),
                buffers: empty_slots(DRV_MAX_BUFFERS),
                images: empty_slots(DRV_MAX_IMAGES),
            }),
        }
    }
}

fn object_index(id: u32, base: u32, capacity: usize) -> Option<usize> {
    id.checked_sub(base)
        .map(|idx| idx as usize)
        .filter(|&idx| idx < capacity)
}

pub(crate) fn config_index(id: VAConfigID) -> Option<usize> {
    object_index(id, DRV_ID_BASE_CONFIG, DRV_MAX_CONFIGS)
}

pub(crate) fn context_index(id: VAContextID) -> Option<usize> {
    object_index(id, DRV_ID_BASE_CONTEXT, DRV_MAX_CONTEXTS)
}

pub(crate) fn surface_index(id: VASurfaceID) -> Option<usize> {
    object_index(id, DRV_ID_BASE_SURFACE, DRV_MAX_SURFACES)
}

pub(crate) fn buffer_index(id: VABufferID) -> Option<usize> {
    object_index(id, DRV_ID_BASE_BUFFER, DRV_MAX_BUFFERS)
}

pub(crate) fn image_index(id: VAImageID) -> Option<usize> {
    object_index(id, DRV_ID_BASE_IMAGE, DRV_MAX_IMAGES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_ids_accept_only_their_reserved_ranges() {
        assert_eq!(config_index(DRV_ID_BASE_CONFIG), Some(0));
        assert_eq!(
            config_index(DRV_ID_BASE_CONFIG + DRV_MAX_CONFIGS as u32 - 1),
            Some(DRV_MAX_CONFIGS - 1)
        );
        assert_eq!(
            config_index(DRV_ID_BASE_CONFIG + DRV_MAX_CONFIGS as u32),
            None
        );
        assert_eq!(config_index(DRV_ID_BASE_CONFIG - 1), None);

        assert_eq!(surface_index(DRV_ID_BASE_SURFACE), Some(0));
        assert_eq!(context_index(DRV_ID_BASE_CONTEXT), Some(0));
        assert_eq!(buffer_index(DRV_ID_BASE_BUFFER), Some(0));
        assert_eq!(image_index(DRV_ID_BASE_IMAGE), Some(0));
    }

    #[test]
    fn object_id_ranges_do_not_accept_other_object_bases() {
        assert_eq!(config_index(DRV_ID_BASE_SURFACE), None);
        assert_eq!(surface_index(DRV_ID_BASE_CONTEXT), None);
        assert_eq!(context_index(DRV_ID_BASE_BUFFER), None);
        assert_eq!(buffer_index(DRV_ID_BASE_IMAGE), None);
        assert_eq!(image_index(DRV_ID_BASE_CONFIG), None);
    }
}
