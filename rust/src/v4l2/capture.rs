//! CAPTURE queue mode selection.
//!
//! CPU-copy clients expect the old V4L2 model: keep the whole CAPTURE queue
//! supplied and match completed buffers to surfaces by timestamp. Pre-decode
//! PRIME export needs a stricter model where one VA surface owns one CAPTURE
//! slot before submission, so an exported dma-buf keeps backing the same
//! surface. This module owns that split.
//!
//! Stable-capture slots are a reservation pool, not decoder targets. This
//! firmware chooses its own CAPTURE buffer for every decoded frame, so a
//! queued reservation receives someone else's frame. Export identity therefore
//! requires that reserved slots never be queued: the firmware decodes into
//! unreserved "working" slots, and `dequeue_capture` copies the completed
//! working slot into the owning surface's reservation before publishing it.

use super::{BufferState, V4l2Session, WORKING_QUEUE_MAX, debug_enabled};

impl V4l2Session {
    /// Whether a client-visible CAPTURE index belongs to this active queue
    /// incarnation. Published surfaces from a recovered session can still
    /// point into a legacy pool and must be rebound before reuse.
    pub(crate) fn capture_is_live(&self, idx: usize) -> bool {
        let base = self.legacy_len();
        idx >= base && idx - base < self.cap.buffers.len()
    }

    pub(crate) fn stable_capture_mode(&self) -> bool {
        self.stable_capture
    }

    /// Return a live CAPTURE slot as the stable reservation of `surface`.
    /// The slot never enters the kernel queue; the client may export it
    /// immediately, and `dequeue_capture` later copies the completed working
    /// slot into it before publishing.
    pub(crate) fn reserve_capture(&mut self, surface: u32) -> Option<usize> {
        if self.cap.buffers.is_empty() && self.capture_pool_setup().is_err() {
            return None;
        }
        let idx = self
            .cap
            .buffers
            .iter()
            .position(|b| b.state == BufferState::Free && b.reserved_for.is_none())?;
        self.stable_capture = true;
        if let Some(buffer) = self.cap.buffers.get_mut(idx) {
            buffer.state = BufferState::Reserved;
            buffer.reserved_for = Some(surface);
        }
        Some(self.legacy_len() + idx)
    }

    /// The client-visible reservation slot owned by `surface`, if it is
    /// still live and reserved.
    pub(crate) fn reserved_capture_for(&self, surface: u32) -> Option<usize> {
        let base = self.legacy_len();
        self.cap
            .buffers
            .iter()
            .position(|b| b.reserved_for == Some(surface) && b.state == BufferState::Reserved)
            .map(|idx| base + idx)
    }

    /// Queue every currently free CAPTURE buffer for the legacy CPU-copy
    /// path. DQBUF returns slots to `Free`, so later submissions top the
    /// queue back up without disturbing the timestamp-to-surface mapping.
    pub(super) fn queue_all_capture(&mut self) -> Result<(), ()> {
        if self.stable_capture {
            return Err(());
        }
        for idx in 0..self.cap.buffers.len() {
            if self.cap.buffers[idx].state == BufferState::Free {
                self.qbuf_capture(idx)?;
            }
        }
        Ok(())
    }

    /// Queue free unreserved ("working") CAPTURE buffers up to
    /// `WORKING_QUEUE_MAX`. This is the stable-capture counterpart of
    /// `queue_all_capture`: reserved slots must stay out of the kernel queue
    /// so the firmware cannot write over an exported dma-buf, while completed
    /// working slots are recycled here. Chromium exports its whole 22-frame
    /// pool one surface at a time and interleaves exports with decode; if
    /// every Free slot were queued after the first submit, later exports
    /// would have nothing left to reserve.
    pub(super) fn queue_working_capture(&mut self) -> Result<(), ()> {
        let mut queued = self
            .cap
            .buffers
            .iter()
            .filter(|b| b.state == BufferState::Queued)
            .count();
        if queued >= WORKING_QUEUE_MAX {
            return Ok(());
        }
        for idx in 0..self.cap.buffers.len() {
            if queued >= WORKING_QUEUE_MAX {
                break;
            }
            let workable = self.cap.buffers[idx].state == BufferState::Free
                && self.cap.buffers[idx].reserved_for.is_none();
            if workable {
                self.qbuf_capture(idx)?;
                queued += 1;
            }
        }
        Ok(())
    }

    /// Queue the CAPTURE slot identified by the client-visible index
    /// (legacy CPU-copy mode only; stable reservations are never queued).
    /// Indices from legacy pools cannot be queued after a rebuild.
    pub(super) fn queue_capture(&mut self, idx: usize) -> Result<(), ()> {
        let base = self.legacy_len();
        if idx < base {
            return Err(());
        }
        self.qbuf_capture(idx - base)
    }

