//! OUTPUT submission and decoder drain.
//!
//! This module owns pacing, OUTPUT QBUF construction, and the explicit STOP
//! drain used at end of stream or teardown. Device polling remains in the
//! parent session runtime so submission and completion paths stay distinct.

use super::{
    BufferState, OUT_NUM_BUFFERS, V4L2_BUF_FLAG_KEYFRAME, V4L2_DEC_CMD_START, V4L2_DEC_CMD_STOP,
    V4l2Session, VIDIOC_DECODER_CMD, VIDIOC_QBUF, debug_enabled, xioctl, zeroed,
};
use crate::bindings::*;
use std::ffi::c_void;
use std::ptr;

fn output_inflight_limit(source_change_flush: bool) -> usize {
    if source_change_flush {
        OUT_NUM_BUFFERS as usize
    } else {
        2
    }
}

impl V4l2Session {
    fn resume_source_change_decode(&mut self) {
        if !self.source_change_flush || !self.out.streaming {
            return;
        }
        let mut cmd: v4l2_decoder_cmd = zeroed();
        cmd.cmd = V4L2_DEC_CMD_START;
        if xioctl(
            self.fd,
            VIDIOC_DECODER_CMD,
            &mut cmd as *mut _ as *mut c_void,
        )
        .is_ok()
        {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after source-change submission");
            }
        } else if debug_enabled() {
            eprintln!("msm_drv_video_rs: DECODER_CMD START after source-change submission failed");
        }
    }

    pub(crate) fn submit_frame(
        &mut self,
        surface: u32,
        cap_idx: Option<usize>,
        data: &[u8],
        keyframe: bool,
        timestamp_usec: u64,
        headers: &[u8],
    ) -> Result<(), ()> {
        if !headers.is_empty() {
            self.headers = headers.to_vec();
        }
        // A previously armed abort means the device behind this session is
        // dead; rebuild before queuing anything else. If the rebuild already
        // failed permanently, fail this frame the way pre-recovery did.
        if self.aborted {
            self.recover()?;
        }
        let mut attempts = 0;
        while self.out_queued() >= output_inflight_limit(self.source_change_flush) {
            if self.aborted && self.recover().is_ok() {
                attempts = 0;
                continue;
            }
            attempts += 1;
            if attempts > 2500 {
                if debug_enabled() {
                    eprintln!("msm_drv_video_rs: output pacing stall");
                }
                return Err(());
            }
            let ready = self.pump(2);
            self.ready.extend(ready);
        }
        attempts = 0;
        while self
            .out
            .buffers
            .iter()
            .all(|b| b.state != BufferState::Free)
        {
            if self.aborted && self.recover().is_ok() {
                attempts = 0;
                continue;
            }
            attempts += 1;
            if attempts > 2500 {
                if debug_enabled() {
                    eprintln!("msm_drv_video_rs: no free OUTPUT after pumping");
                }
                return Err(());
            }
            let ready = self.pump(2);
            self.ready.extend(ready);
        }
        if let Some(cap_idx) = cap_idx {
            self.queue_capture(cap_idx)?;
        } else {
            self.queue_all_capture()?;
        }
        let idx = self
            .qbuf_output_bytes(data, keyframe, timestamp_usec)
            .map_err(|_| {
                if debug_enabled() {
                    eprintln!("msm_drv_video_rs: output buffer invalid or QBUF failed");
                }
            })?;
        self.eos = false;
        self.draining = false;
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: OUTPUT QBUF idx={} bytes={} queued={}",
                idx,
                data.len(),
                self.out_queued()
            );
        }
        if let Err(e) = self.try_start() {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: try_start failed");
            }
            return Err(e);
        }
        self.resume_source_change_decode();
        self.fifo.push((surface, timestamp_usec));
        Ok(())
    }

    /// Copy `data` into a free OUTPUT buffer and queue it. Shared by normal
    /// submission and post-rebuild replay. Returns the buffer index.
    pub(super) fn qbuf_output_bytes(
        &mut self,
        data: &[u8],
        keyframe: bool,
        timestamp_usec: u64,
    ) -> Result<usize, ()> {
        let idx = self
            .out
            .buffers
            .iter()
            .position(|b| b.state == BufferState::Free)
            .ok_or(())?;
        let b = &mut self.out.buffers[idx];
        if b.num_planes == 0 || b.addr[0].is_null() || data.len() > b.len[0] {
            return Err(());
        }
        unsafe { ptr::copy_nonoverlapping(data.as_ptr(), b.addr[0] as *mut u8, data.len()) };
        b.planes[0].bytesused = data.len() as u32;

        let mut buf: v4l2_buffer = zeroed();
        buf.type_ = self.out.type_;
        buf.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        buf.index = idx as u32;
        buf.length = b.num_planes as u32;
        buf.flags = if keyframe { V4L2_BUF_FLAG_KEYFRAME } else { 0 };
        buf.timestamp.tv_sec = (timestamp_usec / 1_000_000) as _;
        buf.timestamp.tv_usec = (timestamp_usec % 1_000_000) as _;
        buf.m.planes = b.planes.as_mut_ptr();
        let fd = self.fd;
        let res = xioctl(fd, VIDIOC_QBUF, &mut buf as *mut _ as *mut c_void);
        // If the QBUF failed the buffer must go back to Free so later
        // submissions can reuse it; do this before the error return.
        if res.is_ok() {
            b.state = BufferState::Queued;
            self.out_order.push_back(idx);
        }
        res.map(|_| idx)
    }

    pub(crate) fn maybe_start_drain(&mut self) {
        if self.draining || self.fifo.is_empty() || self.out_queued() != 0 || !self.out.streaming {
            return;
        }
        let mut cmd: v4l2_decoder_cmd = zeroed();
        cmd.cmd = V4L2_DEC_CMD_STOP;
        if xioctl(
            self.fd,
            VIDIOC_DECODER_CMD,
            &mut cmd as *mut _ as *mut c_void,
        )
        .is_ok()
        {
            self.draining = true;
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: DECODER_CMD STOP drain started (pending={})",
                    self.fifo.len()
                );
            }
        } else if debug_enabled() {
            eprintln!("msm_drv_video_rs: DECODER_CMD STOP drain failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_change_flush_temporarily_uses_full_output_queue() {
        assert_eq!(output_inflight_limit(false), 2);
        assert_eq!(output_inflight_limit(true), OUT_NUM_BUFFERS as usize);
        assert!(output_inflight_limit(true) > output_inflight_limit(false));
    }
}
