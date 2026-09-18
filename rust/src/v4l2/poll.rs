//! V4L2 polling and completion ownership.
//!
//! This module converts kernel readiness and DQBUF events into typed
//! `ReadyCapture` records. It also resolves live and legacy CAPTURE indices for
//! CPU copies and dma-buf export without changing queue setup or submission.

use super::{
    BufferState, CaptureExport, O_CLOEXEC, PollFd, ReadyCapture, V4L2_DEC_CMD_START,
    V4L2_EVENT_EOS, V4L2_EVENT_SOURCE_CHANGE, V4l2Buffer, V4l2Session, VIDEO_MAX_PLANES_USIZE,
    VIDIOC_DECODER_CMD, VIDIOC_DQBUF, VIDIOC_DQEVENT, VIDIOC_EXPBUF, VIDIOC_G_FMT, debug_enabled,
    poll, xioctl, zeroed,
};
use crate::bindings::*;
use std::ffi::c_void;

fn should_ack_source_change_flush(capture_streaming: bool) -> bool {
    capture_streaming
}

fn should_resume_after_source_change(capture_streaming: bool) -> bool {
    capture_streaming
}

fn source_change_marker_is_expected(source_change_flush: bool, draining: bool) -> bool {
    source_change_flush && !draining
}

impl V4l2Session {
    pub(crate) fn pump(&mut self, timeout_ms: i32) -> Vec<ReadyCapture> {
        let mut ready = std::mem::take(&mut self.ready);
        let mut pfd = PollFd {
            fd: self.fd,
            events: super::POLLIN
                | super::POLLRDNORM
                | super::POLLPRI
                | super::POLLOUT
                | super::POLLWRNORM,
            revents: 0,
        };
        let ret = unsafe { poll(&mut pfd, 1, timeout_ms) };
        if ret <= 0 {
            // An empty CAPTURE dequeue can arm recovery just before the next
            // poll. Firmware may then stay silent, so do not require a second
            // readiness event before rebuilding the session.
            if self.aborted && !self.in_recover {
                let _ = self.recover();
            }
            return ready;
        }
        self.dequeue_events();
        if self.aborted && !self.in_recover {
            let _ = self.recover();
        }
        while self.dequeue_output() {}
        while let Some(r) = self.dequeue_capture() {
            ready.push(r);
        }
        ready
    }

    pub(super) fn out_queued(&self) -> usize {
        self.out
            .buffers
            .iter()
            .filter(|b| b.state == BufferState::Queued)
            .count()
    }

    pub(crate) fn requeue_capture(&mut self, idx: usize) {
        // Legacy-pool slots have no kernel queue to return to; they are
        // read-only remnants of a previous device incarnation.
        if idx < self.legacy_len() {
            return;
        }
        let live_idx = idx - self.legacy_len();
        if self
            .cap
            .buffers
            .get(live_idx)
            .is_some_and(|b| b.state == BufferState::Reserved)
        {
            if let Some(buffer) = self.cap.buffers.get_mut(live_idx) {
                buffer.state = BufferState::Free;
            }
        } else if self
            .cap
            .buffers
            .get(live_idx)
            .is_some_and(|b| b.state == BufferState::Free)
        {
            let _ = self.qbuf_capture(live_idx);
        }
    }

    /// Total number of CAPTURE slots provided by legacy pools. Live-pool
    /// indices reported to clients are offset by this amount.
    pub(super) fn legacy_len(&self) -> usize {
        self.legacy.iter().map(|p| p.buffers.len()).sum()
    }

    /// Resolve a client-visible virtual CAPTURE index to a buffer plus its
    /// pool's geometry, checking legacy pools before the live one.
    fn resolve_cap(&self, idx: usize) -> Option<(&V4l2Buffer, u32, u32, u32)> {
        let mut base = 0usize;
        for p in &self.legacy {
            if idx < base + p.buffers.len() {
                let b = &p.buffers[idx - base];
                return Some((b, p.width, p.height, p.stride));
            }
            base += p.buffers.len();
        }
        let b = self.cap.buffers.get(idx - base)?;
        let pix = unsafe { self.cap.fmt.fmt.pix_mp };
        Some((b, pix.width, pix.height, pix.plane_fmt[0].bytesperline))
    }

