use crate::bindings::*;
use std::collections::VecDeque;
use std::ffi::{CString, c_int, c_void};
use std::ptr;

mod abi;
mod capture;
mod debug;
mod poll;
mod queue;
mod recovery;
mod setup;
mod submit;
mod teardown;
use abi::*;
pub(crate) use abi::{
    V4L2_PIX_FMT_AV1, V4L2_PIX_FMT_H264, V4L2_PIX_FMT_HEVC, V4L2_PIX_FMT_P010, V4L2_PIX_FMT_VP9,
};
use queue::{BufferState, V4l2Buffer, V4l2Queue};

const OUT_NUM_BUFFERS: u32 = 16;
const CAP_NUM_BUFFERS_MIN: u32 = 20;
const CAP_NUM_BUFFERS_MAX: u32 = 128;
const CAP_EXTRA_BUFFERS: u32 = 28;
/// Cap on how many "working" (non-reserved) CAPTURE slots stay in the kernel
/// queue at once, in stable-capture mode. Chromium exports its whole 22-frame
/// pool one surface at a time and interleaves exports with decode; if the
/// first submit queues every Free slot, later exports have nothing left to
/// reserve. Keeping at most WORKING_QUEUE_MAX slots queued keeps the pipeline
/// fed while leaving Free slots for future reservations.
const WORKING_QUEUE_MAX: usize = 6;
/// Maximum transparent session rebuilds per session. One rebuild rescues a
/// one-off abort on an otherwise healthy device. If the rebuilt session also
/// aborts, the device is wedged at the firmware level (repeated aborted
/// teardowns poison even native decoders), so more opens only dig deeper.
const MAX_SESSION_RECOVERIES: u32 = 1;
/// Single funnel for releasing a plane mapping, so tests can assert that
/// teardown unmaps every region exactly once.
fn release_mapping(addr: *mut c_void, length: usize) {
    unsafe { munmap(addr, length) };
    #[cfg(test)]
    {
        UNMAPPED_PLANES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[cfg(test)]
static UNMAPPED_PLANES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[derive(Clone)]
pub(crate) struct ReadyCapture {
    pub(crate) surface: u32,
    pub(crate) cap_idx: Option<usize>,
    /// Pixels copied at dequeue time. CAPTURE slots are recycled as soon as
    /// they are requeued, so late surface reads must use this snapshot rather
    /// than the slot's live mapping.
    pub(crate) frame: Option<crate::state::SurfaceFrame>,
}

#[derive(Clone)]
struct PendingFrame {
    surface: u32,
    timestamp: u64,
    expects_output: bool,
}

#[derive(Clone)]
struct ReplayChunk {
    data: Vec<u8>,
    timestamp: u64,
    keyframe: bool,
    surface: Option<u32>,
    expects_output: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct CaptureExport {
    pub(crate) fd: c_int,
    pub(crate) size: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride: u32,
    pub(crate) y_offset: u32,
    pub(crate) uv_offset: u32,
    /// CAPTURE surface format (NV12 for 8-bit YUV420, P010 for 10-bit).
    /// Callers building the DRM PRIME descriptor need this to pick the
    /// right per-layer DRM fourcc without re-parsing the raw V4L2 fourcc.
    pub(crate) format: crate::pixel_format::DecodedFormat,
}

/// CAPTURE pool from a previous device incarnation whose decoded frames may
/// still be referenced by published VA surfaces. The kernel queue behind it is
/// gone, but the mmap'd planes stay valid: vb2 dma-contig buffers are dma-bufs
/// and each mapping pins its buffer, so pixels remain readable until the
/// mappings are dropped in `Drop`. Indices into legacy pools occupy the low
/// virtual `cap_idx` space; the live pool is offset by their total length.
struct LegacyPool {
    buffers: Vec<V4l2Buffer>,
    width: u32,
    height: u32,
    stride: u32,
}

pub(crate) struct V4l2Session {
    fd: c_int,
    devnode: String,
    /// Compressed format selected for OUTPUT. Queue objects are rebuilt after
    /// a firmware abort, so this must outlive any one queue incarnation.
    coded_fourcc: u32,
    capture_fourcc: u32,
    out: V4l2Queue,
    cap: V4l2Queue,
    /// CAPTURE pools from before session rebuilds, in pool order.
    legacy: Vec<LegacyPool>,
    fifo: Vec<PendingFrame>,
    ready: Vec<ReadyCapture>,
    no_output_waiting: Vec<u32>,
    eos: bool,
    draining: bool,
    /// Submission order of OUTPUT buffers, used to replay pending chunks in
    /// order when the session is rebuilt.
    out_order: VecDeque<usize>,
    /// Set when the firmware raised EOS with no driver-initiated drain and
    /// work is still pending: the observed small-stream abort signature.
    aborted: bool,
    /// Set while a V4L2 SOURCE_CHANGE flush is in progress: the stateful
    /// decoder returns the last pre-change CAPTURE buffer empty (with
    /// V4L2_BUF_FLAG_LAST) and raises a paired V4L2_EVENT_EOS, neither of which
    /// is the firmware abort.
    source_change_flush: bool,
    /// Empty CAPTURE marker observed for the active source-change boundary.
    source_change_empty_seen: bool,
    /// Paired EOS event observed for the active source-change boundary.
    source_change_eos_seen: bool,
    /// START was already sent for this source-change boundary.
    source_change_start_sent: bool,
    drain_eos_grace: bool,
    /// Set when recovery is impossible or exhausted; sessions then fail like
    /// they did before recovery existed.
    abandoned: bool,
    /// Consecutive failed DECODER_CMD STOP attempts from the sync-path
    /// compatibility drain; crossing the budget abandons the session.
    sync_drain_failures: u32,
    /// Whether a client has requested pre-decode PRIME exports. CPU-copy
    /// clients keep the traditional queue-all CAPTURE behavior; export users
    /// bind one CAPTURE slot to each VA surface before submission.
    stable_capture: bool,
    /// Reentrancy guard: pumps issued from inside `recover` must not trigger
    /// another recovery.
    in_recover: bool,
    recoveries: u32,
    /// Last SPS/PPS bytes seen by the session, prepended to the first replayed
    /// chunk during recovery so the rebuilt decoder sees parameter sets again.
    headers: Vec<u8>,
    replay_history: Vec<ReplayChunk>,
    published_timestamps: VecDeque<u64>,
}

impl V4l2Session {
    pub(crate) fn open_and_setup(
        width: i32,
        height: i32,
        coded_fourcc: u32,
        capture_fourcc: u32,
    ) -> Result<Self, ()> {
        let devnode = std::env::var("V4L2_VA_DEVICE")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/dev/video16".to_string());
        let c_path = CString::new(devnode.as_str()).map_err(|_| ())?;
        let fd = unsafe { open(c_path.as_ptr(), O_RDWR | O_NONBLOCK, 0) };
        if fd < 0 {
            return Err(());
        }

        let mut this = Self {
            fd,
            devnode,
            coded_fourcc,
            capture_fourcc,
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
            abandoned: false,
            sync_drain_failures: 0,
            stable_capture: false,
            in_recover: false,
            recoveries: 0,
            headers: Vec::new(),
            replay_history: Vec::new(),
            published_timestamps: VecDeque::new(),
        };

        if this.query_cap().is_err()
            || this.subscribe_events().is_err()
            || this.setup_output(width, height, coded_fourcc).is_err()
        {
            return Err(());
        }
        Ok(this)
    }

    pub(crate) fn output_sizeimage(&self) -> u32 {
        let pix = unsafe { self.out.fmt.fmt.pix_mp };
        pix.plane_fmt[0].sizeimage
    }

    pub(crate) fn eos(&self) -> bool {
        self.eos
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.fifo.len() + self.ready.len()
    }

    /// Whether this session has latched an unrecoverable device failure.
    ///
    /// Callers use this to turn pending VA surfaces into a prompt decode error
    /// instead of waiting for the generic sync timeout after recovery is
    /// exhausted.
    pub(crate) fn failed(&self) -> bool {
        self.abandoned
    }

    /// Finish queued work before a VA context is detached and return every
    /// completed capture while its mappings are still live. Context teardown
    /// can then publish CPU snapshots before `Drop` releases the V4L2 queues.
    pub(crate) fn drain_for_context_destroy(&mut self) -> Vec<ReadyCapture> {
        self.flush_for_teardown();
        self.pump(0)
    }

    fn stream_on(&mut self, output: bool) -> Result<(), ()> {
        let q = if output { &mut self.out } else { &mut self.cap };
        let mut type_ = q.type_ as c_int;
        let res = xioctl(
            self.fd,
            VIDIOC_STREAMON,
            &mut type_ as *mut _ as *mut c_void,
        );
        if res.is_err() {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: STREAMON {} failed: {}",
                    if output { "OUTPUT" } else { "CAPTURE" },
                    std::io::Error::last_os_error()
                );
            }
            return Err(());
        }
        q.streaming = true;
        Ok(())
    }

    fn reqbufs(&self, type_: u32, count: u32) -> Result<u32, ()> {
        let mut req: v4l2_requestbuffers = zeroed();
        req.type_ = type_;
        req.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        req.count = count;
        if xioctl(self.fd, VIDIOC_REQBUFS, &mut req as *mut _ as *mut c_void).is_err() {
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: REQBUFS type={} count={} failed: {}",
                    type_,
                    count,
                    std::io::Error::last_os_error()
                );
            }
            return Err(());
        }
        Ok(req.count)
    }

    fn mmap_queue(&mut self, output: bool) -> Result<(), ()> {
        let q = if output { &mut self.out } else { &mut self.cap };
        for (i, b) in q.buffers.iter_mut().enumerate() {
            let mut buf: v4l2_buffer = zeroed();
            b.planes = [zeroed(); VIDEO_MAX_PLANES_USIZE];
            buf.type_ = q.type_;
            buf.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
            buf.index = i as u32;
            buf.length = VIDEO_MAX_PLANES;
            buf.m.planes = b.planes.as_mut_ptr();
            xioctl(self.fd, VIDIOC_QUERYBUF, &mut buf as *mut _ as *mut c_void)?;
            b.num_planes = b
                .planes
                .iter()
                .take(buf.length as usize)
                .filter(|p| p.length != 0)
                .count()
                .max(1);
            for p in 0..b.num_planes {
                let length = b.planes[p].length as usize;
                let offset = unsafe { b.planes[p].m.mem_offset } as isize;
                let addr = unsafe {
                    mmap(
                        ptr::null_mut(),
                        length,
                        PROT_READ | PROT_WRITE,
                        MAP_SHARED,
                        self.fd,
                        offset,
                    )
                };
                if addr as isize == -1 {
                    return Err(());
                }
                b.addr[p] = addr;
                b.len[p] = length;
            }
            b.state = BufferState::Free;
        }
        Ok(())
    }
}

