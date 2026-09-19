//! V4L2 format negotiation and queue bring-up.
//!
//! Session orchestration lives in the parent module. This module keeps the
//! device-specific setup sequence together: capability discovery, format
//! negotiation, buffer allocation, and bounded CAPTURE STREAMON recovery.

use super::{
    BufferState, CAP_EXTRA_BUFFERS, CAP_NUM_BUFFERS_MAX, CAP_NUM_BUFFERS_MIN, OUT_NUM_BUFFERS,
    V4L2_PIX_FMT_H264, V4L2_PIX_FMT_NV12, V4l2Buffer, V4l2Session, VIDIOC_ENUM_FMT, VIDIOC_G_FMT,
    VIDIOC_QBUF, VIDIOC_QUERYCAP, VIDIOC_S_FMT, debug_enabled, xioctl, zeroed,
};
use crate::bindings::*;
use std::ffi::c_void;

impl V4l2Session {
    pub(super) fn query_cap(&mut self) -> Result<(), ()> {
        let mut cap: v4l2_capability = zeroed();
        xioctl(self.fd, VIDIOC_QUERYCAP, &mut cap as *mut _ as *mut c_void)?;
        if (cap.capabilities & V4L2_CAP_STREAMING) == 0
            || (cap.capabilities
                & (V4L2_CAP_VIDEO_M2M_MPLANE
                    | V4L2_CAP_VIDEO_CAPTURE_MPLANE
                    | V4L2_CAP_VIDEO_OUTPUT_MPLANE))
                == 0
        {
            return Err(());
        }
        Ok(())
    }

    pub(super) fn subscribe_events(&mut self) -> Result<(), ()> {
        let mut sub: v4l2_event_subscription = zeroed();
        sub.type_ = V4L2_EVENT_SOURCE_CHANGE;
        let _ = xioctl(
            self.fd,
            super::VIDIOC_SUBSCRIBE_EVENT,
            &mut sub as *mut _ as *mut c_void,
        );
        let mut sub: v4l2_event_subscription = zeroed();
        sub.type_ = V4L2_EVENT_EOS;
        let _ = xioctl(
            self.fd,
            super::VIDIOC_SUBSCRIBE_EVENT,
            &mut sub as *mut _ as *mut c_void,
        );
        Ok(())
    }

    pub(super) fn setup_output(&mut self, width: i32, height: i32) -> Result<(), ()> {
        if !self.enum_formats_contains(self.out.type_, V4L2_PIX_FMT_H264) {
            return Err(());
        }

        let mut out_fmt: v4l2_format = zeroed();
        out_fmt.type_ = self.out.type_;
        xioctl(self.fd, VIDIOC_G_FMT, &mut out_fmt as *mut _ as *mut c_void)?;
        let mut pix = unsafe { out_fmt.fmt.pix_mp };
        pix.pixelformat = V4L2_PIX_FMT_H264;
        pix.width = width.max(0) as u32;
        pix.height = height.max(0) as u32;
        pix.field = v4l2_field::V4L2_FIELD_NONE as u32;
        if width > 0 && height > 0 {
            pix.plane_fmt[0].sizeimage = ((width as u32 * height as u32 * 3 / 2) / 2) + 128;
        }
        pix.num_planes = 1;
        out_fmt.fmt.pix_mp = pix;
        xioctl(self.fd, VIDIOC_S_FMT, &mut out_fmt as *mut _ as *mut c_void)?;
        xioctl(self.fd, VIDIOC_G_FMT, &mut out_fmt as *mut _ as *mut c_void)?;
        self.out.fmt = out_fmt;
        let out_pix = unsafe { self.out.fmt.fmt.pix_mp };
        self.out.fourcc = out_pix.pixelformat;
        self.out.width = out_pix.width;
        self.out.height = out_pix.height;
        if debug_enabled() {
            let (fc, w, h, sz, bpl) = (
                out_pix.pixelformat,
                out_pix.width,
                out_pix.height,
                out_pix.plane_fmt[0].sizeimage,
                out_pix.plane_fmt[0].bytesperline,
            );
            eprintln!(
                "msm_drv_video_rs: OUTPUT fmt fourcc=0x{:08x} {}x{} sizeimage={} bytesperline={}",
                fc, w, h, sz, bpl
            );
        }

        let mut cap_fmt: v4l2_format = zeroed();
        cap_fmt.type_ = self.cap.type_;
        let mut cap_pix: v4l2_pix_format_mplane = zeroed();
        cap_pix.pixelformat = V4L2_PIX_FMT_NV12;
        cap_pix.width = width.max(0) as u32;
        cap_pix.height = height.max(0) as u32;
        cap_pix.field = v4l2_field::V4L2_FIELD_NONE as u32;
        cap_pix.num_planes = 1;
        cap_fmt.fmt.pix_mp = cap_pix;
        xioctl(self.fd, VIDIOC_S_FMT, &mut cap_fmt as *mut _ as *mut c_void)?;
        self.cap.fmt = cap_fmt;
        let cap_pix = unsafe { self.cap.fmt.fmt.pix_mp };
        self.cap.fourcc = cap_pix.pixelformat;
        self.cap.width = cap_pix.width;
        self.cap.height = cap_pix.height;
        if debug_enabled() {
            let (fc, w, h, sz, bpl) = (
                cap_pix.pixelformat,
                cap_pix.width,
                cap_pix.height,
                cap_pix.plane_fmt[0].sizeimage,
                cap_pix.plane_fmt[0].bytesperline,
            );
            eprintln!(
                "msm_drv_video_rs: CAPTURE fmt fourcc=0x{:08x} {}x{} sizeimage={} bytesperline={}",
                fc, w, h, sz, bpl
            );
        }

        let count = self.reqbufs(self.out.type_, OUT_NUM_BUFFERS)?;
        if count == 0 {
            return Err(());
        }
        self.out.buffers = (0..count).map(|_| V4l2Buffer::new()).collect();
        self.mmap_queue(true)?;
        Ok(())
    }

