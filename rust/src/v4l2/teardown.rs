//! Deterministic V4L2 queue teardown.
//!
//! CAPTURE mappings and legacy pools can outlive the active decoder session
//! while published VA surfaces still reference their pixels. Teardown keeps
//! streamoff, unmapping, queue release, and `Drop` in one module so that
//! ownership changes remain auditable.

use super::{
    BufferState, LegacyPool, V4l2Queue, V4l2Session, VIDEO_MAX_PLANES_USIZE, VIDIOC_REQBUFS,
    VIDIOC_STREAMOFF, close, debug_enabled, release_mapping, xioctl, zeroed,
};
use crate::bindings::*;
use std::ffi::{c_int, c_void};
use std::ptr;

impl V4l2Session {
    pub(super) fn stream_off_fd(fd: c_int, q: &mut V4l2Queue) {
        if fd >= 0 && q.streaming {
            let mut type_ = q.type_ as c_int;
            let _ = xioctl(fd, VIDIOC_STREAMOFF, &mut type_ as *mut _ as *mut c_void);
            q.streaming = false;
        }
    }

    pub(super) fn release_queue_fd(fd: c_int, q: &mut V4l2Queue) {
        Self::stream_off_fd(fd, q);
        for b in &mut q.buffers {
            for p in 0..b.num_planes.min(VIDEO_MAX_PLANES_USIZE) {
                if !b.addr[p].is_null() && b.len[p] != 0 {
                    release_mapping(b.addr[p], b.len[p]);
                    b.addr[p] = ptr::null_mut();
                    b.len[p] = 0;
                }
            }
            b.num_planes = 0;
        }
        q.buffers.clear();
        if fd >= 0 {
            let mut req: v4l2_requestbuffers = zeroed();
            req.type_ = q.type_;
            req.memory = q.memory;
            req.count = 0;
            let _ = xioctl(fd, VIDIOC_REQBUFS, &mut req as *mut _ as *mut c_void);
        }
    }

    /// Best-effort flush before teardown. A session closed while OUTPUT
    /// buffers are still in flight can wedge the next CAPTURE STREAMON, so
    /// drain it within a bounded interval before releasing queue resources.
    pub(super) fn flush_for_teardown(&mut self) {
        if !self.out.streaming {
            return;
        }
        if self.out_queued() == 0 && self.fifo.is_empty() {
            return;
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        while self.out_queued() > 0 && std::time::Instant::now() < deadline {
            let ready = self.pump(10);
            self.ready.extend(ready);
        }
        if self.out_queued() == 0 && !self.fifo.is_empty() {
            self.maybe_start_drain();
        }
        while ((!self.fifo.is_empty() || self.out_queued() > 0) && !self.eos)
            && std::time::Instant::now() < deadline
        {
            let ready = self.pump(10);
            self.ready.extend(ready);
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: teardown flush done pending={} out_queued={} eos={}",
                self.fifo.len(),
                self.out_queued(),
                self.eos
            );
        }
    }
}

impl Drop for V4l2Session {
    fn drop(&mut self) {
        // Teardown must not trigger a session rebuild; the driver is going
        // away and a fresh device open here would just have to be closed.
        self.abandoned = true;
        self.flush_for_teardown();
        // Stream off CAPTURE before OUTPUT so decoded buffers still holding
        // hardware references are not discarded out of order.
        Self::release_queue_fd(self.fd, &mut self.cap);
        // Legacy mappings remain readable for published surfaces and must be
        // dropped only after the active queue has been released.
        for pool in &mut self.legacy {
            release_legacy_pool(pool);
        }
        self.legacy.clear();
        Self::release_queue_fd(self.fd, &mut self.out);
        if self.fd >= 0 {
            let _ = unsafe { close(self.fd) };
            self.fd = -1;
        }
    }
}

fn release_legacy_pool(pool: &mut LegacyPool) {
    for b in &mut pool.buffers {
        for p in 0..b.num_planes.min(VIDEO_MAX_PLANES_USIZE) {
            if !b.addr[p].is_null() && b.len[p] != 0 {
                release_mapping(b.addr[p], b.len[p]);
                b.addr[p] = ptr::null_mut();
                b.len[p] = 0;
            }
        }
        b.state = BufferState::Free;
    }
}
