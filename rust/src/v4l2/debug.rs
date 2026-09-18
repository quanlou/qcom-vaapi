//! Read-only V4L2 session diagnostics.
//!
//! Timeout diagnostics must summarize bookkeeping without issuing ioctls or
//! changing queue ownership. Keeping that formatting separate makes the
//! session runtime easier to audit.

use super::V4l2Session;
use super::queue::{BufferState, V4l2Queue};

/// Point-in-time summary of session queue state for timeout diagnostics.
///
/// Collected without touching the device (no ioctls), so it is safe to build
/// while holding the driver lock on an error path.
struct SessionDebug {
    devnode: String,
    out_buffers: usize,
    out_queued: usize,
    out_streaming: bool,
    cap_buffers: usize,
    cap_queued: usize,
    cap_streaming: bool,
    fifo: usize,
    ready: usize,
    eos: bool,
    draining: bool,
    aborted: bool,
    abandoned: bool,
    recoveries: u32,
    legacy_pools: usize,
}

fn format_session_debug(d: &SessionDebug) -> String {
    format!(
        "dev={} out={}/{} streaming={} cap={}/{} streaming={} pending(fifo={},ready={}) eos={} draining={} aborted={} recov={}/{} legacy_pools={}",
        d.devnode,
        d.out_queued,
        d.out_buffers,
        d.out_streaming,
        d.cap_queued,
        d.cap_buffers,
        d.cap_streaming,
        d.fifo,
        d.ready,
        d.eos,
        d.draining,
        d.aborted,
        d.recoveries,
        if d.abandoned { "gave_up" } else { "ok" },
        d.legacy_pools,
    )
}

impl V4l2Session {
    /// One-line summary of the session's OUTPUT/CAPTURE queue bookkeeping.
    pub(crate) fn debug_snapshot(&self) -> String {
        let queued = |q: &V4l2Queue| {
            q.buffers
                .iter()
                .filter(|b| b.state == BufferState::Queued)
                .count()
        };
        format_session_debug(&SessionDebug {
            devnode: self.devnode.clone(),
            out_buffers: self.out.buffers.len(),
            out_queued: queued(&self.out),
            out_streaming: self.out.streaming,
            cap_buffers: self.cap.buffers.len(),
            cap_queued: queued(&self.cap),
            cap_streaming: self.cap.streaming,
            fifo: self.fifo.len(),
            ready: self.ready.len(),
            eos: self.eos,
            draining: self.draining,
            aborted: self.aborted,
            abandoned: self.abandoned,
            recoveries: self.recoveries,
            legacy_pools: self.legacy.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> SessionDebug {
        SessionDebug {
            devnode: "/dev/video16".to_string(),
            out_buffers: 16,
            out_queued: 2,
            out_streaming: true,
            cap_buffers: 32,
            cap_queued: 30,
            cap_streaming: true,
            fifo: 3,
            ready: 1,
            eos: false,
            draining: true,
            aborted: false,
            abandoned: false,
            recoveries: 0,
            legacy_pools: 0,
        }
    }

    #[test]
    fn formats_queue_counts_and_flags() {
        let s = format_session_debug(&snapshot());
        assert!(s.contains("dev=/dev/video16"), "got: {s}");
        assert!(s.contains("out=2/16"), "got: {s}");
        assert!(s.contains("cap=30/32"), "got: {s}");
        assert!(s.contains("pending(fifo=3,ready=1)"), "got: {s}");
        assert!(s.contains("eos=false"), "got: {s}");
        assert!(s.contains("draining=true"), "got: {s}");
        assert!(s.contains("aborted=false"), "got: {s}");
        assert!(s.contains("recov=0/ok"), "got: {s}");
        assert!(s.contains("legacy_pools=0"), "got: {s}");
    }

    #[test]
    fn shows_stopped_queues_and_idle_session() {
        let mut d = snapshot();
        d.out_streaming = false;
        d.cap_streaming = false;
        d.eos = true;
        d.draining = false;
        d.fifo = 0;
        d.ready = 0;
        let s = format_session_debug(&d);
        assert!(s.contains("streaming=false"), "got: {s}");
        assert!(s.contains("eos=true"), "got: {s}");
        assert!(s.contains("draining=false"), "got: {s}");
        assert!(s.contains("pending(fifo=0,ready=0)"), "got: {s}");
    }

    #[test]
    fn shows_recovery_state_after_abort() {
        let mut d = snapshot();
        d.aborted = true;
        d.recoveries = 2;
        d.legacy_pools = 2;
        let s = format_session_debug(&d);
        assert!(s.contains("aborted=true"), "got: {s}");
        assert!(s.contains("recov=2/ok"), "got: {s}");
        assert!(s.contains("legacy_pools=2"), "got: {s}");
        d.abandoned = true;
        let s = format_session_debug(&d);
        assert!(s.contains("recov=2/gave_up"), "got: {s}");
    }
}
