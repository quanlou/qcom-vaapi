//! V4L2 format negotiation and queue bring-up.
//!
//! Session orchestration lives in the parent module. This module keeps the
//! device-specific setup sequence together: capability discovery, format
//! negotiation, buffer allocation, and bounded CAPTURE STREAMON recovery.

use super::{
    BufferState, CAP_NUM_BUFFERS_MAX, OUT_NUM_BUFFERS, V4l2Buffer, V4l2Session, VIDIOC_CREATE_BUFS,
    VIDIOC_ENUM_FMT, VIDIOC_G_CTRL, VIDIOC_G_FMT, VIDIOC_QBUF, VIDIOC_QUERYCAP, VIDIOC_QUERYCTRL,
    VIDIOC_S_CTRL, VIDIOC_S_FMT, WORKING_QUEUE_MAX, debug_enabled, xioctl, zeroed,
};
use crate::bindings::*;
use std::ffi::c_void;

impl V4l2Session {
    pub(super) fn query_cap(&mut self) -> Result<(), ()> {
        let mut cap: v4l2_capability = zeroed();
        xioctl(self.fd, VIDIOC_QUERYCAP, &mut cap as *mut _ as *mut c_void)?;
        if !supports_decoder_queues(&cap) {
            return Err(());
        }
        Ok(())
    }

    pub(super) fn subscribe_events(&mut self) -> Result<(), ()> {
        let mut sub: v4l2_event_subscription = zeroed();
        sub.type_ = V4L2_EVENT_SOURCE_CHANGE;
        xioctl(
            self.fd,
            super::VIDIOC_SUBSCRIBE_EVENT,
            &mut sub as *mut _ as *mut c_void,
        )?;
        let mut sub: v4l2_event_subscription = zeroed();
        sub.type_ = V4L2_EVENT_EOS;
        xioctl(
            self.fd,
            super::VIDIOC_SUBSCRIBE_EVENT,
            &mut sub as *mut _ as *mut c_void,
        )?;
        Ok(())
    }

    pub(super) fn setup_output(
        &mut self,
        width: i32,
        height: i32,
        coded_fourcc: u32,
    ) -> Result<(), ()> {
        self.capture_metadata_ready = false;
        if !self.enum_formats_contains(self.out.type_, coded_fourcc) {
            return Err(());
        }

        let mut out_fmt: v4l2_format = zeroed();
        out_fmt.type_ = self.out.type_;
        xioctl(self.fd, VIDIOC_G_FMT, &mut out_fmt as *mut _ as *mut c_void)?;
        let mut pix = unsafe { out_fmt.fmt.pix_mp };
        pix.pixelformat = coded_fourcc;
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
        if unsafe { out_fmt.fmt.pix_mp }.pixelformat != coded_fourcc
            || unsafe { out_fmt.fmt.pix_mp }.num_planes != 1
        {
            return Err(());
        }
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
        cap_pix.pixelformat = self.capture_fourcc;
        cap_pix.width = width.max(0) as u32;
        cap_pix.height = height.max(0) as u32;
        cap_pix.field = v4l2_field::V4L2_FIELD_NONE as u32;
        cap_pix.num_planes = 1;
        cap_fmt.fmt.pix_mp = cap_pix;
        xioctl(self.fd, VIDIOC_S_FMT, &mut cap_fmt as *mut _ as *mut c_void)?;
        let negotiated = unsafe { cap_fmt.fmt.pix_mp };
        // Iris reports provisional NV12 before parsing Main10 headers. The
        // final P010 selection is validated by try_start before STREAMON.
        let provisional_main10 = self.capture_fourcc == super::V4L2_PIX_FMT_P010
            && negotiated.pixelformat == super::V4L2_PIX_FMT_NV12;
        if (negotiated.pixelformat != self.capture_fourcc && !provisional_main10)
            || negotiated.num_planes != 1
        {
            return Err(());
        }
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

        self.configure_decode_order()?;
        let count = self.reqbufs(self.out.type_, OUT_NUM_BUFFERS)?;
        if count == 0 {
            return Err(());
        }
        self.out.buffers = (0..count).map(|_| V4l2Buffer::new()).collect();
        self.mmap_queue(true)?;
        Ok(())
    }