    pub(crate) fn capture_copy(&self, idx: usize) -> Option<(Vec<u8>, u32, u32)> {
        let (b, _w, height, stride) = self.resolve_cap(idx)?;
        if b.addr[0].is_null() || b.len[0] == 0 {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(b.addr[0] as *const u8, b.len[0]) }.to_vec();
        Some((bytes, stride, height))
    }

    pub(crate) fn export_capture(&self, idx: usize) -> Option<CaptureExport> {
        // Exporting from a legacy pool is impossible: its kernel queue and
        // device fd are gone. Already-exported legacy frames keep working
        // through the fds handed out before the rebuild.
        if idx < self.legacy_len() {
            return None;
        }
        let live_idx = idx - self.legacy_len();
        let b = self.cap.buffers.get(live_idx)?;
        if !matches!(b.state, BufferState::Free | BufferState::Reserved) || b.len[0] == 0 {
            return None;
        }
        let cap_pix = unsafe { self.cap.fmt.fmt.pix_mp };
        let stride = cap_pix.plane_fmt[0].bytesperline;
        let height = cap_pix.height;
        let uv_offset = stride.checked_mul(height)?;
        let mut exp: v4l2_exportbuffer = zeroed();
        exp.type_ = self.cap.type_;
        exp.index = live_idx as u32;
        exp.plane = 0;
        exp.flags = O_CLOEXEC;
        if xioctl(self.fd, VIDIOC_EXPBUF, &mut exp as *mut _ as *mut c_void).is_err() || exp.fd < 0
        {
            return None;
        }
        Some(CaptureExport {
            fd: exp.fd,
            size: b.len[0] as u32,
            width: cap_pix.width,
            height,
            stride,
            y_offset: 0,
            uv_offset,
        })
    }

