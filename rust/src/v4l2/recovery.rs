//! Bounded recovery for firmware-aborted V4L2 decoder sessions.
//!
//! The hardware can raise EOS or return an empty CAPTURE buffer while queued
//! OUTPUT work remains. Recovery is intentionally capped: repeated rebuilds
//! poison the firmware and make subsequent native sessions fail as well.

use super::{
    LegacyPool, MAX_SESSION_RECOVERIES, V4L2_DEC_CMD_STOP, V4l2Queue, V4l2Session,
    VIDIOC_DECODER_CMD, debug_enabled, open, xioctl, zeroed,
};
use crate::bindings::*;
use std::borrow::Cow;
use std::ffi::CString;

impl V4l2Session {
    /// Rebuild the V4L2 session after a firmware abort. One rebuild is allowed
    /// for a transient session failure; a second failure latches the session.
    pub(super) fn recover(&mut self) -> Result<(), ()> {
        if !self.aborted {
            return Ok(());
        }
        if self.abandoned {
            return Err(());
        }
        self.in_recover = true;
        let result = self.recover_inner();
        self.in_recover = false;
        result
    }

    fn recover_inner(&mut self) -> Result<(), ()> {
        if !self.aborted {
            return Ok(());
        }
        if self.abandoned {
            return Err(());
        }
        self.recoveries += 1;
        let attempt = self.recoveries;
        if attempt > MAX_SESSION_RECOVERIES {
            self.abandoned = true;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: session recovery limit reached; giving up");
            }
            return Err(());
        }
        if debug_enabled() {
            eprintln!("msm_drv_video_rs: session rebuild attempt {}", attempt);
        }

