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
use std::ptr;

fn should_ack_source_change_flush(capture_streaming: bool) -> bool {
    capture_streaming
}

fn source_change_marker_is_expected(source_change_flush: bool, draining: bool) -> bool {
    source_change_flush && !draining
}

fn take_prior_drain_marker(grace: &mut bool, draining: bool) -> bool {
    !draining && std::mem::take(grace)
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

fn drc_resume_mode(cap_streaming: bool) -> DrcResumeMode {
    if cap_streaming {
        DrcResumeMode::CaptureCycle
    } else {
        DrcResumeMode::DecoderStart
    }
}

impl V4l2Session {
    pub(crate) fn pump(&mut self, timeout_ms: i32) -> Vec<ReadyCapture> {
        let mut ready = std::mem::take(&mut self.ready);
        // A CPU-copy client can block in sync/download while its submitter
        // waits for that frame. Completed slots already have detached pixel
        // snapshots, so replenish the bounded working queue during polling
        // too; waiting for the next submission can starve this completion.
        if !self.stable_capture
            && self.cap.streaming
            && !self.aborted
            && !self.abandoned
            && self.queue_working_capture().is_err()
        {
            self.abandoned = true;
            return ready;
        }
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
        // Iris marks both vb2 queues in error after an HFI session fatal.
        // v4l2_m2m_poll then reports POLLERR rather than a usable completion.
        // Startup and an empty pair of queues also report POLLERR, so require
        // both queues to be streaming with at least one known queued buffer
        // before treating it as terminal. A lost/invalid descriptor is always fatal.
        let lost_fd = pfd.revents & (super::POLLHUP | super::POLLNVAL) != 0;
        let failed_queues = pfd.revents & super::POLLERR != 0
            && self.out.streaming
            && self.cap.streaming
            && (self
                .out
                .buffers
                .iter()
                .any(|b| b.state == BufferState::Queued)
                || self
                    .cap
                    .buffers
                    .iter()
                    .any(|b| b.state == BufferState::Queued));
        if lost_fd || failed_queues {
            // A normal browser does not enable verbose decoder logging. Keep
            // one bounded failure record so the next incident retains the
            // compressed format and dimensions without any media payload.
            if !self.abandoned {
                eprintln!(
                    "msm_drv_video_rs: terminal decoder poll revents=0x{:x} coded_fourcc=0x{:08x} dimensions={}x{} streaming={}/{} queued={}/{} pending={}/{}; no session rebuild",
                    pfd.revents,
                    self.coded_fourcc,
                    self.out.width,
                    self.out.height,
                    self.out.streaming,
                    self.cap.streaming,
                    self.out_queued(),
                    self.cap
                        .buffers
                        .iter()
                        .filter(|b| b.state == BufferState::Queued)
                        .count(),
                    self.fifo.len(),
                    self.no_output_waiting.len()
                );
            }
            self.abandoned = true;
            // Retire pending VA owners as errors, without publishing pixels
            // or disturbing the mappings/queue state required by teardown.
            for pending in self.fifo.drain(..) {
                ready.push(ReadyCapture {
                    surface: pending.surface,
                    failed: true,
                    cap_idx: None,
                    frame: None,
                });
            }
            for surface in self.no_output_waiting.drain(..) {
                ready.push(ReadyCapture {
                    surface,
                    failed: true,
                    cap_idx: None,
                    frame: None,
                });
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
        self.finish_discarded_drain(&mut ready);
        ready
    }

    fn finish_discarded_drain(&mut self, ready: &mut Vec<ReadyCapture>) {
        // LAST proves CAPTURE has no further completions in this drain.
        // Also wait for every OUTPUT buffer to be returned. Remaining owners
        // were discarded by firmware; keeping them Pending blocks a new IDR,
        // while marking them Ready would certify pixels we never received.
        if !self.draining || !self.drain_last_seen || self.out_queued() != 0 {
            return;
        }
        for surface in self
            .fifo
            .drain(..)
            .map(|pending| pending.surface)
            .chain(self.no_output_waiting.drain(..))
        {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: completed drain discarded surface={}; reporting decode error",
                    surface
                );
            }
            ready.push(ReadyCapture {
                surface,
                failed: true,
                cap_idx: None,
                frame: None,
            });
        }
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
                buffer.reserved_for = None;
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

    pub(crate) fn capture_copy(&mut self, idx: usize) -> Option<(Vec<u8>, u32, u32)> {
        // Legacy pools retain mappings established before their slots moved.
        // Live slots need no CPU mapping until a completed frame is read.
        let legacy_len = self.legacy_len();
        if idx >= legacy_len {
            self.map_buffer(false, idx - legacy_len).ok()?;
        }
        let (b, width, height, stride) = self.resolve_cap(idx)?;
        let format = crate::pixel_format::DecodedFormat::from_v4l2_fourcc(self.capture_fourcc)?;
        let required = semiplanar_storage_size(width, height, stride, format)?;
        if b.addr[0].is_null() || required > b.len[0] {
            return None;
        }
        let bytes =
            unsafe { std::slice::from_raw_parts(b.addr[0] as *const u8, b.len[0]) }.to_vec();
        Some((bytes, stride, height))
    }

    /// Copy a completed live CAPTURE slot's mapped plane into another live
    /// slot and mark the destination plane used. This is the stable-capture
    /// publish step: the firmware-chosen working slot's frame must land in
    /// the surface's reserved slot, whose allocation backs the surface's
    /// exported dma-bufs.
    pub(super) fn copy_capture_slot(&mut self, from_live: usize, to_live: usize) -> Result<(), ()> {
        if from_live == to_live {
            return Ok(());
        }
        self.map_buffer(false, from_live)?;
        self.map_buffer(false, to_live)?;
        let (src, len) = match self.cap.buffers.get(from_live) {
            Some(b) if !b.addr[0].is_null() => (b.addr[0], b.len[0]),
            _ => return Err(()),
        };
        let Some(dst) = self.cap.buffers.get_mut(to_live) else {
            return Err(());
        };
        if dst.addr[0].is_null() || len == 0 {
            return Err(());
        }
        if len > dst.len[0] {
            return Err(());
        }
        use std::os::fd::AsRawFd;
        let access =
            super::dmabuf::CpuWriteAccess::begin(dst.sync_fd.as_ref().map(AsRawFd::as_raw_fd))?;
        let bytes = len;
        unsafe { ptr::copy_nonoverlapping(src as *const u8, dst.addr[0] as *mut u8, bytes) };
        access.finish()?;
        dst.planes[0].bytesused = bytes as u32;
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: stable publish slot {} -> {} bytes={}",
                from_live, to_live, bytes
            );
        }
        Ok(())
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
        let format = crate::pixel_format::DecodedFormat::from_v4l2_fourcc(self.capture_fourcc)?;
        let required = semiplanar_storage_size(cap_pix.width, height, stride, format)?;
        if required > b.len[0] {
            return None;
        }
        let size = u32::try_from(b.len[0]).ok()?;
        let mut exp: v4l2_exportbuffer = zeroed();
        exp.type_ = self.cap.type_;
        exp.index = live_idx as u32;
        exp.plane = 0;
        exp.flags = O_CLOEXEC;
        if xioctl(self.fd, VIDIOC_EXPBUF, &mut exp as *mut _ as *mut c_void).is_err() || exp.fd < 0
        {
            return None;
        }
        // Keep a separate allocation handle for CPU cache maintenance. It
        // lives with the slot, independently of the client export lifetime.
        use std::os::fd::BorrowedFd;
        if self.cap.buffers[live_idx].sync_fd.is_none() {
            let duplicate = unsafe { BorrowedFd::borrow_raw(exp.fd) }.try_clone_to_owned();
            let Ok(duplicate) = duplicate else {
                unsafe { super::close(exp.fd) };
                return None;
            };
            self.cap.buffers[live_idx].sync_fd = Some(duplicate);
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
            format,
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
        // Source change can land while both queues are already streaming in
        // either CPU-copy or export sessions. A bare START skips the
        // CAPTURE-streamon work (input-internal buffer requeue, STAGE/PIPE),
        // which leaves the firmware silently unable to consume OUTPUT; run the
        // streamon-based resume instead and keep the bare START for sessions
        // that match the native CAPTURE-after-event flow.
        if drc_resume_mode(self.cap.streaming) == DrcResumeMode::CaptureCycle
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

    pub(super) fn dequeue_events(&mut self) {
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
                    self.capture_metadata_ready |=
                        unsafe { evt.u.src_change.changes } & V4L2_EVENT_SRC_CH_RESOLUTION != 0;
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
        if buf.length != 1
            || bytesused as usize > b.len[0]
            || planes[0].data_offset != 0
            // Iris marks some empty source-change/drain completions ERROR.
            // Their stateful marker handling below must run; damaged pixel
            // payloads still fail before publication.
            || (bytesused != 0 && buf.flags & V4L2_BUF_FLAG_ERROR != 0)
        {
            // Never publish damaged frames or reinterpret an unsupported
            // memory layout as contiguous NV12/P010 pixels.
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: invalid CAPTURE completion index={} planes={} bytesused={} length={} offset={} flags=0x{:x}",
                    idx, buf.length, bytesused, b.len[0], planes[0].data_offset, buf.flags,
                );
            }
            self.abandoned = true;
            return None;
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: CAP DQ idx={} bytes={} ts={}.{} flags=0x{:x}",
                idx, bytesused, buf.timestamp.tv_sec, buf.timestamp.tv_usec, buf.flags
            );
        }
        if self.draining && buf.flags & V4L2_BUF_FLAG_LAST != 0 {
            self.drain_last_seen = true;
        }
        if bytesused == 0 {
            // Empty CAPTURE buffers are either a drain marker, a source-change
            // marker, an AV1 hidden-reference completion, or a firmware-abort
            // signature. A marker immediately after SOURCE_CHANGE is normal for
            // this stateful decoder: native keeps CAPTURE streaming and simply
            // requeues the buffer. Treat only an empty buffer outside known
            // marker/no-output cases, with pending work, as a fatal session
            // abort.
            let pending = self.fifo.len() + self.out_queued();
            if take_prior_drain_marker(&mut self.drain_empty_grace, self.draining) {
                // START can precede dequeue of STOP's final empty buffer.
                // This buffer has no picture owner. Treat exactly one as the
                // prior drain marker, independently of the paired EOS event.
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE paired with prior drain (pending={}); requeueing",
                        pending
                    );
                }
            } else if source_change_marker_is_expected(self.source_change_flush, self.draining) {
                self.source_change_empty_seen = true;
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE paired with source change (pending={}); requeueing",
                        pending
                    );
                }
                self.maybe_resume_source_change();
            } else if self
                .fifo
                .first()
                .is_some_and(|pending| !pending.expects_output)
            {
                let hidden = self.fifo.remove(0);
                let _ = self.qbuf_capture(idx);
                self.no_output_waiting.push(hidden.surface);
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: empty CAPTURE retired no-output surface={} ts={} (pending={}); waiting for next displayable capture",
                        hidden.surface, hidden.timestamp, pending
                    );
                }
                return None;
            } else if !self.draining && pending > 0 && self.out_queued() == 0 {
                self.maybe_start_sync_drain();
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
        // A real capture after START ends the window for a delayed marker,
        // including reference-only replay output without a FIFO owner.
        self.drain_empty_grace = false;
        if self.fifo.is_empty() {
            let _ = self.qbuf_capture(idx);
            return None;
        }
        let ts_usec = (buf.timestamp.tv_sec as u64).saturating_mul(1_000_000)
            + (buf.timestamp.tv_usec as u64);
        let Some(hit) = self
            .fifo
            .iter()
            .position(|pending| pending.timestamp == ts_usec)
        else {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: CAP timestamp {} has no pending surface; dropping replay output",
                    ts_usec
                );
            }
            let _ = self.qbuf_capture(idx);
            return None;
        };
        let pending = self.fifo.remove(hit);
        let surface = pending.surface;
        self.published_timestamps.push_back(ts_usec);
        const MAX_PUBLISHED_TIMESTAMPS: usize = super::replay::MAX_REPLAY_CHUNKS;
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
        // Stable-capture publish: the firmware decoded into its own working
        // slot (`idx`), but this surface's exported dma-bufs alias its
        // reserved slot. Copy the completed frame into the reservation and
        // publish the reservation index so export identity and publish
        // identity agree. Legacy CPU-copy sessions keep publishing the
        // dequeued slot itself.
        let dq_cap_idx = self.legacy_len() + idx;
        let cap_idx = if self.stable_capture
            && let Some(reserved) = self.reserved_capture_for(surface)
        {
            if reserved != dq_cap_idx
                && self
                    .copy_capture_slot(idx, reserved - self.legacy_len())
                    .is_err()
            {
                self.abandoned = true;
                return None;
            }
            reserved
        } else {
            dq_cap_idx
        };
        // Recover the DecodedFormat that the session was set up with, so
        // late CPU-copy reads (vaGetImage / vaDeriveImage) can pick the
        // right layout without carrying the raw V4L2 fourcc through the
        // state layer.
        let format = crate::pixel_format::DecodedFormat::from_v4l2_fourcc(self.capture_fourcc)
            .unwrap_or(crate::pixel_format::DecodedFormat::Nv12);
        let frame =
            self.capture_copy(cap_idx)
                .map(|(data, stride, height)| crate::state::SurfaceFrame {
                    data: std::sync::Arc::new(data),
                    stride,
                    height,
                    format,
                });
        if frame.is_none() {
            self.abandoned = true;
            return None;
        }
        let ready = ReadyCapture {
            surface,
            failed: false,
            cap_idx: Some(cap_idx),
            frame,
        };
        for hidden_surface in self.no_output_waiting.drain(..) {
            self.ready.push(ReadyCapture {
                surface: hidden_surface,
                failed: false,
                cap_idx: ready.cap_idx,
                frame: ready.frame.clone(),
            });
        }
        Some(ready)
    }
}

