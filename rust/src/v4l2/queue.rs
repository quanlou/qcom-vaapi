use super::{VIDEO_MAX_PLANES_USIZE, zeroed};
use crate::bindings::*;
use std::ffi::c_void;
use std::ptr;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum BufferState {
    Free,
    Reserved,
    Queued,
}

pub(super) struct V4l2Buffer {
    pub(super) state: BufferState,
    pub(super) num_planes: usize,
    pub(super) addr: [*mut c_void; VIDEO_MAX_PLANES_USIZE],
    pub(super) len: [usize; VIDEO_MAX_PLANES_USIZE],
    pub(super) planes: [v4l2_plane; VIDEO_MAX_PLANES_USIZE],
}

impl V4l2Buffer {
    pub(super) fn new() -> Self {
        Self {
            state: BufferState::Free,
            num_planes: 0,
            addr: [ptr::null_mut(); VIDEO_MAX_PLANES_USIZE],
            len: [0; VIDEO_MAX_PLANES_USIZE],
            planes: [zeroed(); VIDEO_MAX_PLANES_USIZE],
        }
    }
}

pub(super) struct V4l2Queue {
    pub(super) type_: u32,
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
            fmt: zeroed(),
            fourcc: 0,
            width: 0,
            height: 0,
            buffers: Vec::new(),
            streaming: false,
        }
    }
}
