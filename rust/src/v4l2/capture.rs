//! CAPTURE queue mode selection.
//!
//! CPU-copy clients expect the old V4L2 model: keep the whole CAPTURE queue
//! supplied and match completed buffers to surfaces by timestamp. Pre-decode
//! PRIME export needs a stricter model where one VA surface owns one CAPTURE
//! slot before submission, so an exported dma-buf keeps backing the same
//! surface. This module owns that split.

use super::{BufferState, V4l2Session};

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

    /// Return a live CAPTURE slot before it is queued. The caller can export
    /// this slot immediately; `queue_capture` later puts the same slot in the
    /// kernel queue for the VA surface that owns it.
    pub(crate) fn reserve_capture(&mut self) -> Option<usize> {
        let idx = self
            .cap
            .buffers
            .iter()
            .position(|b| b.state == BufferState::Free)?;
        self.stable_capture = true;
        self.cap.buffers[idx].state = BufferState::Reserved;
        Some(self.legacy_len() + idx)
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

    /// Queue the reserved CAPTURE slot identified by the client-visible
    /// index. Indices from legacy pools cannot be queued after a rebuild.
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
        }
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
            abandoned: false,
            stable_capture: false,
            in_recover: false,
            recoveries: 0,
            headers: Vec::new(),
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
        assert_eq!(session.reserve_capture(), Some(1));
        assert!(session.stable_capture_mode());
        assert!(matches!(
            session.cap.buffers[0].state,
            BufferState::Reserved
        ));

        session.release_capture_reservation(1);
        assert!(matches!(session.cap.buffers[0].state, BufferState::Free));
    }
}