fn semiplanar_storage_size(
    width: u32,
    height: u32,
    stride: u32,
    format: crate::pixel_format::DecodedFormat,
) -> Option<usize> {
    if width == 0
        || height == 0
        || !width.is_multiple_of(2)
        || !height.is_multiple_of(2)
        || stride < width.checked_mul(format.bytes_per_sample())?
    {
        return None;
    }
    (stride as usize).checked_mul(height as usize + height as usize / 2)
}

#[cfg(test)]
mod tests {
    use super::super::{LegacyPool, O_RDWR, V4L2_PIX_FMT_H264, V4l2Buffer, V4l2Queue, open};
    use super::*;
    use crate::bindings::v4l2_buf_type;
    use std::collections::VecDeque;
    use std::ffi::CString;

    // A pipe with no reader produces a real libc POLLERR without a decoder
    // device or a mock of the production pump. All queue addresses are empty;
    // teardown ioctls on the pipe are harmless ENOTTY failures.
    fn session_on_error_pipe() -> V4l2Session {
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
        }
        let mut fds = [-1; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { super::super::close(fds[0]) }, 0);
        let (mut session, _backing) = session_with_backed_capture(fds[1]);
        for b in &mut session.cap.buffers {
            b.addr.fill(std::ptr::null_mut());
            b.len.fill(0);
        }
        session.legacy.clear();
        session.stable_capture = true;
        session.out.streaming = true;
        session.cap.streaming = true;
        session.cap.buffers[0].state = BufferState::Queued;
        let mut output = V4l2Buffer::new();
        output.state = BufferState::Queued;
        session.out.buffers.push(output);
        session.fifo.push(super::super::PendingFrame {
            surface: 9,
            timestamp: 7,
            expects_output: true,
        });
        session.no_output_waiting.push(10);
        session
    }

    #[test]
    fn fatal_poll_error_fails_pending_owners_without_reopening() {
        let mut session = session_on_error_pipe();
        let ready = session.pump(0);
        assert!(session.failed(), "active queue error must latch failure");
        assert_eq!(session.recoveries, 0, "terminal errors must not reopen");
        assert!(session.fifo.is_empty());
        assert!(session.no_output_waiting.is_empty());
        assert_eq!(ready.iter().map(|r| r.surface).collect::<Vec<_>>(), [9, 10]);
        assert!(
            ready
                .iter()
                .all(|r| r.failed && r.frame.is_none() && r.cap_idx.is_none())
        );
        assert!(
            session.pump(0).is_empty(),
            "failure publication must occur once"
        );
    }

    #[test]
    fn poll_error_before_capture_streaming_does_not_abort_startup() {
        let mut session = session_on_error_pipe();
        session.cap.streaming = false;
        assert!(session.pump(0).is_empty());
        assert!(!session.failed());
        assert_eq!(session.fifo.len(), 1);
    }

    #[test]
    fn poll_error_with_empty_queues_does_not_abort_idle_drain() {
        let mut session = session_on_error_pipe();
        session.out.buffers[0].state = BufferState::Free;
        session.cap.buffers[0].state = BufferState::Free;
        session.fifo.clear();
        session.no_output_waiting.clear();
        assert!(session.pump(0).is_empty());
        assert!(!session.failed());
    }

    #[test]
    fn terminal_poll_error_wins_over_an_armed_session_rebuild() {
        let mut session = session_on_error_pipe();
        session.aborted = true;
        assert_eq!(session.pump(0).len(), 2);
        assert!(session.failed());
        assert_eq!(session.recoveries, 0);
    }

    #[test]
    fn fatal_poll_error_is_detected_when_only_one_queue_still_has_buffers() {
        for output_is_empty in [true, false] {
            let mut session = session_on_error_pipe();
            if output_is_empty {
                session.out.buffers[0].state = BufferState::Free;
            } else {
                session.cap.buffers[0].state = BufferState::Free;
            }
            assert_eq!(session.pump(0).len(), 2);
            assert!(session.failed());
            assert_eq!(session.recoveries, 0);
        }
    }

    #[test]
    fn hung_up_poll_descriptor_fails_without_trying_session_recovery() {
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
        }
        let mut session = session_on_error_pipe();
        let mut fds = [-1; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        assert_eq!(unsafe { super::super::close(fds[1]) }, 0);
        assert_eq!(unsafe { super::super::close(session.fd) }, 0);
        session.fd = fds[0];
        session.out.streaming = false;
        session.cap.streaming = false;
        assert_eq!(session.pump(0).len(), 2);
        assert!(session.failed());
        assert_eq!(session.recoveries, 0);
    }

    #[test]
    fn invalid_poll_descriptor_fails_without_streaming_or_queued_work() {
        let mut session = session_on_error_pipe();
        assert_eq!(unsafe { super::super::close(session.fd) }, 0);
        // Far above the process descriptor limit, so parallel tests cannot
        // reuse this number between close and poll.
        session.fd = 1_000_000_000;
        session.out.streaming = false;
        session.cap.streaming = false;
        assert_eq!(session.pump(0).len(), 2);
        assert!(session.failed());
        assert_eq!(session.recoveries, 0);
    }

    #[test]
    fn streaming_source_change_is_acknowledged() {
        assert!(should_ack_source_change_flush(true));
        assert!(!should_ack_source_change_flush(false));
    }

    #[test]
    fn discarded_picture_errors_require_last_and_all_output_returned() {
        let (mut session, _backing) = session_with_backed_capture(-1);
        session.fifo.push(super::super::PendingFrame {
            surface: 9,
            timestamp: 7,
            expects_output: true,
        });
        session.no_output_waiting.push(10);
        let mut ready = Vec::new();
        session.finish_discarded_drain(&mut ready);
        session.draining = true;
        session.finish_discarded_drain(&mut ready);
        assert_eq!(session.fifo.len(), 1);
        assert_eq!(session.no_output_waiting, [10]);
        assert!(ready.is_empty());
        session.drain_last_seen = true;
        let mut output = V4l2Buffer::new();
        output.state = BufferState::Queued;
        session.out.buffers.push(output);
        session.finish_discarded_drain(&mut ready);
        assert_eq!(session.fifo.len(), 1);
        assert!(ready.is_empty());
        session.out.buffers[0].state = BufferState::Free;
        session.finish_discarded_drain(&mut ready);
        assert!(session.fifo.is_empty());
        assert!(session.no_output_waiting.is_empty());
        assert_eq!(ready.iter().map(|r| r.surface).collect::<Vec<_>>(), [9, 10]);
        assert!(
            ready
                .iter()
                .all(|r| r.failed && r.frame.is_none() && r.cap_idx.is_none())
        );
        // Synthetic CAPTURE addresses belong to backing vectors, not mmap.
        for b in &mut session.cap.buffers {
            b.addr.fill(std::ptr::null_mut());
            b.len.fill(0);
        }
        for p in &mut session.legacy {
            for b in &mut p.buffers {
                b.addr.fill(std::ptr::null_mut());
                b.len.fill(0);
            }
        }
    }

    #[test]
    fn source_change_markers_do_not_arm_firmware_abort() {
        assert!(source_change_marker_is_expected(true, false));
        assert!(!source_change_marker_is_expected(true, true));
        assert!(!source_change_marker_is_expected(false, false));
    }

    #[test]
    fn prior_drain_empty_marker_is_consumed_once_after_start() {
        let mut grace = true;
        assert!(!take_prior_drain_marker(&mut grace, true));
        assert!(grace);
        assert!(take_prior_drain_marker(&mut grace, false));
        assert!(!grace);
        assert!(!take_prior_drain_marker(&mut grace, false));
    }

    #[test]
    fn every_streaming_capture_resumes_drc_via_capture_cycle() {
        assert_eq!(drc_resume_mode(true), DrcResumeMode::CaptureCycle);
        assert_eq!(drc_resume_mode(false), DrcResumeMode::DecoderStart);
    }

    /// A synthetic session with one legacy slot and two backed live CAPTURE
    /// slots. The backing memory is heap-allocated and handed to the session
    /// as a raw pointer; the test must clear `addr`/`len` before dropping the
    /// session so teardown never unmaps it.
    fn session_with_backed_capture(fd: i32) -> (V4l2Session, Vec<Vec<u8>>) {
        let mut session = V4l2Session {
            fd,
            devnode: "/dev/null".to_string(),
            coded_fourcc: V4L2_PIX_FMT_H264,
            capture_fourcc: crate::pixel_format::DecodedFormat::Nv12.v4l2_fourcc(),
            out: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE as u32),
            cap: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as u32),
            legacy: Vec::new(),
            fifo: Vec::new(),
            ready: Vec::new(),
            no_output_waiting: Vec::new(),
            eos: false,
            draining: false,
            out_order: VecDeque::new(),
            aborted: false,
            source_change_flush: false,
            source_change_empty_seen: false,
            source_change_eos_seen: false,
            source_change_start_sent: false,
            drain_eos_grace: false,
            drain_empty_grace: false,
            drain_last_seen: false,
            abandoned: false,
            sync_drain_failures: 0,
            stable_capture: false,
            in_recover: false,
            recoveries: 0,
            headers: Vec::new(),
            replay_history: Vec::new(),
            published_timestamps: VecDeque::new(),
            next_submission_timestamp: 0,
            capture_metadata_ready: false,
        };
        session.legacy.push(LegacyPool {
            buffers: vec![V4l2Buffer::new()],
            width: 320,
            height: 240,
            stride: 320,
        });
        let mut backing = Vec::new();
        for _ in 0..2 {
            let mut b = V4l2Buffer::new();
            let mut mem = vec![0u8; 64];
            b.addr[0] = mem.as_mut_ptr() as *mut c_void;
            b.len[0] = 64;
            b.num_planes = 1;
            session.cap.buffers.push(b);
            // Moving the Vec header into `backing` does not move the heap
            // bytes, so the raw pointer stays valid for the test's lifetime.
            backing.push(mem);
        }
        (session, backing)
    }

    #[test]
    fn stable_publish_copies_working_slot_into_the_reserved_slot() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the publish test");

        let (mut session, _backing) = session_with_backed_capture(fd);
        session.stable_capture = true;
        session.cap.buffers.extend(
            (session.cap.buffers.len()..=super::super::WORKING_QUEUE_MAX)
                .map(|_| V4l2Buffer::new()),
        );
        // Legacy pool occupies client index zero; live slot zero becomes
        // surface 7's reservation (client index one).
        assert_eq!(session.reserve_capture(7), Some(1));

        // Stale reservation content; a completed firmware frame filling the
        // whole working-slot plane (live index one).
        unsafe {
            ptr::write_bytes(session.cap.buffers[0].addr[0] as *mut u8, 0x11, 64);
            ptr::write_bytes(session.cap.buffers[1].addr[0] as *mut u8, 0xAA, 64);
        }
        session.cap.buffers[1].planes[0].bytesused = 40;

        // The dequeued working slot (client index 2) publishes through
        // surface 7's reservation (client index 1). The copy refreshes the
        // whole mapped plane (min of the two lengths) so stale reservation
        // content cannot leak past the frame boundary.
        let dq_live = 1;
        let dq_cap_idx = session.legacy_len() + dq_live;
        let reserved = session.reserved_capture_for(7).unwrap();
        assert_ne!(reserved, dq_cap_idx);
        let copy_result = session.copy_capture_slot(dq_live, reserved - session.legacy_len());

        // Read every observable result into locals before handing the heap
        // backing back, so a failed assert can never leave raw heap pointers
        // in the session for Drop to "unmap".
        let (used, copied) = {
            let b = &session.cap.buffers[0];
            let data = unsafe { std::slice::from_raw_parts(b.addr[0] as *const u8, 64) };
            (b.planes[0].bytesused, data.to_vec())
        };
        let working_owned = session.cap.buffers[1].reserved_for;
        let working_free = matches!(session.cap.buffers[1].state, BufferState::Free);

        // Hand the heap backing back before Drop so teardown never unmaps it.
        for b in session.cap.buffers.iter_mut() {
            b.addr[0] = ptr::null_mut();
            b.len[0] = 0;
            b.num_planes = 0;
        }

        assert!(copy_result.is_ok());
        assert_eq!(used, 64);
        assert!(copied.iter().all(|&x| x == 0xAA));
        // The working slot keeps no owner and stays Free so the next working
        // top-up can requeue it.
        assert_eq!(working_owned, None);
        assert!(working_free);
    }
    #[test]
    fn failed_exported_copy_does_not_overwrite_or_publish_destination() {
        use std::os::fd::IntoRawFd;
        use std::os::unix::net::UnixStream;
        let fd = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
        let (mut session, backing) = session_with_backed_capture(fd);
        let (stream, _peer) = UnixStream::pair().unwrap();
        session.cap.buffers[0].sync_fd = Some(stream.into());
        unsafe {
            ptr::write_bytes(session.cap.buffers[1].addr[0].cast::<u8>(), 0xaa, 64);
        }
        let result = session.copy_capture_slot(1, 0);
        let published = session.cap.buffers[0].planes[0].bytesused;
        // Avoid allowing teardown to munmap heap storage on assertion failure.
        for b in &mut session.cap.buffers {
            b.addr[0] = ptr::null_mut();
            b.len[0] = 0;
        }
        assert!(result.is_err());
        assert_eq!(published, 0);
        assert!(backing[0].iter().all(|&value| value == 0));
    }

    #[test]
    fn capture_layout_rejects_invalid_or_undersized_geometry() {
        use crate::pixel_format::DecodedFormat::{Nv12, P010};
        assert_eq!(
            semiplanar_storage_size(1280, 736, 1280, Nv12),
            Some(1_413_120)
        );
        assert_eq!(
            semiplanar_storage_size(1280, 736, 2560, P010),
            Some(2_826_240)
        );
        assert_eq!(semiplanar_storage_size(1280, 736, 1280, P010), None);
        assert_eq!(semiplanar_storage_size(1280, 0, 1280, Nv12), None);
        assert_eq!(semiplanar_storage_size(1279, 736, 1280, Nv12), None);
    }
}