        let chunks = self.snapshot_pending_output();
        let fifo_tail = self.fifo.clone();
        if chunks.is_empty() {
            self.abandoned = true;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: session rebuild has no replayable OUTPUT");
            }
            return Err(());
        }
        // Never truncate pending owners to fit the kernel queue. A consumed
        // OUTPUT can still own an unpublished picture; discarding that owner
        // would leave its VA surface pending forever after a successful rebuild.
        if fifo_tail.len() != chunks.len() {
            self.abandoned = true;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: session rebuild fifo mismatch");
            }
            return Err(());
        }
        // Parameter sets do not restore decoded reference pictures. The
        // current rebuild can replay only queued OUTPUT, so it is safe only
        // when that queue contains the complete retained GOP.
        if !super::replay::rebuild_is_complete(&self.replay_history, &fifo_tail, &chunks) {
            self.abandoned = true;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: rebuild lacks a queued keyframe reference chain");
            }
            return Err(());
        }
        let headers = std::mem::take(&mut self.headers);
        let (w, h) = (self.out.width as i32, self.out.height as i32);

        let c_path = match CString::new(self.devnode.as_str()) {
            Ok(path) => path,
            Err(_) => {
                self.abandoned = true;
                self.headers = headers;
                return Err(());
            }
        };
        let new_fd = unsafe {
            open(
                c_path.as_ptr(),
                super::O_RDWR | super::O_NONBLOCK | super::O_CLOEXEC as std::ffi::c_int,
                0,
            )
        };
        if new_fd < 0 {
            self.abandoned = true;
            self.headers = headers;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: session rebuild reopen failed");
            }
            return Err(());
        }

        // Stop and flush the old incarnation before closing it. Closing with
        // OUTPUT queued can wedge the next CAPTURE STREAMON.
        if self.out.streaming {
            let mut cmd: v4l2_decoder_cmd = zeroed();
            cmd.cmd = V4L2_DEC_CMD_STOP;
            if xioctl(
                self.fd,
                VIDIOC_DECODER_CMD,
                &mut cmd as *mut _ as *mut std::ffi::c_void,
            )
            .is_ok()
            {
                self.draining = true;
            }
            self.flush_for_teardown();
        }
        let (legacy_w, legacy_h, legacy_stride) = {
            let pix = unsafe { self.cap.fmt.fmt.pix_mp };
            (
                self.cap.width,
                self.cap.height,
                pix.plane_fmt[0].bytesperline,
            )
        };
        let legacy_buffers = std::mem::take(&mut self.cap.buffers);
        self.legacy.push(LegacyPool {
            buffers: legacy_buffers,
            width: legacy_w,
            height: legacy_h,
            stride: legacy_stride,
        });
        Self::release_queue_fd(self.fd, &mut self.out);
        let old_fd = self.fd;
        self.fd = new_fd;
        if old_fd >= 0 {
            let _ = unsafe { super::close(old_fd) };
        }
        self.out = V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE as u32);
        self.cap = V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as u32);
        self.out_order.clear();
        self.fifo.clear();
        self.eos = false;
        self.draining = false;
        self.aborted = false;

        let setup = self
            .query_cap()
            .and_then(|_| self.subscribe_events())
            .and_then(|_| self.setup_output(w, h, self.coded_fourcc))
            .and_then(|_| self.capture_pool_setup());
        if setup.is_err() {
            self.abandoned = true;
            self.headers = headers;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: session rebuild setup failed");
            }
            return Err(());
        }

        // The rebuilt decoder has no parameter sets, so prepend the last
        // synthesized SPS/PPS to the first replayed chunk.
        for (i, chunk) in chunks.iter().enumerate() {
            let payload = if i == 0 && !headers.is_empty() {
                let mut first = headers.clone();
                first.extend_from_slice(chunk);
                Cow::Owned(first)
            } else {
                Cow::Borrowed(chunk.as_slice())
            };
            let pending = fifo_tail[i].clone();
            if self.stable_capture {
                // Rebind the replayed surface's reservation in the new pool.
                // Reserved slots never enter the kernel queue, so only the
                // unreserved working sub-pool is topped up here; the DQ-time
                // copy in `dequeue_capture` finds the reservation by surface.
                if self.reserve_capture(pending.surface).is_none() {
                    self.abandoned = true;
                    self.headers = headers;
                    if debug_enabled() {
                        eprintln!("msm_drv_video_rs: session rebuild CAPTURE reservation failed");
                    }
                    return Err(());
                }
                if self.queue_working_capture().is_err() {
                    self.abandoned = true;
                    self.headers = headers;
                    if debug_enabled() {
                        eprintln!("msm_drv_video_rs: session rebuild CAPTURE QBUF failed");
                    }
                    return Err(());
                }
            } else if i == 0 && self.queue_all_capture().is_err() {
                self.abandoned = true;
                self.headers = headers;
                if debug_enabled() {
                    eprintln!("msm_drv_video_rs: session rebuild CAPTURE QBUF failed");
                }
                return Err(());
            }
            match self.qbuf_output_bytes(
                &payload,
                i == 0,
                pending.timestamp,
                Some(pending.surface),
                pending.expects_output,
                false,
            ) {
                Ok(idx) => {
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: rebuild replay QBUF idx={} bytes={}",
                            idx,
                            payload.len()
                        );
                    }
                    self.fifo.push(pending);
                }
                Err(_) => {
                    self.abandoned = true;
                    self.headers = headers;
                    if debug_enabled() {
                        eprintln!("msm_drv_video_rs: session rebuild replay failed");
                    }
                    return Err(());
                }
            }
        }
        self.headers = headers;
        // A STREAMON failure is left retryable for the next submission; it is
        // the known post-churn firmware condition rather than a replay error.
        if self.try_start().is_err() && debug_enabled() {
            eprintln!("msm_drv_video_rs: session rebuild STREAMON failed; will retry");
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: session rebuilt attempt={} replayed={} legacy_pools={}",
                attempt,
                chunks.len(),
                self.legacy.len()
            );
        }
        Ok(())
    }
}