fn debug_enabled() -> bool {
    std::env::var_os("V4L2_VA_DEBUG").is_some()
}

/// Read-only coded-format discovery for capability gating.
///
/// Opens the decoder node exactly like `open_and_setup` resolves it and walks
/// VIDIOC_ENUM_FMT on the OUTPUT (coded) queue. Nothing here negotiates a
/// format, allocates buffers, or streams: no decode session is started, so it
/// is safe to run while other clients decode. An empty result means the node
/// could not be opened or exposed nothing; callers keep the historical
/// H.264-only capability table in that case.
fn enumerate_queue_fourccs(queue_type: u32, label: &str) -> Vec<u32> {
    let devnode = std::env::var("V4L2_VA_DEVICE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/dev/video16".to_string());
    let Ok(c_path) = CString::new(devnode.as_str()) else {
        return Vec::new();
    };
    let fd = unsafe { open(c_path.as_ptr(), O_RDWR | O_NONBLOCK, 0) };
    if fd < 0 {
        return Vec::new();
    }
    let mut fourccs = Vec::new();
    let mut desc: v4l2_fmtdesc = zeroed();
    desc.type_ = queue_type;
    // 64 is a safety bound far above any real coded-format count; the loop
    // normally ends on the first ENUM_FMT EINVAL past the last format.
    while fourccs.len() < 64
        && xioctl(fd, VIDIOC_ENUM_FMT, &mut desc as *mut _ as *mut c_void).is_ok()
    {
        fourccs.push(desc.pixelformat);
        desc.index = desc.index.saturating_add(1);
    }
    unsafe { close(fd) };
    if debug_enabled() {
        let list = fourccs
            .iter()
            .map(|fourcc| format!("{fourcc:#010x}"))
            .collect::<Vec<_>>()
            .join(",");
        eprintln!("msm_drv_video_rs: {label} formats on {devnode}: [{list}]");
    }
    fourccs
}