    fn configure_decode_order(&self) -> Result<(), ()> {
        // VA synchronizes in decode order. Stateful display-order output can
        // withhold a reference picture until the client submits its B frames,
        // while that same client waits for the picture before submitting.
        // Ask only drivers that expose both standard delay controls; older
        // Iris modules keep the bounded compatibility drain path.
        let controls = [
            (V4L2_CID_MPEG_VIDEO_DEC_DISPLAY_DELAY, 0),
            (V4L2_CID_MPEG_VIDEO_DEC_DISPLAY_DELAY_ENABLE, 1),
        ];
        for (id, _) in controls {
            let mut query: v4l2_queryctrl = zeroed();
            query.id = id;
            if xioctl(
                self.fd,
                VIDIOC_QUERYCTRL,
                &mut query as *mut _ as *mut c_void,
            )
            .is_err()
            {
                let kind = std::io::Error::last_os_error().raw_os_error();
                if matches!(kind, Some(22 | 25)) {
                    if debug_enabled() {
                        eprintln!(
                            "msm_drv_video_rs: decode-order controls unavailable; compatibility drain retained"
                        );
                    }
                    return Ok(());
                }
                return Err(());
            }
            if query.flags & V4L2_CTRL_FLAG_DISABLED != 0 || query.minimum > 0 || query.maximum < 1
            {
                return Err(());
            }
        }
        for (id, value) in controls {
            let mut control: v4l2_control = zeroed();
            control.id = id;
            control.value = value;
            xioctl(
                self.fd,
                VIDIOC_S_CTRL,
                &mut control as *mut _ as *mut c_void,
            )?;
        }
        if debug_enabled() {
            eprintln!("msm_drv_video_rs: decode-order output requested via display-delay controls");
        }
        Ok(())
    }

    fn enum_formats_contains(&self, type_: u32, fourcc: u32) -> bool {
        let mut desc: v4l2_fmtdesc = zeroed();
        desc.type_ = type_;
        for index in 0..64 {
            desc.index = index;
            if xioctl(self.fd, VIDIOC_ENUM_FMT, &mut desc as *mut _ as *mut c_void).is_err() {
                return false;
            }
            if desc.pixelformat == fourcc {
                return true;
            }
        }
        false
    }