    fn enum_formats_contains(&self, type_: u32, fourcc: u32) -> bool {
        let mut desc: v4l2_fmtdesc = zeroed();
        desc.type_ = type_;
        loop {
            if xioctl(self.fd, VIDIOC_ENUM_FMT, &mut desc as *mut _ as *mut c_void).is_err() {
                return false;
            }
            if desc.pixelformat == fourcc {
                return true;
            }
            desc.index = desc.index.saturating_add(1);
        }
    }

    pub(super) fn try_start(&mut self) -> Result<(), ()> {
        if !self.out.streaming {
            self.stream_on(true)?;
        }
        if self.cap.streaming {
            return Ok(());
        }

        // After rapid session churn the device can briefly report a stale or
        // empty CAPTURE format. Re-apply NV12 and give the hardware a short
        // window to accept it before failing the whole session.
        let mut attempts = 0u32;
        loop {
            let mut cap_fmt: v4l2_format = zeroed();
            cap_fmt.type_ = self.cap.type_;
            xioctl(self.fd, VIDIOC_G_FMT, &mut cap_fmt as *mut _ as *mut c_void)?;
            let pix = unsafe { cap_fmt.fmt.pix_mp };
            if pix.pixelformat == V4L2_PIX_FMT_NV12 {
                self.cap.fmt = cap_fmt;
                self.cap.fourcc = pix.pixelformat;
                self.cap.width = pix.width;
                self.cap.height = pix.height;
                break;
            }
            attempts += 1;
            if attempts > 25 {
                if debug_enabled() {
                    let got = pix.pixelformat;
                    eprintln!(
                        "msm_drv_video_rs: CAPTURE format never became NV12 (fourcc=0x{:08x})",
                        got
                    );
                }
                return Err(());
            }
            let mut set: v4l2_format = zeroed();
            set.type_ = self.cap.type_;
            let mut spix: v4l2_pix_format_mplane = zeroed();
            spix.pixelformat = V4L2_PIX_FMT_NV12;
            spix.width = self.out.width;
            spix.height = self.out.height;
            spix.field = v4l2_field::V4L2_FIELD_NONE as u32;
            spix.num_planes = 1;
            set.fmt.pix_mp = spix;
            let _ = xioctl(self.fd, VIDIOC_S_FMT, &mut set as *mut _ as *mut c_void);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        if self.cap.buffers.is_empty()
            && let Err(e) = self.capture_pool_setup()
        {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: CAPTURE pool setup failed");
            }
            return Err(e);
        }
        if !self.stable_capture && self.queue_all_capture().is_err() {
            if debug_enabled() {
                eprintln!("msm_drv_video_rs: CAPTURE QBUF failed before STREAMON");
            }
            return Err(());
        }
        // A failed STREAMON can leave the queue in an error state; retrying
        // STREAMON on the same buffers never clears it. Reinitialize the whole
        // CAPTURE queue (STREAMOFF, REQBUFS(0), realloc, requeue) between
        // attempts, which is the V4L2-sanctioned recovery sequence.
        let mut attempts = 0u32;
        loop {
            match self.stream_on(false) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    attempts += 1;
                    if attempts >= 5 {
                        if debug_enabled() {
                            eprintln!("msm_drv_video_rs: CAPTURE STREAMON failed");
                        }
                        return Err(e);
                    }
                    // A CAPTURE slot is now a stable VA-surface backing
                    // allocation. Rebuilding the queue here would invalidate
                    // a pre-decode PRIME fd and lose the surface-to-buffer
                    // binding, so fail this session rather than handing the
                    // client a live descriptor for a freed buffer.
                    if self.stable_capture {
                        if debug_enabled() {
                            eprintln!(
                                "msm_drv_video_rs: CAPTURE STREAMON failed with bound buffers"
                            );
                        }
                        return Err(e);
                    }
                    if debug_enabled() {
                        eprintln!("msm_drv_video_rs: CAPTURE STREAMON retry {}", attempts);
                    }
                    Self::release_queue_fd(self.fd, &mut self.cap);
                    if let Err(e2) = self.capture_pool_setup() {
                        if debug_enabled() {
                            eprintln!("msm_drv_video_rs: CAPTURE pool setup failed");
                        }
                        return Err(e2);
                    }
                    if self.queue_all_capture().is_err() {
                        if debug_enabled() {
                            eprintln!("msm_drv_video_rs: CAPTURE QBUF retry failed");
                        }
                        return Err(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        }
    }

    pub(super) fn capture_pool_setup(&mut self) -> Result<(), ()> {
        let mut want = 4 + CAP_EXTRA_BUFFERS;
        want = want.clamp(CAP_NUM_BUFFERS_MIN, CAP_NUM_BUFFERS_MAX);
        let mut count = self.reqbufs(self.cap.type_, want).unwrap_or(0);
        if count == 0 && want != OUT_NUM_BUFFERS {
            count = self.reqbufs(self.cap.type_, OUT_NUM_BUFFERS).unwrap_or(0);
        }
        if count == 0 {
            count = self.reqbufs(self.cap.type_, 4).unwrap_or(0);
        }
        if count == 0 {
            return Err(());
        }
        if debug_enabled() {
            eprintln!("msm_drv_video_rs: CAPTURE REQBUFS count={}", count);
        }
        self.cap.buffers = (0..count).map(|_| V4l2Buffer::new()).collect();
        self.mmap_queue(false)?;
        Ok(())
    }

    pub(super) fn qbuf_capture(&mut self, idx: usize) -> Result<(), ()> {
        let b = self.cap.buffers.get_mut(idx).ok_or(())?;
        if b.state == BufferState::Queued || b.num_planes == 0 || b.len[0] == 0 {
            return Err(());
        }
        for p in 0..b.num_planes {
            b.planes[p].bytesused = 0;
        }
        let mut buf: v4l2_buffer = zeroed();
        buf.type_ = self.cap.type_;
        buf.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        buf.index = idx as u32;
        buf.length = b.num_planes as u32;
        buf.m.planes = b.planes.as_mut_ptr();
        let fd = self.fd;
        let res = xioctl(fd, VIDIOC_QBUF, &mut buf as *mut _ as *mut c_void);
        if res.is_ok()
            && let Some(b) = self.cap.buffers.get_mut(idx)
        {
            b.state = BufferState::Queued;
        }
        res
    }

    /// Bytes still queued on OUTPUT, in submission order. The caller must
    /// treat these as the only survivors of an aborted session.
    pub(super) fn snapshot_pending_output(&self) -> Vec<Vec<u8>> {
        let mut chunks = Vec::new();
        for &idx in &self.out_order {
            if let Some(b) = self.out.buffers.get(idx) {
                if b.state != BufferState::Queued || b.addr[0].is_null() {
                    continue;
                }
                let n = (b.planes[0].bytesused as usize).min(b.len[0]);
                if n == 0 {
                    continue;
                }
                chunks.push(
                    unsafe { std::slice::from_raw_parts(b.addr[0] as *const u8, n) }.to_vec(),
                );
            }
        }
        chunks
    }
}