pub(crate) fn enumerate_output_fourccs() -> Vec<u32> {
    enumerate_queue_fourccs(
        v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_OUTPUT_MPLANE as u32,
        "OUTPUT coded",
    )
}

pub(crate) fn enumerate_capture_fourccs() -> Vec<u32> {
    enumerate_queue_fourccs(
        v4l2_buf_type::V4L2_BUF_TYPE_VIDEO_CAPTURE_MPLANE as u32,
        "CAPTURE decoded",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAP_PRIVATE: c_int = 0x02;
    const MAP_ANONYMOUS: c_int = 0x20;
    static SESSION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A real anonymous mapping so `release_mapping` exercises actual munmap.
    fn anon_plane() -> (*mut c_void, usize) {
        const LEN: usize = 4096;
        let addr = unsafe {
            mmap(
                ptr::null_mut(),
                LEN,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert!(addr as isize != -1, "anonymous mmap failed");
        (addr, LEN)
    }

    /// A synthetic session (fd on /dev/null, no ioctls expected) whose OUTPUT,
    /// CAPTURE, and legacy pools each hold one real mapped plane.
    fn session_with_mapped_planes(fd: c_int) -> V4l2Session {
        let mut s = V4l2Session {
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
            abandoned: false,
            sync_drain_failures: 0,
            stable_capture: false,
            in_recover: false,
            recoveries: 0,
            headers: Vec::new(),
            replay_history: Vec::new(),
            published_timestamps: VecDeque::new(),
        };
        for q in [&mut s.out, &mut s.cap] {
            for _ in 0..2 {
                let mut b = V4l2Buffer::new();
                let (addr, len) = anon_plane();
                b.addr[0] = addr;
                b.len[0] = len;
                b.num_planes = 1;
                q.buffers.push(b);
            }
        }
        let mut b = V4l2Buffer::new();
        let (addr, len) = anon_plane();
        b.addr[0] = addr;
        b.len[0] = len;
        b.num_planes = 1;
        s.legacy.push(LegacyPool {
            buffers: vec![b],
            width: 320,
            height: 240,
            stride: 320,
        });
        s
    }

    #[test]
    fn teardown_unmaps_planes_and_closes_fd() {
        let _lock = SESSION_TEST_LOCK.lock().unwrap();
        let before = UNMAPPED_PLANES.load(std::sync::atomic::Ordering::Relaxed);
        let path = CString::new("/dev/null").unwrap();
        let fd = unsafe { open(path.as_ptr(), O_RDWR, 0) };
        assert!(fd >= 0, "could not open /dev/null for the leak check");

        let mut s = session_with_mapped_planes(fd);
        V4l2Session::release_queue_fd(s.fd, &mut s.cap);
        for b in &s.cap.buffers {
            assert!(b.addr[0].is_null(), "CAPTURE plane not reset on release");
            assert_eq!(b.len[0], 0);
            assert_eq!(b.num_planes, 0);
        }
        let after_release = UNMAPPED_PLANES.load(std::sync::atomic::Ordering::Relaxed) - before;
        assert_eq!(
            after_release, 2,
            "CAPTURE release must unmap exactly its planes"
        );

        drop(s);
        let total = UNMAPPED_PLANES.load(std::sync::atomic::Ordering::Relaxed) - before;
        assert_eq!(
            total, 5,
            "OUTPUT + legacy planes must also be released on drop"
        );
        // Do not assert on `fd` after `Drop`: Rust tests run in parallel, and
        // another test can legally open a new file that reuses the same numeric
        // descriptor before `fcntl(fd)` runs. The mmap counters above cover the
        // resource ownership this test is meant to prove.
    }
}