    fn dequeue_events(&mut self) {
        loop {
            let mut evt: v4l2_event = zeroed();
            if xioctl(self.fd, VIDIOC_DQEVENT, &mut evt as *mut _ as *mut c_void).is_err() {
                break;
            }
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DQEVENT type={}", evt.type_);
            }
            if evt.type_ == V4L2_EVENT_EOS {
                // EOS is legitimate after this driver initiated a drain
                // (DECODER_CMD STOP), and Iris also raises a paired EOS around
                // SOURCE_CHANGE while keeping CAPTURE streaming. EOS with
                // pending work outside those two handshakes is the firmware
                // abort signature: the session is dead and must be rebuilt.
                let pending = self.fifo.len() + self.out_queued();
                if source_change_marker_is_expected(self.source_change_flush, self.draining) {
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: EOS paired with source change (pending={}); continuing",
                            pending
                        );
                    }
                } else if !self.draining && pending > 0 {
                    self.aborted = true;
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: anomalous EOS without drain (pending={}); recovery armed",
                            pending
                        );
                    }
                } else {
                    self.eos = true;
                }
            } else if evt.type_ == V4L2_EVENT_SOURCE_CHANGE {
                let mut newfmt: v4l2_format = zeroed();
                newfmt.type_ = self.cap.type_;
                if xioctl(self.fd, VIDIOC_G_FMT, &mut newfmt as *mut _ as *mut c_void).is_ok() {
                    let old_w = self.cap.width;
                    let old_h = self.cap.height;
                    self.cap.fmt = newfmt;
                    let pix = unsafe { self.cap.fmt.fmt.pix_mp };
                    self.cap.fourcc = pix.pixelformat;
                    self.cap.width = pix.width;
                    self.cap.height = pix.height;
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: SOURCE_CHANGE {}x{} -> {}x{} streaming={}",
                            old_w, old_h, self.cap.width, self.cap.height, self.cap.streaming
                        );
                    }
                    if should_ack_source_change_flush(self.cap.streaming) {
                        // Iris reports an empty CAPTURE marker and a paired EOS
                        // around SOURCE_CHANGE even when CAPTURE stays
                        // streaming. Mark that boundary so neither marker arms
                        // firmware recovery; the next real decoded frame clears
                        // it.
                        self.source_change_flush = true;
                    }
                    if should_resume_after_source_change(self.cap.streaming) {
                        // Without an explicit START this driver can remain
                        // silent after the marker pair with OUTPUT still queued.
                        // The marker handling above prevents the old false
                        // abort, so START is now only a resume nudge.
                        let mut cmd: v4l2_decoder_cmd = zeroed();
                        cmd.cmd = V4L2_DEC_CMD_START;
                        let _ = xioctl(
                            self.fd,
                            VIDIOC_DECODER_CMD,
                            &mut cmd as *mut _ as *mut c_void,
                        );
                    }
                }
            }
        }
    }

    fn dequeue_output(&mut self) -> bool {
        if self.out.buffers.is_empty() {
            return false;
        }
        let mut planes: [v4l2_plane; VIDEO_MAX_PLANES_USIZE] = [zeroed(); VIDEO_MAX_PLANES_USIZE];
        let mut buf: v4l2_buffer = zeroed();
        buf.type_ = self.out.type_;
        buf.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        buf.length = VIDEO_MAX_PLANES;
        buf.m.planes = planes.as_mut_ptr();
        if xioctl(self.fd, VIDIOC_DQBUF, &mut buf as *mut _ as *mut c_void).is_err() {
            return false;
        }
        let idx = buf.index as usize;
        if let Some(b) = self.out.buffers.get_mut(idx) {
            b.state = BufferState::Free;
            self.out_order.retain(|&i| i != idx);
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: OUT DQ idx={}", idx);
            }
            return true;
        }
        false
    }

    fn dequeue_capture(&mut self) -> Option<ReadyCapture> {
        if self.cap.buffers.is_empty() {
            return None;
        }
        let mut planes: [v4l2_plane; VIDEO_MAX_PLANES_USIZE] = [zeroed(); VIDEO_MAX_PLANES_USIZE];
        let mut buf: v4l2_buffer = zeroed();
        buf.type_ = self.cap.type_;
        buf.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        buf.length = VIDEO_MAX_PLANES;
        buf.m.planes = planes.as_mut_ptr();
        if xioctl(self.fd, VIDIOC_DQBUF, &mut buf as *mut _ as *mut c_void).is_err() {
            return None;
        }
        let idx = buf.index as usize;
        let b = self.cap.buffers.get_mut(idx)?;
        b.state = BufferState::Free;
        let plane_count = (buf.length as usize).min(VIDEO_MAX_PLANES_USIZE);
        b.planes[..plane_count].copy_from_slice(&planes[..plane_count]);
        let bytesused = planes[0].bytesused;
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: CAP DQ idx={} bytes={} ts={}.{}",
                idx, bytesused, buf.timestamp.tv_sec, buf.timestamp.tv_usec
            );
        }
        if bytesused == 0 {
            // Empty CAPTURE buffers are either a drain marker, a source-change
            // marker, or a firmware-abort signature. A marker immediately after
            // SOURCE_CHANGE is normal for this stateful decoder: native keeps
            // CAPTURE streaming and simply requeues the buffer. Treat only an
            // empty buffer outside drain/source-change, with pending work, as a
            // fatal session abort.
            let pending = self.fifo.len() + self.out_queued();
            if source_change_marker_is_expected(self.source_change_flush, self.draining) {
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE paired with source change (pending={}); requeueing",
                        pending
                    );
                }
            } else if !self.draining && pending > 0 {
                self.aborted = true;
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE without drain (pending={}); recovery armed",
                        pending
                    );
                }
            }
            let _ = self.qbuf_capture(idx);
            return None;
        }
        if self.fifo.is_empty() {
            let _ = self.qbuf_capture(idx);
            return None;
        }
        let ts_usec = (buf.timestamp.tv_sec as u64).saturating_mul(1_000_000)
            + (buf.timestamp.tv_usec as u64);
        let hit = self
            .fifo
            .iter()
            .position(|(_, ts)| *ts == ts_usec)
            .unwrap_or(0);
        let (surface, _) = self.fifo.remove(hit);
        // A real decoded frame means the decoder has resumed after any
        // source-change flush; stop suppressing abort detection.
        self.source_change_flush = false;
        Some(ReadyCapture {
            surface,
            cap_idx: self.legacy_len() + idx,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_source_change_is_acknowledged_and_resumed() {
        assert!(should_ack_source_change_flush(true));
        assert!(!should_ack_source_change_flush(false));
        assert!(should_resume_after_source_change(true));
        assert!(!should_resume_after_source_change(false));
    }

    #[test]
    fn source_change_markers_do_not_arm_firmware_abort() {
        assert!(source_change_marker_is_expected(true, false));
        assert!(!source_change_marker_is_expected(true, true));
        assert!(!source_change_marker_is_expected(false, false));
    }
}
