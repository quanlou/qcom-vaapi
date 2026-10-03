use super::{VIDEO_MAX_PLANES_USIZE, zeroed};
use crate::bindings::*;
use std::ffi::c_void;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::ptr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BufferState {
    Free,
    Reserved,
    Queued,
}

pub(super) struct V4l2Buffer {
    pub(super) state: BufferState,
    /// VA surface that owns this slot as a stable-capture reservation.
    /// Reserved slots never enter the kernel queue: this firmware picks its
    /// own target buffer for every decoded frame, so the only way an
    /// exported dma-buf can keep backing the same VA surface is for its
    /// allocation to be invisible to the decoder until the driver copies the
    /// completed working-slot frame in at dequeue time.
    pub(super) reserved_for: Option<u32>,
    pub(super) num_planes: usize,
    pub(super) addr: [*mut c_void; VIDEO_MAX_PLANES_USIZE],
    pub(super) len: [usize; VIDEO_MAX_PLANES_USIZE],
    pub(super) planes: [v4l2_plane; VIDEO_MAX_PLANES_USIZE],
    /// Driver-side count of outstanding EXPBUF exports bound to this slot.
    /// Incremented by `export_capture`, retired by `retire_slot_exports`
    /// before the slot may return to the kernel queue. The stable-capture
    /// recycle path intentionally requeues slots with outstanding exports
    /// (the VA-API contract allows content changes once a surface is
    /// reused); this counter exists so surface-release ordering is
    /// observable and enforced, not to pin slots.
    pub(super) export_refs: u32,
    /// One CLOEXEC handle for cache maintenance and waiting for importer
    /// fences when CPU copies refresh this stable allocation.
    pub(super) sync_fd: Option<OwnedFd>,
    /// Externally allocated storage imported by CAPTURE. This owner follows
    /// the slot into legacy pools when the decoder session is rebuilt.
    pub(super) import_fd: Option<OwnedFd>,
}

impl V4l2Buffer {
    pub(super) fn new() -> Self {
        Self {
            state: BufferState::Free,
            reserved_for: None,
            num_planes: 0,
            addr: [ptr::null_mut(); VIDEO_MAX_PLANES_USIZE],
            len: [0; VIDEO_MAX_PLANES_USIZE],
            planes: [zeroed(); VIDEO_MAX_PLANES_USIZE],
            export_refs: 0,
            sync_fd: None,
            import_fd: None,
        }
    }

    pub(super) fn cpu_sync_fd(&self) -> Option<RawFd> {
        self.import_fd
            .as_ref()
            .or(self.sync_fd.as_ref())
            .map(AsRawFd::as_raw_fd)
    }
}

pub(super) struct V4l2Queue {
    pub(super) type_: u32,
    pub(super) memory: u32,
    pub(super) fmt: v4l2_format,
    pub(super) fourcc: u32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) buffers: Vec<V4l2Buffer>,
    pub(super) streaming: bool,
}

impl V4l2Queue {
    pub(super) fn new(type_: u32) -> Self {
        Self {
            type_,
            memory: v4l2_memory::V4L2_MEMORY_MMAP as u32,
            fmt: zeroed(),
            fourcc: 0,
            width: 0,
            height: 0,
            buffers: Vec::new(),
            streaming: false,
        }
    }
}