    pub(super) fn try_start(&mut self) -> Result<(), ()> {
        if !self.out.streaming {
            self.stream_on(true)?;
        }
        if self.cap.streaming {
            return Ok(());
        }

        // Provisional S_FMT dimensions do not prove that the firmware has
        // parsed its DPB requirement. A CPU pool may shrink only after the
        // initial SOURCE_CHANGE event; pre-decode export/recovery pools keep32.
        if self.cap.buffers.is_empty() && !self.capture_metadata_ready {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while !self.capture_metadata_ready {
                self.dequeue_events();
                if self.aborted || self.abandoned {
                    return Err(());
                }
                if !self.capture_metadata_ready {
                    if std::time::Instant::now() >= deadline {
                        return Err(());
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
        }

        // After rapid session churn the device can briefly report a stale or
        // empty CAPTURE format. Re-apply the negotiated format and give the hardware a short
        // window to accept it before failing the whole session.
        let mut attempts = 0u32;
        loop {
            let mut cap_fmt: v4l2_format = zeroed();
            cap_fmt.type_ = self.cap.type_;
            xioctl(self.fd, VIDIOC_G_FMT, &mut cap_fmt as *mut _ as *mut c_void)?;
            let pix = unsafe { cap_fmt.fmt.pix_mp };
            if pix.pixelformat == self.capture_fourcc {
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
                        "msm_drv_video_rs: CAPTURE format never became 0x{:08x} (fourcc=0x{:08x})",
                        self.capture_fourcc, got
                    );
                }
                return Err(());
            }
            let mut set: v4l2_format = zeroed();
            set.type_ = self.cap.type_;
            let mut spix: v4l2_pix_format_mplane = zeroed();
            spix.pixelformat = self.capture_fourcc;
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

    fn capture_pool_target(&self) -> Result<u32, ()> {
        if !self.capture_metadata_ready {
            // Chromium may export before submitting any stream header.
            return Ok(32);
        }
        let mut control: v4l2_control = zeroed();
        control.id = V4L2_CID_MIN_BUFFERS_FOR_CAPTURE;
        xioctl(
            self.fd,
            VIDIOC_G_CTRL,
            (&mut control as *mut v4l2_control).cast(),
        )?;
        capture_target(control.value)
    }

    pub(super) fn capture_pool_setup(&mut self) -> Result<(), ()> {
        let want = self.capture_pool_target()?;
        let count = self.reqbufs(self.cap.type_, want)?;
        // Never retry with an arbitrarily smaller allocation after a failure.
        if count < want || count > CAP_NUM_BUFFERS_MAX {
            return Err(());
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: CAPTURE REQBUFS requested={} count={}",
                want, count
            );
        }
        self.cap.buffers = (0..count).map(|_| V4l2Buffer::new()).collect();
        self.mmap_queue(false)
    }

    pub(super) fn grow_capture_pool(&mut self) -> Result<(), ()> {
        let start = self.cap.buffers.len();
        let count = 4.min(CAP_NUM_BUFFERS_MAX.saturating_sub(start as u32));
        if count == 0 {
            return Err(());
        }
        let mut create: v4l2_create_buffers = zeroed();
        create.count = count;
        create.memory = v4l2_memory::V4L2_MEMORY_MMAP as u32;
        create.format = self.cap.fmt;
        xioctl(
            self.fd,
            VIDIOC_CREATE_BUFS,
            (&mut create as *mut v4l2_create_buffers).cast(),
        )?;
        if create.index as usize != start || create.count == 0 || create.count > count {
            self.abandoned = true;
            return Err(());
        }
        self.cap
            .buffers
            .extend((0..create.count).map(|_| V4l2Buffer::new()));
        if self.mmap_queue(false).is_err() {
            self.abandoned = true;
            return Err(());
        }
        if debug_enabled() {
            eprintln!(
                "msm_drv_video_rs: CAPTURE CREATE_BUFS start={} count={} total={}",
                start,
                create.count,
                self.cap.buffers.len()
            );
        }
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

// capabilities is the union across all nodes of a physical device. Prefer
// device_caps when present and require both multi-planar queue directions.
fn supports_decoder_queues(cap: &v4l2_capability) -> bool {
    let caps = if cap.capabilities & V4L2_CAP_DEVICE_CAPS != 0 {
        cap.device_caps
    } else {
        cap.capabilities
    };
    let both = V4L2_CAP_VIDEO_CAPTURE_MPLANE | V4L2_CAP_VIDEO_OUTPUT_MPLANE;
    caps & V4L2_CAP_STREAMING != 0 && (caps & V4L2_CAP_VIDEO_M2M_MPLANE != 0 || caps & both == both)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_decode_order_control_ioctl_numbers_match_linux_abi() {
        assert_eq!(VIDIOC_S_CTRL, 0xc008_561c);
        assert_eq!(VIDIOC_QUERYCTRL, 0xc044_5624);
    }

    #[test]
    fn decoder_capabilities_require_streaming_and_both_directions() {
        let mut cap: v4l2_capability = zeroed();
        cap.capabilities = V4L2_CAP_STREAMING | V4L2_CAP_VIDEO_CAPTURE_MPLANE;
        assert!(!supports_decoder_queues(&cap));
        cap.capabilities |= V4L2_CAP_VIDEO_OUTPUT_MPLANE;
        assert!(supports_decoder_queues(&cap));
        cap.capabilities = V4L2_CAP_VIDEO_M2M_MPLANE;
        assert!(!supports_decoder_queues(&cap));
        cap.capabilities |= V4L2_CAP_STREAMING;
        assert!(supports_decoder_queues(&cap));
    }

    #[test]
    fn decoder_capabilities_use_the_opened_node_not_the_device_union() {
        let mut cap: v4l2_capability = zeroed();
        cap.capabilities = V4L2_CAP_DEVICE_CAPS | V4L2_CAP_STREAMING | V4L2_CAP_VIDEO_M2M_MPLANE;
        cap.device_caps = V4L2_CAP_STREAMING | V4L2_CAP_VIDEO_CAPTURE_MPLANE;
        assert!(!supports_decoder_queues(&cap));
        cap.device_caps |= V4L2_CAP_VIDEO_OUTPUT_MPLANE;
        assert!(supports_decoder_queues(&cap));
    }
}

// Firmware requirement plus all six working slots. Surface-owned PRIME
// backing and CPU snapshots do not need a spare pool of twenty mapped frames;
// legacy CAPTURE reservations grow separately through CREATE_BUFS.
// Invalid controls fail setup; they never authorize shrinking the queue.
fn capture_target(firmware_minimum: i32) -> Result<u32, ()> {
    if !(1..=32).contains(&firmware_minimum) {
        return Err(());
    }
    let target = firmware_minimum as u32 + WORKING_QUEUE_MAX as u32;
    if target > CAP_NUM_BUFFERS_MAX {
        return Err(());
    }
    Ok(target)
}

#[cfg(test)]
mod allocation_tests {
    use super::*;
    #[test]
    fn firmware_minimum_and_working_slack_bound_the_cpu_pool() {
        assert_eq!(capture_target(1), Ok(7));
        assert_eq!(capture_target(4), Ok(10));
        assert_eq!(capture_target(14), Ok(20));
        assert_eq!(capture_target(18), Ok(24));
        assert_eq!(capture_target(32), Ok(38));
        for minimum in [i32::MIN, -1, 0, 33, i32::MAX] {
            assert!(capture_target(minimum).is_err());
        }
    }
}
