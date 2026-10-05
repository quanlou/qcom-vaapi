//! OUTPUT submission and decoder drain.
//!
//! This module owns pacing, OUTPUT QBUF construction, and the explicit STOP
//! drain used at end of stream or teardown. Device polling remains in the
//! parent session runtime so submission and completion paths stay distinct.

use super::{
    BufferState, OUT_NUM_BUFFERS, V4L2_BUF_FLAG_KEYFRAME, V4L2_DEC_CMD_STOP, V4l2Session,
    VIDIOC_DECODER_CMD, VIDIOC_QBUF, debug_enabled, xioctl, zeroed,
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

// POLLOUT may stay ready even while firmware still owns the queued buffers.
// Counting poll calls therefore does not measure how long we waited.
const OUTPUT_PACING_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

impl V4l2Session {
    /// Pump without turning a permanently writable fd into an instant timeout
    /// or a busy loop. The same deadline covers pacing and free-slot waiting.
    fn pump_output_wait(&mut self, deadline: std::time::Instant) -> Result<(), ()> {
        if self.abandoned || std::time::Instant::now() >= deadline {
            return Err(());
        }
        let before = self.out_queued();
        let ready = self.pump(2);
        self.ready.extend(ready);
        if self.abandoned {
            return Err(());
        }
        if self.out_queued() >= before {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(1)));
        }
        Ok(())
    }

    fn remember_replay_chunk(
        &mut self,
        data: &[u8],
        timestamp_usec: u64,
        keyframe: bool,
        surface: Option<u32>,
        expects_output: bool,
    ) {
        if !super::replay::remember(
            &mut self.replay_history,
            data,
            timestamp_usec,
            keyframe,
            surface,
            expects_output,
        ) && debug_enabled()
        {
            eprintln!(
                "msm_drv_video_rs: replay history unavailable until next bounded keyframe GOP"
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn submit_frame(
        &mut self,
        surface: u32,
        cap_idx: Option<usize>,
        data: &[u8],
        keyframe: bool,
        expects_output: bool,
        _timestamp_usec: u64,
        headers: &[u8],
        direct_copy: bool,
    ) -> Result<(), ()> {
        if self.abandoned {
            return Err(());
        }
        // A previously armed abort means the device behind this session is
        // dead; rebuild before queuing anything else. If the rebuild already
        // failed permanently, fail this frame the way pre-recovery did.
        if self.aborted {
            self.recover()?;
        }
        // VP9 has no B-picture reordering: input for a different owner may
        // wait until completion selects its target. Its hidden/show_existing
        // pair uses the same owner and remains asynchronous. H264/HEVC need
        // following access units to complete reordered pictures; serializing
        // those codecs here would block their own required input.
        if self.coded_fourcc == super::V4L2_PIX_FMT_VP9 {
            self.wait_for_direct_target(
                surface,
                std::time::Instant::now() + OUTPUT_PACING_TIMEOUT,
            )?;
        }
        // An IDR can discard withheld pictures from the previous GOP. Finish
        // those owners first, including a seek's partially submitted GOP,
        // instead of treating the firmware's discarded-picture completions
        // as an abort and leaving their VA surfaces pending indefinitely.
        if keyframe && !self.draining && !self.eos && !self.fifo.is_empty() {
            // Ordinary pipelining leaves the previous GOP's last pictures
            // briefly pending. Give their existing decode a short chance to
            // finish before STOP: resetting every natural IDR adds needless
            // firmware pause/resume cycles and can stall later OUTPUT.
            // Two in-flight 4K pictures can take longer than five 2ms polls.
            // Allow a bounded elapsed grace before the compatibility drain;
            // real seeks still reach STOP when prior owners do not finish.
            let grace_deadline = std::time::Instant::now() + std::time::Duration::from_millis(100);
            while std::time::Instant::now() < grace_deadline {
                if self.cap.streaming {
                    self.queue_working_capture()?;
                }
                let ready = self.pump(2);
                self.ready.extend(ready);
                if self.fifo.is_empty() || self.aborted || self.abandoned {
                    break;
                }
                // POLLOUT can remain ready while CAPTURE is still decoding;
                // the poll timeout alone does not provide a wait here.
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            if self.aborted || self.abandoned {
                return Err(());
            }
            if !self.fifo.is_empty() && !self.maybe_start_sync_drain() {
                return Err(());
            }
        }
        // Drain recovery can queue a complete replay prefix. Do it before
        // pacing and waiting for a free OUTPUT slot for the new access unit.
        self.maybe_resume_after_drain(keyframe)?;
        if !headers.is_empty() {
            self.headers = headers.to_vec();
        }
        let output_deadline = std::time::Instant::now() + OUTPUT_PACING_TIMEOUT;
        // A VP9 packet can contain hidden input, its reference export, and a
        // visible picture. Use the existing four bounded OUTPUT slots for
        // the same VP9 owner's hidden input and synthetic reference export;
        // other codecs retain their asynchronous reordered-input pipeline.
        let inflight_limit = if self.direct_capture_mode() {
            super::OUT_NUM_BUFFERS as usize
        } else {
            output_inflight_limit(self.source_change_flush)
        };
        while self.out_queued() >= inflight_limit {
            if self.aborted && self.recover().is_ok() {
                continue;
            }
            if self.pump_output_wait(output_deadline).is_err() {
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: output pacing stall {}",
                        self.debug_snapshot()
                    );
                }
                return Err(());
            }
        }
        while self
            .out
            .buffers
            .iter()
            .all(|b| b.state != BufferState::Free)
        {
            if self.aborted && self.recover().is_ok() {
                continue;
            }
            if self.pump_output_wait(output_deadline).is_err() {
                if debug_enabled() {
                    eprintln!("msm_drv_video_rs: no free OUTPUT after pumping");
                }
                return Err(());
            }
        }
        if self.stable_capture {
            // Reserved slots must never enter the kernel queue (the firmware
            // would write someone else's frame into an exported dma-buf).
            // Only the unreserved working sub-pool is visible to the decoder;
            // topping it up here also recycles working slots completed by
            // earlier frames.
            self.queue_working_capture()?;
        } else if self.cap.streaming {
            // CPU snapshots let firmware reuse any unreserved working slot.
            // A bounded CPU queue needs the same full top-up as export mode;
            // requeueing only this target's old slot leaves other completed
            // slots Free and can starve a pipelined decoder.
            self.queue_all_capture()?;
        } else if let Some(cap_idx) = cap_idx {
            self.queue_capture(cap_idx)?;
        }
        let timestamp_usec = self.allocate_submission_timestamp()?;
        let idx = self
            .qbuf_output_bytes(
                data,
                keyframe,
                timestamp_usec,
                Some(surface),
                expects_output,
                true,
            )
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
        self.fifo.push(super::PendingFrame {
            surface,
            timestamp: timestamp_usec,
            expects_output,
            direct_copy,
        });
        let ready = self.pump(0);
        self.ready.extend(ready);
        Ok(())
    }

    fn allocate_submission_timestamp(&mut self) -> Result<u64, ()> {
        let timestamp = self.next_submission_timestamp;
        self.next_submission_timestamp = timestamp.checked_add(1).ok_or(())?;
        Ok(timestamp)
    }

    /// Copy `data` into a free OUTPUT buffer and queue it. Shared by normal
    /// submission and post-rebuild replay. Returns the buffer index.
    pub(super) fn qbuf_output_bytes(
        &mut self,
        data: &[u8],
        keyframe: bool,
        timestamp_usec: u64,
        surface: Option<u32>,
        expects_output: bool,
        remember: bool,
    ) -> Result<usize, ()> {
        let idx = self
            .out
            .buffers
            .iter()
            .position(|b| b.state == BufferState::Free)
            .ok_or(())?;
        if data.len() > self.out.buffers[idx].len[0] {
            return Err(());
        }
        self.map_buffer(true, idx)?;
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
                self.remember_replay_chunk(data, timestamp_usec, keyframe, surface, expects_output);
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

    fn maybe_resume_after_drain(&mut self, keyframe: bool) -> Result<(), ()> {
        if !self.eos && !self.draining {
            return Ok(());
        }
        // A completed picture does not complete STOP. In vb2, dequeuing a
        // LAST buffer sets last_buffer_dequeued and blocks later CAPTURE
        // dequeues with EPIPE. START clears that flag, so it must happen
        // after LAST has actually been dequeued, not merely after firmware
        // signalled completion. Otherwise a delayed LAST re-stops vb2.
        // STOP acceptance does not imply completion: sync can return as soon
        // as its requested surface is ready, with other owners still draining.
        // Finish their existing decode sequence before ownerless replay. Keep
        // completions queued for normal publication, never erase hidden owners.
        self.finish_drain_before_resume(std::time::Instant::now() + OUTPUT_PACING_TIMEOUT)?;
        // START destroys the decoder's references on this firmware. Check
        // the complete published prefix before changing the device state;
        // filtering out missing pictures can silently change later pixels.
        let replay = if keyframe {
            Vec::new()
        } else if let Some(prefix) =
            super::replay::drain_prefix(&self.replay_history, &self.published_timestamps)
        {
            prefix.to_vec()
        } else {
            self.abandoned = true;
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: drain resume lacks a complete keyframe reference chain"
                );
            }
            return Err(());
        };
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
            self.drain_empty_grace = true;
            self.drain_last_seen = false;
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after drain");
            }
        } else {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: DECODER_CMD START after drain failed");
            }
            return Err(());
        }

        // Random-access pictures replace the reference chain themselves.
        // Replaying the previous GOP wastes buffers and decode bandwidth.
        if keyframe {
            return Ok(());
        }

        // Publication copied the previous frame out of a working CAPTURE
        // slot and left that slot Free. Replay must replenish these slots
        // before OUTPUT pacing: the ordinary submit top-up happens only
        // after replay has completed, which is too late to unblock it.
        self.replenish_replay_capture()?;

        // STOP releases frames that the stateful decoder withheld while a
        // synchronous VA client had no more access units to submit. START
        // resumes an empty decode sequence on this hardware, so restore the
        // already-published prefix before accepting the next client frame.
        // Replayed frames have no FIFO owner and are discarded at dequeue;
        // their only purpose is to reconstruct the decoder reference state.

        // Bound total waiting across the whole GOP, rather than allowing a
        // fresh five-second wait for each of up to 1,024 access units.
        let replay_deadline = std::time::Instant::now() + OUTPUT_PACING_TIMEOUT;
        for chunk in &replay {
            self.wait_replay_output(replay_deadline)?;
            if self.aborted || self.abandoned {
                self.abandoned = true;
                return Err(());
            }
            if self
                .qbuf_output_bytes(
                    &chunk.data,
                    chunk.keyframe,
                    chunk.timestamp,
                    None,
                    chunk.expects_output,
                    false,
                )
                .is_err()
            {
                // START already discarded references. Never let the next
                // submission use a partially restored reference chain.
                self.abandoned = true;
                return Err(());
            }
        }
        if debug_enabled() && !replay.is_empty() {
            eprintln!(
                "msm_drv_video_rs: replayed {} published access units after drain",
                replay.len()
            );
        }
        Ok(())
    }

    fn finish_drain_before_resume(&mut self, deadline: std::time::Instant) -> Result<(), ()> {
        while (self.draining && !self.drain_last_seen)
            || !self.fifo.is_empty()
            || !self.no_output_waiting.is_empty()
            || self.out_queued() != 0
        {
            if self.aborted || self.abandoned {
                return Err(());
            }
            if std::time::Instant::now() >= deadline {
                // A slow healthy drain is retryable. Do not START or abandon
                // it just because its completion did not fit this wait budget.
                return Err(());
            }
            if self.stable_capture || self.cap.streaming {
                // Previous completions leave working slots Free. Queue only
                // unreserved slots; held/exported allocations remain untouched.
                self.queue_working_capture()?;
            }
            self.pump_output_wait(deadline)?;
        }
        if self.aborted || self.abandoned {
            return Err(());
        }
        Ok(())
    }

    fn wait_replay_output(&mut self, deadline: std::time::Instant) -> Result<(), ()> {
        while self.out_queued() >= output_inflight_limit(self.source_change_flush) {
            if self.aborted || self.abandoned || self.pump_output_wait(deadline).is_err() {
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: replay pacing stall {}",
                        self.debug_snapshot()
                    );
                }
                self.abandoned = true;
                return Err(());
            }
            self.replenish_replay_capture()?;
        }
        if self.aborted || self.abandoned {
            self.abandoned = true;
            return Err(());
        }
        Ok(())
    }

    /// Once START discarded references, a CAPTURE top-up failure makes the
    /// replay incomplete. No later ordinary submission may bypass it.
    fn replenish_replay_capture(&mut self) -> Result<(), ()> {
        if (self.stable_capture || self.cap.streaming) && self.queue_working_capture().is_err() {
            self.abandoned = true;
            return Err(());
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
            self.drain_last_seen = false;
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
        if self.draining || self.out_queued() != 0 || !self.out.streaming {
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
            self.drain_last_seen = false;
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
pub(super) mod tests {
    use super::super::{O_RDWR, V4l2Queue, open};
    use super::*;
    use std::collections::VecDeque;
    use std::ffi::CString;

    #[test]
    fn replay_writable_poll_waits_for_elapsed_budget_before_abandoning() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        for _ in 0..2 {
            let mut buffer = super::super::V4l2Buffer::new();
            buffer.state = BufferState::Queued;
            session.out.buffers.push(buffer);
        }
        let started = std::time::Instant::now();
        let budget = std::time::Duration::from_millis(30);
        assert!(session.wait_replay_output(started + budget).is_err());
        let elapsed = started.elapsed();
        eprintln!("replay pacing elapsed={elapsed:?}");
        assert!(
            elapsed >= budget,
            "writable poll readiness is not decode completion"
        );
        assert!(
            session.abandoned,
            "partial reference restoration must fail closed"
        );
        assert_eq!(session.out_queued(), 2);
    }

    #[test]
    fn incomplete_drain_waits_for_elapsed_budget_and_retains_owner() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.draining = true;
        let started = std::time::Instant::now();
        let budget = std::time::Duration::from_millis(30);
        assert!(
            session
                .finish_drain_before_resume(started + budget)
                .is_err()
        );
        let elapsed = started.elapsed();
        eprintln!("drain pacing elapsed={elapsed:?}");
        assert!(
            elapsed >= budget,
            "writable poll readiness cannot expire a healthy drain"
        );
        assert!(!session.abandoned, "pre-START timeout remains retryable");
        assert_eq!(session.fifo.len(), 1);
        assert!(session.draining);
        assert!(!session.drain_last_seen);
    }

    #[test]
    fn writable_fd_does_not_exhaust_output_wait_by_poll_count() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        for _ in 0..2 {
            let mut buffer = super::super::V4l2Buffer::new();
            buffer.state = BufferState::Queued;
            session.out.buffers.push(buffer);
        }
        session.out_order.extend([0, 1]);
        let started = std::time::Instant::now();
        let deadline = started + std::time::Duration::from_millis(30);
        let mut polls = 0;
        while session.pump_output_wait(deadline).is_ok() {
            polls += 1;
        }
        assert!(started.elapsed() >= std::time::Duration::from_millis(30));
        assert!(
            polls < 2500,
            "writable fd must yield between unsuccessful polls"
        );
        assert_eq!(session.out_queued(), 2);
        assert!(session.ready.is_empty());
        assert!(!session.draining);
        session.out.streaming = false;
    }

    #[test]
    fn output_wait_expired_deadline_returns_without_pumping() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        let deadline = std::time::Instant::now();
        assert!(session.pump_output_wait(deadline).is_err());
        assert_eq!(session.sync_drain_failures, 0);
        assert!(!session.draining);
    }

    #[test]
    fn source_change_flush_temporarily_uses_full_output_queue() {
        assert_eq!(output_inflight_limit(false), 2);
        assert_eq!(output_inflight_limit(true), OUT_NUM_BUFFERS as usize);
        assert!(output_inflight_limit(true) > output_inflight_limit(false));
    }

    /// A synthetic session on /dev/null with a streaming OUTPUT queue and one
    /// pending fifo entry: the minimum state for the compatibility drain to
    /// attempt a STOP (which /dev/null then rejects).
    pub(crate) fn streaming_session_with_pending_fifo(fd: i32) -> V4l2Session {
        let mut session = V4l2Session {
            fd,
            capture_drm_fd: None,
            devnode: "/dev/null".to_string(),
            coded_fourcc: super::super::V4L2_PIX_FMT_H264,
            capture_fourcc: crate::pixel_format::DecodedFormat::Nv12.v4l2_fourcc(),
            out: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE as u32),
            cap: V4l2Queue::new(v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as u32),
            direct_target: None,
            direct_targets: VecDeque::new(),
            decode_order: false,
            legacy: Vec::new(),
            fifo: Vec::new(),
            ready: Vec::new(),
            recycled_snapshot: None,
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
        session.out.streaming = true;
        session.fifo.push(super::super::PendingFrame {
            surface: 0,
            timestamp: 0,
            expects_output: true,
            direct_copy: false,
        });
        session
    }

    fn null_fd() -> i32 {
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the sync-drain test");
        fd
    }

    #[test]
    fn long_gop_retains_the_keyframe_and_all_published_dependencies() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        for timestamp in 0..300 {
            session.remember_replay_chunk(&[1], timestamp, timestamp == 0, Some(0), true);
            session.published_timestamps.push_back(timestamp);
        }
        assert_eq!(session.replay_history.len(), 300);
        assert!(session.replay_history[0].keyframe);
        assert_eq!(
            super::super::replay::drain_prefix(
                &session.replay_history,
                &session.published_timestamps,
            )
            .unwrap()
            .len(),
            300,
        );
        session.draining = true;
        session.drain_last_seen = true;
        // /dev/null rejects START; valid history must reach it without latching
        // a missing-reference error like the old 64-entry suffix did.
        assert!(session.maybe_resume_after_drain(false).is_err());
        assert!(!session.abandoned);
        assert!(session.draining);
    }

    #[test]
    fn completed_drain_reports_unpublished_owners_as_errors_before_resume() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.draining = true;
        session.drain_last_seen = true;
        assert!(session.maybe_resume_after_drain(true).is_err());
        assert!(!session.abandoned);
        assert!(session.fifo.is_empty());
        assert_eq!(session.ready.len(), 1);
        assert!(session.ready[0].failed);
        assert_eq!(session.ready[0].surface, 0);
        assert!(session.ready[0].frame.is_none());
        assert!(session.draining);
        let mut hidden = streaming_session_with_pending_fifo(null_fd());
        hidden.fifo.clear();
        hidden.no_output_waiting.push(7);
        hidden.draining = true;
        hidden.drain_last_seen = true;
        assert!(hidden.maybe_resume_after_drain(true).is_err());
        assert!(!hidden.abandoned);
        assert!(hidden.no_output_waiting.is_empty());
        assert_eq!(hidden.ready.len(), 1);
        assert!(hidden.ready[0].failed);
        assert_eq!(hidden.ready[0].surface, 7);
    }

    #[test]
    fn keyframe_submit_reports_discarded_hidden_owner_without_fabricating_pixels() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        session.no_output_waiting.push(7);
        session.draining = true;
        session.drain_last_seen = true;
        session.headers = vec![1];
        assert!(
            session
                .submit_frame(9, None, &[2], true, true, 9, &[3], false)
                .is_err()
        );
        assert!(session.no_output_waiting.is_empty());
        assert_eq!(session.ready.len(), 1);
        assert_eq!(session.ready[0].surface, 7);
        assert!(session.ready[0].failed);
        assert!(session.ready[0].frame.is_none());
        assert!(
            session.draining,
            "failed START must preserve the stopped state"
        );
        assert!(!session.abandoned, "an incomplete drain is retryable");
        assert_eq!(
            session.headers,
            [1],
            "unsubmitted keyframe cannot change recovery headers"
        );
        assert!(session.replay_history.is_empty());
        assert!(session.out_order.is_empty());
        // Simulate later completion: retry reaches START (/dev/null rejects
        // it) rather than being stopped by an abandoned-session guard.
        session.no_output_waiting.clear();
        assert!(
            session
                .submit_frame(9, None, &[2], true, true, 9, &[3], false)
                .is_err()
        );
        assert!(!session.abandoned);
    }

    #[test]
    fn replay_topup_failure_latches_session_and_preserves_export_reservation() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.stable_capture = true;
        let mut exported = super::super::V4l2Buffer::new();
        exported.state = BufferState::Reserved;
        exported.reserved_for = Some(7);
        exported.export_refs = 1;
        session.cap.buffers.push(exported);
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        // /dev/null rejects the working-slot QBUF after START. The reserved
        // exported allocation cannot enter the queue or change ownership.
        assert!(session.replenish_replay_capture().is_err());
        assert!(session.abandoned);
        let reserved = &session.cap.buffers[0];
        assert!(matches!(reserved.state, BufferState::Reserved));
        assert_eq!(reserved.reserved_for, Some(7));
        assert_eq!(reserved.export_refs, 1);
        assert!(matches!(session.cap.buffers[1].state, BufferState::Free));
    }

    #[test]
    fn drain_wait_topup_failure_preserves_owners_without_abandonment() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.stable_capture = true;
        session.draining = true;
        session.drain_last_seen = true;
        let mut exported = super::super::V4l2Buffer::new();
        exported.state = BufferState::Reserved;
        exported.reserved_for = Some(7);
        exported.export_refs = 1;
        session.cap.buffers.push(exported);
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        assert!(session.maybe_resume_after_drain(true).is_err());
        assert!(!session.abandoned, "START has not discarded the references");
        assert!(session.draining);
        assert_eq!(session.fifo.len(), 1);
        let reserved = &session.cap.buffers[0];
        assert!(matches!(reserved.state, BufferState::Reserved));
        assert_eq!(reserved.reserved_for, Some(7));
        assert_eq!(reserved.export_refs, 1);
    }

    #[test]
    fn abandoned_session_cannot_submit_even_without_abort_flag() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.abandoned = true;
        assert!(!session.aborted);
        assert!(
            session
                .submit_frame(0, None, &[1], true, true, 0, &[2], false)
                .is_err()
        );
        assert!(
            session.headers.is_empty(),
            "do not mutate or queue abandoned work"
        );
        assert!(session.replay_history.is_empty());
    }

    #[test]
    fn a_fresh_keyframe_does_not_require_old_reference_history() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        session.draining = true;
        session.drain_last_seen = true;
        // /dev/null rejects START, but the empty old history must not abandon
        // the session before that ioctl: this new picture replaces references.
        assert!(session.maybe_resume_after_drain(true).is_err());
        assert!(!session.abandoned);
    }

    #[test]
    fn keyframe_cannot_replace_pending_owners_when_stop_fails() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.headers = vec![1, 2, 3];
        assert!(
            session
                .submit_frame(7, None, &[9], true, true, 0, &[4], false)
                .is_err()
        );
        assert_eq!(session.sync_drain_failures, 1, "must try STOP before QBUF");
        assert_eq!(session.fifo.len(), 1);
        assert_eq!(session.fifo[0].surface, 0);
        assert_eq!(session.headers, [1, 2, 3]);
        assert_eq!(session.next_submission_timestamp, 0);
        assert!(session.replay_history.is_empty());
        assert!(!session.abandoned);
    }

    #[test]
    fn cpu_replay_topup_failure_latches_without_discarding_pending_owners() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.cap.streaming = true;
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        assert!(!session.stable_capture);
        assert!(session.replenish_replay_capture().is_err());
        assert!(session.abandoned);
        assert_eq!(session.fifo.len(), 1);
        assert_eq!(session.fifo[0].surface, 0);
    }

    #[test]
    fn cpu_topup_does_not_requeue_a_recycled_slots_foreign_reservation() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        session.fifo.clear();
        session.out.buffers.push(super::super::V4l2Buffer::new());
        session.cap.streaming = true;
        for _ in 0..super::super::WORKING_QUEUE_MAX {
            let mut buffer = super::super::V4l2Buffer::new();
            buffer.state = BufferState::Queued;
            session.cap.buffers.push(buffer);
        }
        let foreign_idx = session.cap.buffers.len();
        let mut foreign = super::super::V4l2Buffer::new();
        foreign.state = BufferState::Reserved;
        foreign.reserved_for = Some(99);
        foreign.export_refs = 1;
        session.cap.buffers.push(foreign);
        // The CPU surface has a detached snapshot; its former slot is now a
        // different surface's reservation. Top-up must use only working slots.
        // OUTPUT has no backing allocation, so the final QBUF cannot succeed.
        assert!(
            session
                .submit_frame(7, Some(foreign_idx), &[1], false, true, 0, &[], false)
                .is_err()
        );
        assert_eq!(
            session.next_submission_timestamp, 1,
            "CAPTURE top-up completed before the unavailable OUTPUT allocation"
        );
        let foreign = &session.cap.buffers[foreign_idx];
        assert!(matches!(foreign.state, BufferState::Reserved));
        assert_eq!(foreign.reserved_for, Some(99));
        assert_eq!(foreign.export_refs, 1);
    }

    #[test]
    fn completion_identity_survives_replay_and_refuses_wraparound() {
        let mut session = streaming_session_with_pending_fifo(null_fd());
        let old = session.allocate_submission_timestamp().unwrap();
        session.remember_replay_chunk(&[1], old, true, Some(0), true);
        session.published_timestamps.push_back(old);
        let fresh = session.allocate_submission_timestamp().unwrap();
        assert_ne!(
            old, fresh,
            "repeated media POC cannot alias a replay completion"
        );
        assert_eq!(session.replay_history[0].timestamp, old);
        session.next_submission_timestamp = u64::MAX;
        assert!(session.allocate_submission_timestamp().is_err());
        assert_eq!(session.next_submission_timestamp, u64::MAX);
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
        session.drain_last_seen = true;
        assert!(session.record_sync_drain_result(true));
        assert!(!session.drain_last_seen);
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