    /// Release a reservation that was never queued. A completed CAPTURE
    /// buffer is already `Free` and needs no transition here.
    pub(super) fn release_capture_reservation(&mut self, idx: usize) {
        let base = self.legacy_len();
        if idx < base {
            return;
        }
        if let Some(buffer) = self.cap.buffers.get_mut(idx - base)
            && buffer.state == BufferState::Reserved
        {
            buffer.state = BufferState::Free;
            buffer.reserved_for = None;
        }
    }

    /// Resume a completed source-change (DRC) flush by cycling CAPTURE
    /// through STREAMOFF/STREAMON instead of a bare `DECODER_CMD START`. The
    /// kernel's CAPTURE-streamon path re-queues the decoder's input-internal
    /// buffers and re-applies STAGE/PIPE before resuming the DRC; the bare
    /// START path does neither, which leaves the firmware silently unable to
    /// consume OUTPUT in stable-capture sessions. Only slots that were queued
    /// before the cycle are requeued, so slot ownership, mapped planes, and
    /// exported dma-bufs are untouched (no REQBUFS, no realloc). Returns
    /// false when the queue is not streaming or the cycle fails; the caller
    /// then falls back to the bare START.
    pub(super) fn resume_drc_capture_cycle(&mut self) -> bool {
        if !self.cap.streaming {
            return false;
        }
        let queued: Vec<usize> = (0..self.cap.buffers.len())
            .filter(|&i| self.cap.buffers[i].state == BufferState::Queued)
            .collect();
        Self::stream_off_fd(self.fd, &mut self.cap);
        for &i in &queued {
            if let Some(b) = self.cap.buffers.get_mut(i) {
                b.state = BufferState::Free;
            }
        }
        let requeued = queued
            .iter()
            .filter(|&&i| self.qbuf_capture(i).is_ok())
            .count();
        if self.stream_on(false).is_err() {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DRC CAPTURE cycle STREAMON failed");
            }
            return false;
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: DRC resumed via CAPTURE cycle queued={} requeued={}",
                queued.len(),
                requeued
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::{O_RDWR, V4l2Buffer, V4l2Queue, open};
    use super::*;
    use crate::bindings::*;
    use std::collections::VecDeque;
    use std::ffi::CString;

    fn session_with_unmapped_capture(fd: i32) -> V4l2Session {
        let mut session = V4l2Session {
            fd,
            devnode: "/dev/null".to_string(),
            coded_fourcc: super::super::V4L2_PIX_FMT_H264,
            capture_fourcc: crate::pixel_format::DecodedFormat::Nv12.v4l2_fourcc(),
            out: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE as u32),
            cap: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as u32),
            legacy: Vec::new(),
            fifo: Vec::new(),
            ready: Vec::new(),
            eos: false,
            draining: false,
            out_order: VecDeque::new(),
            aborted: false,
            source_change_flush: false,
            source_change_empty_seen: false,
            source_change_eos_seen: false,
            source_change_start_sent: false,
            drain_eos_grace: false,
            abandoned: false,
            sync_drain_failures: 0,
            stable_capture: false,
            in_recover: false,
            recoveries: 0,
            headers: Vec::new(),
            replay_history: Vec::new(),
            published_timestamps: VecDeque::new(),
        };
        session.cap.buffers.push(V4l2Buffer::new());
        session.legacy.push(super::super::LegacyPool {
            buffers: vec![V4l2Buffer::new()],
            width: 320,
            height: 240,
            stride: 320,
        });
        session
    }

    #[test]
    fn predecode_export_reserves_a_live_capture_slot() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the reservation test");

        let mut session = session_with_unmapped_capture(fd);
        assert!(!session.stable_capture_mode());
        // The synthetic legacy pool occupies client-visible index zero, so
        // the first live slot must be returned at index one.
        assert_eq!(session.reserve_capture(7), Some(1));
        assert!(session.stable_capture_mode());
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));
        assert_eq!(session.cap.buffers[0].reserved_for, Some(7));
        // The owner is resolvable, and other surfaces cannot see the slot.
        assert_eq!(session.reserved_capture_for(7), Some(1));
        assert_eq!(session.reserved_capture_for(9), None);

        session.release_capture_reservation(1);
        assert!(matches!(session.cap.buffers[0].state, BufferState::Free));
        assert_eq!(session.cap.buffers[0].reserved_for, None);
    }

    #[test]
    fn stable_queue_topup_feeds_only_unreserved_working_slots() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(
            fd >= 0,
            "could not open /dev/null for the working-pool test"
        );

        let mut session = session_with_unmapped_capture(fd);
        // The builder leaves one live slot; add a second so a reservation
        // and a working slot can coexist.
        session.cap.buffers.push(V4l2Buffer::new());
        // Live slot one (first free) becomes surface 7's reservation; live
        // slot two stays a working slot.
        assert_eq!(session.reserve_capture(7), Some(1));

        // The top-up only ever offers the free working slot to the kernel;
        // /dev/null rejects QBUF, which surfaces as an error while the
        // reservation itself must remain untouched and unqueued (queued
        // reservations are what let the firmware overwrite exported
        // dma-bufs).
        assert!(session.queue_working_capture().is_err());
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));
        assert_eq!(session.cap.buffers[0].reserved_for, Some(7));
        assert_eq!(session.reserved_capture_for(7), Some(1));
        // The rejected QBUF leaves the working slot Free for the next top-up.
        assert!(matches!(session.cap.buffers[1].state, BufferState::Free));
    }

    #[test]
    fn working_queue_topup_stops_at_working_queue_max() {
        // Chromium exports its 22-frame pool one surface at a time and
        // interleaves exports with decode; each submit calls
        // `queue_working_capture`. If it queued every Free unreserved slot,
        // later exports would find no unreserved slot left and
        // `reserve_capture` would fail. The cap keeps the pipeline fed while
        // leaving Free unreserved slots available for future reservations.
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the cap test");

        let mut session = session_with_unmapped_capture(fd);
        // The builder gives us one live slot; add enough to exceed
        // WORKING_QUEUE_MAX. All slots start Free unreserved, which is the
        // shape of a freshly bound stable-capture pool immediately after
        // reservations retire (destroy/release path).
        for _ in 0..12 {
            session.cap.buffers.push(V4l2Buffer::new());
        }
        // Pre-load the queue with WORKING_QUEUE_MAX slots to prove the
        // top-up is a no-op once the cap is met: the loop's early return
        // must fire before any qbuf attempt reaches /dev/null (which would
        // return Err and abort the top-up mid-scan).
        for idx in 0..WORKING_QUEUE_MAX {
            session.cap.buffers[idx].state = BufferState::Queued;
        }
        assert!(session.queue_working_capture().is_ok());
        // The remaining slots must stay Free so a later `reserve_capture`
        // can grab them.
        for idx in WORKING_QUEUE_MAX..session.cap.buffers.len() {
            assert!(
                matches!(session.cap.buffers[idx].state, BufferState::Free),
                "slot {} was queued past the WORKING_QUEUE_MAX cap",
                idx
            );
        }
    }

    #[test]
    fn drc_capture_cycle_requires_a_streaming_capture_queue() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the DRC cycle test");

        let mut session = session_with_unmapped_capture(fd);
        assert!(!session.cap.streaming);
        // Not streaming means the bare START fallback is correct; the cycle
        // must refuse without touching the queue.
        assert!(!session.resume_drc_capture_cycle());
        assert!(matches!(session.cap.buffers[0].state, BufferState::Free));
    }

    #[test]
    fn export_refcount_accounts_only_live_slots() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the refcount test");

        let mut session = session_with_unmapped_capture(fd);
        // Legacy indices are ignored: their kernel queue is gone and no
        // future requeue will observe their counters.
        session.retire_slot_exports(0, 3);
        assert_eq!(session.cap.buffers[0].export_refs, 0);
        // Zero-retire and out-of-range indices are no-ops.
        session.retire_slot_exports(1, 0);
        session.retire_slot_exports(99, 2);
        assert_eq!(session.cap.buffers[0].export_refs, 0);
        // Driver-side retire decrements by the number of closed dups.
        session.cap.buffers[0].export_refs = 5;
        session.retire_slot_exports(1, 2);
        assert_eq!(session.cap.buffers[0].export_refs, 3);
        // The decrement saturates: a rebound surface may carry dups counted
        // against an older slot incarnation.
        session.retire_slot_exports(1, 10);
        assert_eq!(session.cap.buffers[0].export_refs, 0);
    }

    #[test]
    fn requeue_keeps_export_accounting_and_frees_reserved_slots() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the requeue test");

        let mut session = session_with_unmapped_capture(fd);
        // A reserved slot returns to Free without a kernel QBUF, even with
        // outstanding exports (the stable-recycle contract allows overwrite
        // once the surface is released).
        session.cap.buffers[0].export_refs = 2;
        session.cap.buffers[0].state = BufferState::Reserved;
        session.requeue_capture(1);
        assert!(matches!(session.cap.buffers[0].state, BufferState::Free));
        // Releasing the reservation drops its owner too: a freed slot must
        // never stay bound to a surface that no longer exists.
        assert_eq!(session.cap.buffers[0].reserved_for, None);
        // The requeue itself never mutates the counter; retirement is
        // explicit via `retire_slot_exports` on the release path.
        assert_eq!(session.cap.buffers[0].export_refs, 2);
        session.retire_slot_exports(1, 2);
        assert_eq!(session.cap.buffers[0].export_refs, 0);
    }
}
