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

fn source_change_marker_is_expected(source_change_flush: bool, draining: bool) -> bool {
    source_change_flush && !draining
}

/// How a completed source-change flush is resumed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DrcResumeMode {
    /// CAPTURE streamoff/streamon cycle: the kernel's streamon path re-queues
    /// the decoder's input-internal buffers and re-applies STAGE/PIPE before
    /// resuming the DRC.
    CaptureCycle,
    /// Bare `DECODER_CMD START`, valid for sessions whose CAPTURE queue was
    /// not streaming when the source change landed.
    DecoderStart,
}

fn drc_resume_mode(stable_capture: bool, cap_streaming: bool) -> DrcResumeMode {
    if stable_capture && cap_streaming {
        DrcResumeMode::CaptureCycle
    } else {
        DrcResumeMode::DecoderStart
    }
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
        if debug_enabled()
            && self
                .cap
                .buffers
                .get(live_idx)
                .is_some_and(|b| b.export_refs > 0)
        {
            eprintln!(
                "msm_drv_video_rs: requeueing cap_idx={} with outstanding export refs={}",
                idx, self.cap.buffers[live_idx].export_refs
            );
        }
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

    /// Retire per-slot export accounting after the surface's tracked dups
    /// were closed. Called from the surface-release path before
    /// `requeue_capture`, so the requeue always follows the retire; the
    /// decrement saturates because a rebound surface may carry dups that
    /// were counted against an older slot incarnation.
    pub(crate) fn retire_slot_exports(&mut self, idx: usize, retired: usize) {
        if idx < self.legacy_len() || retired == 0 {
            return;
        }
        let live_idx = idx - self.legacy_len();
        let Some(buffer) = self.cap.buffers.get_mut(live_idx) else {
            return;
        };
        let before = buffer.export_refs;
        buffer.export_refs = before.saturating_sub(retired as u32);
        if debug_enabled() && before != buffer.export_refs {
            eprintln!(
                "msm_drv_video_rs: retired exports cap_idx={} refs {} -> {}",
                idx, before, buffer.export_refs
            );
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

    pub(crate) fn export_capture(&mut self, idx: usize) -> Option<CaptureExport> {
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
        let size = b.len[0] as u32;
        let mut exp: v4l2_exportbuffer = zeroed();
        exp.type_ = self.cap.type_;
        exp.index = live_idx as u32;
        exp.plane = 0;
        exp.flags = O_CLOEXEC;
        if xioctl(self.fd, VIDIOC_EXPBUF, &mut exp as *mut _ as *mut c_void).is_err() || exp.fd < 0
        {
            return None;
        }
        // Account the export against the slot while the driver-side dup is
        // alive; callers retire it via `retire_slot_exports` when that dup
        // closes (surface release) or when the client dup fails (unwind).
        if let Some(b) = self.cap.buffers.get_mut(live_idx) {
            b.export_refs = b.export_refs.saturating_add(1);
        }
        Some(CaptureExport {
            fd: exp.fd,
            size,
            width: cap_pix.width,
            height,
            stride,
            y_offset: 0,
            uv_offset,
        })
    }

    fn maybe_resume_source_change(&mut self) {
        if self.source_change_flush && self.source_change_empty_seen && self.source_change_eos_seen
        {
            self.nudge_source_change_start();
        }
    }

    fn nudge_source_change_start(&mut self) {
        if self.source_change_start_sent {
            return;
        }
        // Stable-capture (export) sessions stream CAPTURE before the first AU,
        // so the first source change lands as a full DRC while both planes are
        // streaming. A bare DECODER_CMD START resumes that DRC but skips the
        // CAPTURE-streamon work (input-internal buffer requeue, STAGE/PIPE),
        // which leaves the firmware silently unable to consume OUTPUT; run the
        // streamon-based resume instead and keep the bare START for sessions
        // that match the native CAPTURE-after-event flow.
        if drc_resume_mode(self.stable_capture, self.cap.streaming) == DrcResumeMode::CaptureCycle
            && self.resume_drc_capture_cycle()
        {
            self.source_change_start_sent = true;
            self.source_change_flush = false;
            self.source_change_empty_seen = false;
            self.source_change_eos_seen = false;
            return;
        }
        let mut cmd: v4l2_decoder_cmd = zeroed();
        cmd.cmd = V4L2_DEC_CMD_START;
        let res = xioctl(
            self.fd,
            VIDIOC_DECODER_CMD,
            &mut cmd as *mut _ as *mut c_void,
        );
        if debug_enabled() {
            if res.is_ok() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after source-change event");
            } else {
                eprintln!(
                    "msm_drv_video_rs: DECODER_CMD START after source-change event failed/ignored"
                );
            }
        }
        self.source_change_start_sent = true;
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
                if self.drain_eos_grace {
                    self.drain_eos_grace = false;
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: EOS paired with prior drain (pending={}); continuing",
                            pending
                        );
                    }
                } else if source_change_marker_is_expected(self.source_change_flush, self.draining)
                {
                    self.source_change_eos_seen = true;
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: EOS paired with source change (pending={}); continuing",
                            pending
                        );
                    }
                    self.maybe_resume_source_change();
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
                        self.source_change_empty_seen = false;
                        self.source_change_eos_seen = false;
                        self.source_change_start_sent = false;
                        // Keep CAPTURE streaming and treat the following empty
                        // CAPTURE/EOS pair as a marker. This firmware accepts
                        // START in rebuilt sessions but then rejects OUTPUT.
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
                self.source_change_empty_seen = true;
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE paired with source change (pending={}); requeueing",
                        pending
                    );
                }
                self.maybe_resume_source_change();
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
        let Some(hit) = self.fifo.iter().position(|(_, ts)| *ts == ts_usec) else {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: CAP timestamp {} has no pending surface; dropping replay output",
                    ts_usec
                );
            }
            let _ = self.qbuf_capture(idx);
            return None;
        };
        let (surface, _) = self.fifo.remove(hit);
        self.published_timestamps.push_back(ts_usec);
        const MAX_PUBLISHED_TIMESTAMPS: usize = 128;
        if self.published_timestamps.len() > MAX_PUBLISHED_TIMESTAMPS {
            self.published_timestamps.pop_front();
        }
        // A real decoded frame means the decoder has resumed after any
        // source-change flush; stop suppressing abort detection.
        self.source_change_flush = false;
        self.source_change_empty_seen = false;
        self.source_change_eos_seen = false;
        self.source_change_start_sent = false;
        self.drain_eos_grace = false;
        let cap_idx = self.legacy_len() + idx;
        let frame =
            self.capture_copy(cap_idx)
                .map(|(data, stride, height)| crate::state::SurfaceFrame {
                    data,
                    stride,
                    height,
                });
        Some(ReadyCapture {
            surface,
            cap_idx,
            frame,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaming_source_change_is_acknowledged() {
        assert!(should_ack_source_change_flush(true));
        assert!(!should_ack_source_change_flush(false));
    }

    #[test]
    fn source_change_markers_do_not_arm_firmware_abort() {
        assert!(source_change_marker_is_expected(true, false));
        assert!(!source_change_marker_is_expected(true, true));
        assert!(!source_change_marker_is_expected(false, false));
    }

    #[test]
    fn only_stable_streaming_sessions_resume_drc_via_capture_cycle() {
        assert_eq!(drc_resume_mode(true, true), DrcResumeMode::CaptureCycle);
        // Without stable capture the native-shaped CAPTURE-after-event flow
        // still resumes through the bare START.
        assert_eq!(drc_resume_mode(true, false), DrcResumeMode::DecoderStart);
        assert_eq!(drc_resume_mode(false, true), DrcResumeMode::DecoderStart);
        assert_eq!(drc_resume_mode(false, false), DrcResumeMode::DecoderStart);
    }
}
