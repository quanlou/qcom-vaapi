//! CAPTURE queue mode selection.
//!
//! CPU-copy clients match completed working buffers to surfaces by timestamp.
//! Both modes keep an unqueued spare pool for a first post-decode export. Pre-decode
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
        if let Some(existing) = self.reserved_capture_for(surface) {
            return Some(existing);
        }
        let working = self
            .cap
            .buffers
            .iter()
            .filter(|b| b.reserved_for.is_none() && b.export_refs == 0)
            .count();
        if (working <= WORKING_QUEUE_MAX
            || !self.cap.buffers.iter().any(|b| {
                b.state == BufferState::Free && b.reserved_for.is_none() && b.export_refs == 0
            }))
            && self.grow_capture_pool().is_err()
        {
            return None;
        }
        let Some(idx) = self.cap.buffers.iter().position(|b| {
            b.state == BufferState::Free && b.reserved_for.is_none() && b.export_refs == 0
        }) else {
            if debug_enabled() {
                let queued = self
                    .cap
                    .buffers
                    .iter()
                    .filter(|b| b.state == BufferState::Queued)
                    .count();
                eprintln!(
                    "msm_drv_video_rs: stable reservation unavailable surface={} slots={} queued={}",
                    surface,
                    self.cap.buffers.len(),
                    queued
                );
            }
            return None;
        };
        self.stable_capture = true;
        if let Some(buffer) = self.cap.buffers.get_mut(idx) {
            buffer.state = BufferState::Reserved;
            buffer.reserved_for = Some(surface);
        }
        Some(self.legacy_len() + idx)
    }

    /// Late-binding stabilization for post-decode PRIME export (the Firefox
    /// pattern: `vaExportSurfaceHandle` arrives only through the frame
    /// callback, so the surface's frame lives in a shared working slot that
    /// the legacy flow already dequeued and handed back to the kernel).
    /// Reserve a fresh stable slot for `surface`, copy the completed frame
    /// into it, and return the new client-visible reservation index. A still
    /// `Free` published slot is adopted in place instead. `None` means the
    /// frame has no valid snapshot or live spare allocation: the caller must
    /// fail rather than hand out foreign bytes. With a validated snapshot the
    /// old slot may already belong to another surface; its mapping is ignored.
    /// Requeued working buffers belong to firmware and may already contain
    /// a later frame. Late exports must copy the dequeue-time CPU snapshot;
    /// they must never read a Queued mapping, even when the queue is deep.
    pub(crate) fn stabilize_published_capture(
        &mut self,
        published_idx: usize,
        surface: u32,
        snapshot: Option<&crate::state::SurfaceFrame>,
    ) -> Option<usize> {
        if !self.capture_is_live(published_idx) {
            return None;
        }
        if let Some(existing) = self.reserved_capture_for(surface) {
            // Repeated export is read-only. Never refresh an established
            // reservation from another published index, or release it while
            // unwinding a failed copy/cache operation. Importers can still
            // hold this allocation even when the incoming snapshot differs.
            return (existing == published_idx).then_some(existing);
        }
        let live = published_idx - self.legacy_len();
        let source_state = self.cap.buffers.get(live)?.state;
        if snapshot.is_none()
            && (source_state != BufferState::Free
                || self.cap.buffers[live].reserved_for.is_some()
                || self.cap.buffers[live].export_refs != 0)
        {
            return None;
        }
        if let Some(frame) = snapshot {
            let pix = unsafe { self.cap.fmt.fmt.pix_mp };
            let required = (pix.height as usize)
                .checked_add((pix.height as usize).div_ceil(2))
                .and_then(|rows| rows.checked_mul(pix.plane_fmt[0].bytesperline as usize));
            if frame.format.v4l2_fourcc() != self.capture_fourcc
                || frame.stride != pix.plane_fmt[0].bytesperline
                || frame.height != pix.height
                || required.is_none_or(|size| size == 0 || frame.data.len() < size)
            {
                return None;
            }
        }
        let reserved = self.reserve_capture(surface)?;
        if let Some(frame) = snapshot {
            let live_index = reserved - self.legacy_len();
            if self.map_buffer(false, live_index).is_err() {
                self.release_capture_reservation(reserved);
                return None;
            }
            let buffer = &mut self.cap.buffers[live_index];
            if buffer.addr[0].is_null() || frame.data.len() > buffer.len[0] {
                self.release_capture_reservation(reserved);
                return None;
            }
            use std::os::fd::AsRawFd;
            let Ok(access) = super::dmabuf::CpuWriteAccess::begin(
                buffer.sync_fd.as_ref().map(AsRawFd::as_raw_fd),
            ) else {
                self.release_capture_reservation(reserved);
                return None;
            };
            unsafe {
                std::ptr::copy_nonoverlapping(
                    frame.data.as_ptr(),
                    buffer.addr[0] as *mut u8,
                    frame.data.len(),
                );
                std::ptr::write_bytes(
                    (buffer.addr[0] as *mut u8).add(frame.data.len()),
                    0,
                    buffer.len[0] - frame.data.len(),
                );
            }
            if access.finish().is_err() {
                self.release_capture_reservation(reserved);
                return None;
            }
            buffer.planes[0].bytesused = frame.data.len() as u32;
        } else if reserved != published_idx {
            // Only a dequeued buffer can be read without a snapshot.
            if self
                .copy_capture_slot(live, reserved - self.legacy_len())
                .is_err()
            {
                self.release_capture_reservation(reserved);
                return None;
            }
        }
        Some(reserved)
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

    /// Supply the CPU-copy working queue while retaining reservation slack.
    /// Firefox only exports after a frame completes, so its first export must
    /// be able to transition from CPU-copy mode without stealing a slot from
    /// firmware or an earlier surface. Queueing the entire allocation here
    /// consumes the spare slots requested specifically for stable exports.
    pub(super) fn queue_all_capture(&mut self) -> Result<(), ()> {
        if self.stable_capture {
            return Err(());
        }
        self.queue_working_capture()
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
                && self.cap.buffers[idx].reserved_for.is_none()
                && self.cap.buffers[idx].export_refs == 0;
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
        session.cap.buffers.push(V4l2Buffer::new());
        session.legacy.push(super::super::LegacyPool {
            buffers: vec![V4l2Buffer::new()],
            width: 320,
            height: 240,
            stride: 320,
        });
        session
    }

    fn synthetic_reservation_slack(session: &mut V4l2Session) {
        // Ownership fixtures do not have a decoder capable of CREATE_BUFS.
        // Model a pool with the required working slack before reserving.
        let working = session
            .cap
            .buffers
            .iter()
            .filter(|b| b.reserved_for.is_none() && b.export_refs == 0)
            .count();
        session
            .cap
            .buffers
            .extend((working..=WORKING_QUEUE_MAX).map(|_| V4l2Buffer::new()));
    }

    #[test]
    fn append_failure_preserves_queued_slots_and_exports() {
        let mut session = session_with_unmapped_capture(-1);
        session.cap.buffers[0].state = BufferState::Queued;
        session.cap.buffers[0].reserved_for = Some(7);
        session.cap.buffers[0].export_refs = 3;
        let before = session.cap.buffers.len();
        assert!(session.grow_capture_pool().is_err());
        assert_eq!(session.cap.buffers.len(), before);
        assert!(session.cap.buffers[0].state == BufferState::Queued);
        assert_eq!(session.cap.buffers[0].reserved_for, Some(7));
        assert_eq!(session.cap.buffers[0].export_refs, 3);
    }

    #[test]
    fn repeated_stabilization_never_refreshes_or_releases_an_existing_export() {
        let mut session = session_with_unmapped_capture(-1);
        let addr = unsafe {
            super::super::mmap(
                std::ptr::null_mut(),
                4096,
                super::super::PROT_READ | super::super::PROT_WRITE,
                0x02 | 0x20,
                -1,
                0,
            )
        };
        assert_ne!(addr as isize, -1);
        unsafe { std::ptr::write_bytes(addr as *mut u8, 0x3C, 4096) };
        let reserved = &mut session.cap.buffers[0];
        reserved.state = BufferState::Reserved;
        reserved.reserved_for = Some(7);
        reserved.export_refs = 2;
        reserved.addr[0] = addr;
        reserved.len[0] = 4096;
        reserved.num_planes = 1;
        reserved.planes[0].bytesused = 384;
        session.cap.buffers.push(V4l2Buffer::new());
        session.cap.buffers[1].state = BufferState::Queued;
        let mut pix: v4l2_pix_format_mplane = super::super::zeroed();
        pix.height = 16;
        pix.plane_fmt[0].bytesperline = 16;
        session.cap.fmt.fmt.pix_mp = pix;
        let frame = crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![0xA5; 384]),
            stride: 16,
            height: 16,
            format: crate::pixel_format::DecodedFormat::Nv12,
        };
        // The surface already has live exported backing at client index1.
        // A different published index cannot replace or refresh it implicitly.
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(2, 7, Some(&frame)),
            None
        );
        // Repeating the export of the same backing is read-only/idempotent.
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(1, 7, Some(&frame)),
            Some(1)
        );
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.stabilize_published_capture(1, 7, None), Some(1));
        // A snapshot that would fail destination bounds must not undo an
        // earlier reservation or cache synchronization owned by an importer.
        let oversized = crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![0xA5; 8192]),
            ..frame
        };
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(2, 7, Some(&oversized)),
            None
        );
        let reserved = &session.cap.buffers[0];
        assert!(matches!(reserved.state, BufferState::Reserved));
        assert_eq!(reserved.reserved_for, Some(7));
        assert_eq!(reserved.export_refs, 2);
        assert_eq!(reserved.planes[0].bytesused, 384);
        assert_eq!(session.reserved_capture_for(7), Some(1));
        assert!(
            unsafe { std::slice::from_raw_parts(addr as *const u8, 4096) }
                .iter()
                .all(|byte| *byte == 0x3C)
        );
    }

    #[test]
    fn reservation_is_idempotent_for_a_live_surface_owner() {
        let mut session = session_with_unmapped_capture(-1);
        session.cap.buffers.push(V4l2Buffer::new());
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.reserve_capture(7), Some(1));
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.reserve_capture(7), Some(1));
        assert_eq!(session.cap.buffers[1].reserved_for, None);
        assert!(matches!(session.cap.buffers[1].state, BufferState::Free));
    }

    #[test]
    fn legacy_topup_keeps_slack_for_the_first_post_decode_export() {
        let mut session = session_with_unmapped_capture(-1);
        session.cap.buffers.resize_with(32, V4l2Buffer::new);
        for b in &mut session.cap.buffers[..WORKING_QUEUE_MAX] {
            b.state = BufferState::Queued;
        }
        // fd=-1 and unmapped buffers: an attempt to QBUF the spare pool fails.
        // A full working queue needs no ioctl and must leave export slack Free.
        assert!(session.queue_all_capture().is_ok());
        assert!(!session.stable_capture_mode());
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.reserve_capture(7), Some(WORKING_QUEUE_MAX + 1));
        assert_eq!(session.reserved_capture_for(7), Some(WORKING_QUEUE_MAX + 1));
        assert_eq!(
            session
                .cap
                .buffers
                .iter()
                .filter(|b| b.state == BufferState::Queued)
                .count(),
            WORKING_QUEUE_MAX
        );
    }

    #[test]
    fn delayed_snapshot_survives_its_old_slot_becoming_a_foreign_reservation() {
        let mut session = session_with_unmapped_capture(-1);
        session.cap.buffers[0].state = BufferState::Reserved;
        session.cap.buffers[0].reserved_for = Some(9);
        session.cap.buffers[0].export_refs = 2;
        session.cap.buffers.push(V4l2Buffer::new());
        let addr = unsafe {
            super::super::mmap(
                std::ptr::null_mut(),
                4096,
                super::super::PROT_READ | super::super::PROT_WRITE,
                0x02 | 0x20,
                -1,
                0,
            )
        };
        assert_ne!(addr as isize, -1);
        session.cap.buffers[1].addr[0] = addr;
        session.cap.buffers[1].len[0] = 4096;
        session.cap.buffers[1].num_planes = 1;
        let mut pix: v4l2_pix_format_mplane = super::super::zeroed();
        pix.height = 16;
        pix.plane_fmt[0].bytesperline = 16;
        session.cap.fmt.fmt.pix_mp = pix;
        let frame = crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![0xA5; 384]),
            stride: 16,
            height: 16,
            format: crate::pixel_format::DecodedFormat::Nv12,
        };
        // Never borrow bytes from the foreign reservation; only our snapshot
        // restores this frame into a different live backing allocation.
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.stabilize_published_capture(1, 7, None), None);
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(1, 7, Some(&frame)),
            Some(2)
        );
        assert_eq!(session.cap.buffers[0].reserved_for, Some(9));
        assert_eq!(session.cap.buffers[0].export_refs, 2);
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));
        assert_eq!(session.reserved_capture_for(7), Some(2));
        let restored = unsafe { std::slice::from_raw_parts(addr as *const u8, 4096) };
        assert_eq!(&restored[..384], frame.data.as_slice());
        assert!(restored[384..].iter().all(|byte| *byte == 0));
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
        synthetic_reservation_slack(&mut session);
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
        synthetic_reservation_slack(&mut session);
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

    #[test]
    fn stabilize_copies_a_recycled_working_slot_into_a_fresh_reservation() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the stabilize test");

        let mut session = session_with_unmapped_capture(fd);
        session.cap.buffers.push(V4l2Buffer::new());
        // Legacy publish already requeued the frame's working slot: client
        // index one is Queued again while still holding the frame bytes.
        session.cap.buffers[0].state = BufferState::Queued;
        assert!(!session.stable_capture_mode());

        // A queued source has no mapping in this fixture: the function must
        // use the snapshot, not read firmware-owned memory.
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.stabilize_published_capture(1, 7, None), None);
        let addr = unsafe {
            super::super::mmap(
                std::ptr::null_mut(),
                4096,
                super::super::PROT_READ | super::super::PROT_WRITE,
                0x02 | 0x20,
                -1,
                0,
            )
        };
        assert_ne!(addr as isize, -1);
        session.cap.buffers[1].addr[0] = addr;
        session.cap.buffers[1].len[0] = 4096;
        session.cap.buffers[1].num_planes = 1;
        let mut pix: v4l2_pix_format_mplane = super::super::zeroed();
        pix.height = 16;
        pix.plane_fmt[0].bytesperline = 16;
        session.cap.fmt.fmt.pix_mp = pix;
        let frame = crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![0xA5; 384]),
            stride: 16,
            height: 16,
            format: crate::pixel_format::DecodedFormat::Nv12,
        };
        let mut truncated = frame.clone();
        std::sync::Arc::make_mut(&mut truncated.data).pop();
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(1, 7, Some(&truncated)),
            None
        );
        assert!(!session.stable_capture_mode());
        assert_eq!(session.cap.buffers[1].reserved_for, None);
        assert!(
            unsafe { std::slice::from_raw_parts(addr as *const u8, 4096) }
                .iter()
                .all(|byte| *byte == 0)
        );
        synthetic_reservation_slack(&mut session);
        assert_eq!(
            session.stabilize_published_capture(1, 7, Some(&frame)),
            Some(2)
        );
        let copied = unsafe { std::slice::from_raw_parts(addr as *const u8, 4096) };
        assert_eq!(&copied[..384], frame.data.as_slice());
        assert!(copied[384..].iter().all(|byte| *byte == 0));
        assert!(session.stable_capture_mode());
        // The recycled source is untouched and the fresh reservation is
        // bound to the exporting surface only.
        assert!(matches!(session.cap.buffers[0].state, BufferState::Queued));
        assert!(matches!(
            session.cap.buffers[1].state,
            BufferState::Reserved
        ));
        assert_eq!(session.cap.buffers[1].reserved_for, Some(7));
        assert_eq!(session.reserved_capture_for(7), Some(2));

        // A second late export must stabilize its own old working slot even
        // though the first surface already enabled session-wide stable mode.
        let second_addr = unsafe {
            super::super::mmap(
                std::ptr::null_mut(),
                4096,
                super::super::PROT_READ | super::super::PROT_WRITE,
                0x02 | 0x20,
                -1,
                0,
            )
        };
        assert_ne!(second_addr as isize, -1);
        let mut second_buffer = V4l2Buffer::new();
        second_buffer.addr[0] = second_addr;
        second_buffer.len[0] = 4096;
        second_buffer.num_planes = 1;
        // Synthetic working slots are firmware-owned for this second export;
        // only the newly backed slot is a free reservation candidate.
        for buffer in &mut session.cap.buffers[2..] {
            buffer.state = BufferState::Queued;
        }
        let second_index = session.legacy_len() + session.cap.buffers.len();
        session.cap.buffers.push(second_buffer);
        use crate::state::{
            Context, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE, DriverBox, Surface, SurfaceState,
        };
        let driver = DriverBox::new();
        let mut guard = driver.lock.lock().unwrap();
        guard.contexts[0] = Some(Context {
            config_id: VA_INVALID_ID,
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 16,
            height: 16,
            render_targets: Vec::new(),
            frame_open: false,
            render_target: VA_INVALID_ID,
            decoder: crate::codec::Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
            out_seq: 0,
            v4l2: Some(session),
        });
        guard.surfaces[0] = Some(Surface {
            backing: None,
            width: 16,
            height: 16,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Ready,
            cap_idx: Some(1),
            frame: Some(frame),
            owner: DRV_ID_BASE_CONTEXT,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });
        // The public export now owns independent storage. Export the saved
        // pixels without consuming or relabeling another decoder CAPTURE slot.
        let descriptor = crate::surface_export::export_ready_surface_for_test(
            &mut guard,
            DRV_ID_BASE_SURFACE,
            crate::va_drm::DrmPrimeLayout::Composed,
        )
        .unwrap();
        assert_eq!(guard.surfaces[0].as_ref().unwrap().cap_idx, Some(1));
        let session = guard.contexts[0].as_ref().unwrap().v4l2.as_ref().unwrap();
        assert_eq!(session.reserved_capture_for(DRV_ID_BASE_SURFACE), None);
        assert_eq!(
            session.legacy_len() + session.cap.buffers.len() - 1,
            second_index
        );
        assert_eq!(session.cap.buffers.last().unwrap().reserved_for, None);
        use std::os::fd::FromRawFd;
        use std::os::unix::fs::FileExt;
        let exported = unsafe { std::fs::File::from_raw_fd(descriptor.objects[0].fd) };
        let mut row = [0u8; 16];
        for y in 0..16u64 {
            exported
                .read_exact_at(&mut row, y * u64::from(descriptor.layers[0].pitch[0]))
                .unwrap();
            assert!(row.iter().all(|byte| *byte == 0xA5));
        }
        let copied = unsafe { std::slice::from_raw_parts(second_addr as *const u8, 384) };
        assert!(copied.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn stabilize_adopts_a_still_free_published_slot_in_place() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the adopt test");

        let mut session = session_with_unmapped_capture(fd);
        // Dequeued but not yet requeued: the published slot itself is still
        // Free and unreserved, so the reservation adopts it without a copy
        // and no second slot is consumed.
        synthetic_reservation_slack(&mut session);
        let allocation_count = session.cap.buffers.len();
        assert_eq!(session.stabilize_published_capture(1, 7, None), Some(1));
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));
        assert_eq!(session.cap.buffers[0].reserved_for, Some(7));
        assert_eq!(session.cap.buffers.len(), allocation_count);
    }

    #[test]
    fn stabilize_refuses_slots_owned_by_another_surface() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(
            fd >= 0,
            "could not open /dev/null for the foreign-slot test"
        );

        let mut session = session_with_unmapped_capture(fd);
        session.cap.buffers[0].state = BufferState::Reserved;
        session.cap.buffers[0].reserved_for = Some(9);

        synthetic_reservation_slack(&mut session);
        assert_eq!(session.stabilize_published_capture(1, 7, None), None);
        // The refusal must not flip the session into stable capture.
        assert!(!session.stable_capture_mode());
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));
        assert_eq!(session.cap.buffers[0].reserved_for, Some(9));
    }

    #[test]
    fn stabilize_ignores_legacy_pool_indices() {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the legacy test");

        let mut session = session_with_unmapped_capture(fd);
        // The synthetic legacy pool owns client index zero.
        synthetic_reservation_slack(&mut session);
        assert_eq!(session.stabilize_published_capture(0, 7, None), None);
        assert!(!session.stable_capture_mode());
    }
}
