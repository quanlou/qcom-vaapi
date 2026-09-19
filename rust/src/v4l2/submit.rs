//! OUTPUT submission and decoder drain.
//!
//! This module owns pacing, OUTPUT QBUF construction, and the explicit STOP
//! drain used at end of stream or teardown. Device polling remains in the
//! parent session runtime so submission and completion paths stay distinct.

use super::{
    BufferState, OUT_NUM_BUFFERS, ReplayChunk, V4L2_BUF_FLAG_KEYFRAME, V4L2_DEC_CMD_STOP,
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

/// Consecutive failed DECODER_CMD STOP attempts the sync-path compatibility
/// drain tolerates per session before the session is abandoned. A wedged
/// firmware rejects every STOP; without this budget the sync loop re-arms
/// the drain forever (~174k ioctls/s observed during the DRC-churn wedge).
const SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES: u32 = 8;

impl V4l2Session {
    fn remember_replay_chunk(
        &mut self,
        data: &[u8],
        timestamp_usec: u64,
        keyframe: bool,
        surface: Option<u32>,
    ) {
        if keyframe {
            self.replay_history.clear();
        }
        self.replay_history.push(ReplayChunk {
            data: data.to_vec(),
            timestamp: timestamp_usec,
            keyframe,
            surface,
        });
        const MAX_REPLAY_HISTORY: usize = 64;
        if self.replay_history.len() > MAX_REPLAY_HISTORY {
            let drop_count = self.replay_history.len() - MAX_REPLAY_HISTORY;
            self.replay_history.drain(..drop_count);
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
        self.maybe_resume_after_drain()?;
        if let Some(cap_idx) = cap_idx {
            self.queue_capture(cap_idx)?;
        } else if self.cap.streaming {
            self.queue_all_capture()?;
        }
        let idx = self
            .qbuf_output_bytes(data, keyframe, timestamp_usec, Some(surface), true)
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
        self.fifo.push((surface, timestamp_usec));
        let ready = self.pump(0);
        self.ready.extend(ready);
        Ok(())
    }

    /// Copy `data` into a free OUTPUT buffer and queue it. Shared by normal
    /// submission and post-rebuild replay. Returns the buffer index.
    pub(super) fn qbuf_output_bytes(
        &mut self,
        data: &[u8],
        keyframe: bool,
        timestamp_usec: u64,
        surface: Option<u32>,
        remember: bool,
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
            if remember {
                self.remember_replay_chunk(data, timestamp_usec, keyframe, surface);
            }
        } else if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: OUTPUT QBUF ioctl failed idx={} bytes={} queued_before={} err={}",
                idx,
                data.len(),
                self.out_order.len(),
                std::io::Error::last_os_error()
            );
        }
        res.map(|_| idx)
    }

    fn maybe_resume_after_drain(&mut self) -> Result<(), ()> {
        if !self.eos && !self.draining {
            return Ok(());
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
            self.eos = false;
            self.draining = false;
            self.drain_eos_grace = true;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after drain");
            }
        } else {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after drain failed");
            }
            return Err(());
        }

        // STOP releases frames that the stateful decoder withheld while a
        // synchronous VA client had no more access units to submit. START
        // resumes an empty decode sequence on this hardware, so restore the
        // already-published prefix before accepting the next client frame.
        // Replayed frames have no FIFO owner and are discarded at dequeue;
        // their only purpose is to reconstruct the decoder reference state.
        let replay: Vec<ReplayChunk> = self
            .replay_history
            .iter()
            .filter(|chunk| self.published_timestamps.contains(&chunk.timestamp))
            .cloned()
            .collect();
        for chunk in &replay {
            let mut attempts = 0;
            while self
                .out
                .buffers
                .iter()
                .all(|buffer| buffer.state != BufferState::Free)
            {
                attempts += 1;
                if attempts > 2500 {
                    return Err(());
                }
                let ready = self.pump(2);
                self.ready.extend(ready);
            }
            self.qbuf_output_bytes(&chunk.data, chunk.keyframe, chunk.timestamp, None, false)?;
        }
        if debug_enabled() && !replay.is_empty() {
            eprintln!(
                "msm_drv_video_rs: replayed {} published access units after drain",
                replay.len()
            );
        }
        Ok(())
    }

    /// Bookkeeping for a completed sync-drain STOP attempt: a success clears
    /// the failure streak and arms the drain; a failure grows the streak and,
    /// once it crosses the budget, abandons the session so the sync loop
    /// stops re-arming a drain the firmware will never accept.
    fn record_sync_drain_result(&mut self, started: bool) -> bool {
        if started {
            self.sync_drain_failures = 0;
            self.draining = true;
            return true;
        }
        self.sync_drain_failures = self.sync_drain_failures.saturating_add(1);
        if self.sync_drain_failures >= SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: DECODER_CMD STOP sync drain failed {} consecutive times; abandoning session",
                    self.sync_drain_failures
                );
            }
            self.abandoned = true;
        }
        false
    }

    pub(crate) fn maybe_start_sync_drain(&mut self) -> bool {
        if self.draining || self.fifo.is_empty() || !self.out.streaming {
            return false;
        }
        // The failure budget is already spent: issue no further STOP ioctls
        // so the sync loop terminates instead of spinning on a wedged
        // decoder.
        if self.sync_drain_failures >= SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES {
            return false;
        }
        let mut cmd: v4l2_decoder_cmd = zeroed();
        cmd.cmd = V4L2_DEC_CMD_STOP;
        let started = xioctl(
            self.fd,
            VIDIOC_DECODER_CMD,
            &mut cmd as *mut _ as *mut c_void,
        )
        .is_ok();
        if debug_enabled() {
            if started {
                eprintln!(
                    "msm_drv_video_rs: DECODER_CMD STOP sync drain started (pending={} out_queued={} history={})",
                    self.fifo.len(),
                    self.out_queued(),
                    self.replay_history.len()
                );
            } else {
                eprintln!(
                    "msm_drv_video_rs: DECODER_CMD STOP sync drain failed (streak={}/{})",
                    self.sync_drain_failures + 1,
                    SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES
                );
            }
        }
        self.record_sync_drain_result(started)
    }

    pub(crate) fn maybe_start_drain(&mut self) -> bool {
        if self.draining || self.fifo.is_empty() || self.out_queued() != 0 || !self.out.streaming {
            return false;
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
            true
        } else {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD STOP drain failed");
            }
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{O_RDWR, V4l2Queue, open};
    use super::*;
    use std::collections::VecDeque;
    use std::ffi::CString;

    #[test]
    fn source_change_flush_temporarily_uses_full_output_queue() {
        assert_eq!(output_inflight_limit(false), 2);
        assert_eq!(output_inflight_limit(true), OUT_NUM_BUFFERS as usize);
        assert!(output_inflight_limit(true) > output_inflight_limit(false));
    }

    /// A synthetic session on /dev/null with a streaming OUTPUT queue and one
    /// pending fifo entry: the minimum state for the compatibility drain to
    /// attempt a STOP (which /dev/null then rejects).
    fn streaming_session_with_pending_fifo(fd: i32) -> V4l2Session {
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
        session.out.streaming = true;
        session.fifo.push((0, 0));
        session
    }

    fn null_fd() -> i32 {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the sync-drain test");
        fd
    }

    #[test]
    fn sync_drain_exhaustion_abandons_the_session() {
        let fd = null_fd();
        let mut session = streaming_session_with_pending_fifo(fd);
        // /dev/null rejects every DECODER_CMD STOP, so each call exercises
        // the real failure branch.
        for _ in 0..(SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES - 1) {
            assert!(!session.maybe_start_sync_drain());
            assert!(
                !session.failed(),
                "a transient STOP failure must not kill the session"
            );
        }
        assert!(!session.maybe_start_sync_drain());
        assert_eq!(
            session.sync_drain_failures,
            SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES
        );
        assert!(
            session.failed(),
            "exhausting the retry budget must abandon the session"
        );

        // Once exhausted the budget guard refuses further attempts: the
        // counter can no longer grow and the drain is never re-armed.
        assert!(!session.maybe_start_sync_drain());
        assert_eq!(
            session.sync_drain_failures,
            SYNC_DRAIN_MAX_CONSECUTIVE_FAILURES
        );
        assert!(!session.draining);
    }

    #[test]
    fn sync_drain_success_resets_the_failure_budget_and_arms_the_drain() {
        let fd = null_fd();
        let mut session = streaming_session_with_pending_fifo(fd);
        session.sync_drain_failures = 3;
        assert!(session.record_sync_drain_result(true));
        assert_eq!(session.sync_drain_failures, 0);
        assert!(session.draining);
        assert!(!session.failed());

        // A later failure streak starts over from zero, not from the old one.
        assert!(!session.record_sync_drain_result(false));
        assert_eq!(session.sync_drain_failures, 1);
        assert!(!session.failed());
    }

    #[test]
    fn sync_drain_guard_refuses_to_spend_budget_without_pending_work() {
        let fd = null_fd();

        // No pending fifo entries: refuse before any STOP is attempted.
        let mut session = streaming_session_with_pending_fifo(fd);
        session.fifo.clear();
        assert!(!session.maybe_start_sync_drain());
        assert_eq!(session.sync_drain_failures, 0);

        // Idle OUTPUT queue: same.
        let mut session = streaming_session_with_pending_fifo(fd);
        session.out.streaming = false;
        assert!(!session.maybe_start_sync_drain());
        assert_eq!(session.sync_drain_failures, 0);
    }
}
