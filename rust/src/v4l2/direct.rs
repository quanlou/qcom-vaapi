//! Select a surface's DMA-BUF by making it the only available CAPTURE target.
//! Each completion binds the next chosen target in decode order. The decoder's
//! internal DPB remains separate from these linear display allocations.

use super::{BufferState, V4l2Queue, V4l2Session, debug_enabled};
use crate::surface_backing::DecodeTarget;
use std::os::fd::AsRawFd;

impl V4l2Session {
    pub(crate) fn direct_capture_mode(&self) -> bool {
        self.direct_target.is_some()
    }

    pub(crate) fn bind_decode_target(
        &mut self,
        surface: u32,
        target: DecodeTarget,
    ) -> Result<bool, ()> {
        // Old kernels without decode-order output need the compatibility
        // publication path. AV1 hidden-reference handling is not qualified
        // for a single selected target yet.
        if !self.decode_order || self.coded_fourcc == super::V4L2_PIX_FMT_AV1 {
            return Ok(false);
        }
        if self.direct_target.is_none() && !self.cap.buffers.is_empty() {
            // An imported target may have started a compatibility pool.
            // Keep that mode when a later picture uses driver-owned storage.
            return Ok(false);
        }
        if !self.cap.buffers.is_empty() {
            Self::validate_direct_layout(&self.cap, &target)?;
        }
        if !self.fifo.is_empty()
            || self
                .cap
                .buffers
                .first()
                .is_some_and(|b| b.state == BufferState::Queued)
            || !self.direct_targets.is_empty()
        {
            // Never expose two target allocations to the firmware together.
            // Keep the next owner waiting until the preceding completion.
            // VP9 submit_frame waits for this selection. Reordered codecs
            // still need their following input before a completion can arrive.
            if self.direct_targets.len() >= crate::state::DRV_MAX_SURFACES
                || self
                    .direct_targets
                    .iter()
                    .any(|(owner, _)| *owner == surface)
                || self
                    .direct_target
                    .as_ref()
                    .is_some_and(|(owner, _)| *owner == surface)
            {
                return Err(());
            }
            self.direct_targets.push_back((surface, target));
            return Ok(true);
        }
        if !self.cap.buffers.is_empty() {
            Self::initialize_direct_buffer(&mut self.cap, surface, &target)?;
        }
        self.direct_target = Some((surface, target));
        Ok(true)
    }

    pub(super) fn wait_for_direct_target(
        &mut self,
        surface: u32,
        deadline: std::time::Instant,
    ) -> Result<(), ()> {
        while self
            .direct_target
            .as_ref()
            .is_some_and(|(owner, _)| *owner != surface)
        {
            if self.aborted || self.abandoned || std::time::Instant::now() >= deadline {
                if debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: direct target wait failed surface={surface} active={:?} {}",
                        self.direct_target.as_ref().map(|(owner, _)| *owner),
                        self.debug_snapshot()
                    );
                }
                // bind_decode_target already retained the waiting owner.
                // After its EndPicture fails, advancing to that dead owner
                // on a later submission would misassign the next picture.
                self.abandoned = true;
                return Err(());
            }
            // A completion advances the chosen allocation. Preserve completed
            // owners for VA publication after submission releases the lock;
            // OUTPUT writability alone must not trigger a STOP or busy loop.
            let ready = self.pump(2);
            self.ready.extend(ready);
            if self.aborted || self.abandoned {
                self.abandoned = true;
                return Err(());
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            std::thread::sleep(remaining.min(std::time::Duration::from_millis(1)));
        }
        if self.aborted || self.abandoned {
            self.abandoned = true;
            return Err(());
        }
        Ok(())
    }

    pub(super) fn initialize_direct_capture(&mut self) -> Result<(), ()> {
        let (surface, target) = self.direct_target.as_ref().ok_or(())?;
        Self::initialize_direct_buffer(&mut self.cap, *surface, target)
    }

    fn initialize_direct_buffer(
        cap: &mut V4l2Queue,
        surface: u32,
        target: &DecodeTarget,
    ) -> Result<(), ()> {
        Self::validate_direct_layout(cap, target)?;
        let pix = unsafe { cap.fmt.fmt.pix_mp };
        let layout = target.layout;
        let stride = pix.plane_fmt[0].bytesperline;
        let fd = target.fd.try_clone().map_err(|_| ())?;
        let buffer = cap.buffers.first_mut().ok_or(())?;
        // Direct targets are never CPU mapped through the working queue.
        // CPU images read the stable allocation owned by the VA surface.
        if !buffer.addr[0].is_null()
            || matches!(buffer.state, BufferState::Queued | BufferState::Publishing)
        {
            return Err(());
        }
        buffer.num_planes = 1;
        buffer.len[0] = layout.size as usize;
        buffer.planes[0].length = layout.size;
        buffer.planes[0].m.fd = fd.as_raw_fd();
        buffer.import_fd = Some(fd);
        buffer.state = BufferState::Free;
        if debug_enabled() {
            let storage_height = pix.height;
            eprintln!(
                "msm_drv_video_rs: direct CAPTURE bound surface={} fd={} stride={} storage_height={} size={}",
                surface,
                target.fd.as_raw_fd(),
                stride,
                storage_height,
                layout.size
            );
        }
        Ok(())
    }

    fn validate_direct_layout(cap: &V4l2Queue, target: &DecodeTarget) -> Result<(), ()> {
        let pix = unsafe { cap.fmt.fmt.pix_mp };
        let layout = target.layout;
        let stride = pix.plane_fmt[0].bytesperline;
        if pix.num_planes != 1
            || layout.format.v4l2_fourcc() != pix.pixelformat
            || layout.stride != stride
            || layout.y_offset != 0
            || Some(layout.uv_offset) != stride.checked_mul(pix.height)
            || layout.size < pix.plane_fmt[0].sizeimage
            || layout.width > pix.width
            || layout.height > pix.height
        {
            return Err(());
        }
        Ok(())
    }

    pub(super) fn advance_direct_target(&mut self) -> Result<(), ()> {
        if let Some((surface, target)) = self.direct_targets.pop_front() {
            Self::initialize_direct_buffer(&mut self.cap, surface, &target)?;
            self.direct_target = Some((surface, target));
            self.queue_working_capture()?;
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn capture_is_direct(&self, idx: usize, surface: u32) -> bool {
        idx == self.legacy_len()
            && self
                .direct_target
                .as_ref()
                .is_some_and(|(owner, _)| *owner == surface)
            && self
                .cap
                .buffers
                .first()
                .is_some_and(|b| b.state == BufferState::DirectComplete)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_format::DecodedFormat;
    use crate::surface_backing::SurfaceBacking;
    use std::os::fd::{FromRawFd, IntoRawFd};
    use std::os::unix::fs::{FileExt, MetadataExt};

    fn session() -> V4l2Session {
        let fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap()
            .into_raw_fd();
        let mut session = V4l2Session::pending_sync_test_session(fd, false);
        session.fifo.clear();
        session.decode_order = true;
        session
    }

    fn setup_capture(session: &mut V4l2Session, target: &DecodeTarget) {
        let mut pix: crate::bindings::v4l2_pix_format_mplane = super::super::zeroed();
        pix.width = target.layout.stride;
        pix.height = target.layout.uv_offset / target.layout.stride;
        pix.pixelformat = target.layout.format.v4l2_fourcc();
        pix.num_planes = 1;
        pix.plane_fmt[0].bytesperline = target.layout.stride;
        pix.plane_fmt[0].sizeimage = target.layout.size;
        session.cap.fmt.fmt.pix_mp = pix;
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        session.initialize_direct_capture().unwrap();
    }

    #[test]
    fn existing_compatibility_pool_retains_its_storage_and_mode() {
        let backing = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let mut session = session();
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        session.cap.buffers[0].state = BufferState::Queued;
        assert_eq!(
            session.bind_decode_target(7, backing.decode_target().unwrap().unwrap()),
            Ok(false)
        );
        assert!(!session.direct_capture_mode());
        assert!(session.cap.buffers[0].state == BufferState::Queued);
        assert!(session.cap.buffers[0].import_fd.is_none());
    }

    #[test]
    fn waiting_target_cannot_submit_or_replace_the_active_owner_on_timeout() {
        let first = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let next = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let mut session = session();
        session
            .bind_decode_target(7, first.decode_target().unwrap().unwrap())
            .unwrap();
        setup_capture(&mut session, &first.decode_target().unwrap().unwrap());
        session.cap.buffers[0].state = BufferState::Queued;
        let fd = session.cap.buffers[0]
            .import_fd
            .as_ref()
            .unwrap()
            .as_raw_fd();
        session
            .bind_decode_target(8, next.decode_target().unwrap().unwrap())
            .unwrap();
        // The same owner's hidden input and show_existing export remain a
        // pair; they must not wait for their own not-yet-submitted completion.
        session
            .wait_for_direct_target(7, std::time::Instant::now())
            .unwrap();
        assert!(
            session
                .wait_for_direct_target(8, std::time::Instant::now())
                .is_err()
        );
        assert_eq!(session.direct_target.as_ref().unwrap().0, 7);
        assert_eq!(session.direct_targets.front().unwrap().0, 8);
        assert_eq!(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            fd
        );
        assert!(session.cap.buffers[0].state == BufferState::Queued);
        assert_eq!(session.out_queued(), 0);
        assert!(!session.draining);
        assert!(session.abandoned);
        assert!(
            session
                .wait_for_direct_target(7, std::time::Instant::now())
                .is_err()
        );
    }

    #[test]
    fn selected_slot_changes_fd_without_overwriting_previous_surface() {
        let first = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let second = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let mut session = session();
        assert_eq!(
            session.bind_decode_target(7, first.decode_target().unwrap().unwrap()),
            Ok(true)
        );
        setup_capture(&mut session, &first.decode_target().unwrap().unwrap());
        let writer = std::fs::File::from(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .try_clone()
                .unwrap(),
        );
        writer.write_all_at(&[29], 0).unwrap();
        session.cap.buffers[0].state = BufferState::DirectComplete;
        assert!(session.capture_is_direct(0, 7));
        assert!(!session.capture_is_direct(0, 8));
        assert_eq!(
            session.bind_decode_target(8, second.decode_target().unwrap().unwrap()),
            Ok(true)
        );
        let next_writer = std::fs::File::from(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .try_clone()
                .unwrap(),
        );
        assert_ne!(
            writer.metadata().unwrap().ino(),
            next_writer.metadata().unwrap().ino()
        );
        next_writer.write_all_at(&[41], 0).unwrap();
        // Releasing the older surface must never queue the next one's fd.
        session.requeue_capture(0);
        assert!(session.cap.buffers[0].state == BufferState::Free);
        session.cap.buffers[0].state = BufferState::DirectComplete;
        session.queue_working_capture().unwrap();
        assert!(session.cap.buffers[0].state == BufferState::DirectComplete);
        assert!(session.cap.buffers[1].import_fd.is_none());
        assert_eq!(first.download().unwrap().data[0], 29);
        assert_eq!(second.download().unwrap().data[0], 41);
    }

    #[test]
    fn incompatible_target_and_in_flight_rebind_leave_previous_owner_intact() {
        let backing = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let mut session = session();
        assert_eq!(
            session.bind_decode_target(7, backing.decode_target().unwrap().unwrap()),
            Ok(true)
        );
        setup_capture(&mut session, &backing.decode_target().unwrap().unwrap());
        let fd = session.cap.buffers[0]
            .import_fd
            .as_ref()
            .unwrap()
            .as_raw_fd();
        let mut incompatible = backing.decode_target().unwrap().unwrap();
        incompatible.layout.uv_offset -= 1;
        assert_eq!(session.bind_decode_target(8, incompatible), Err(()));
        assert_eq!(session.direct_target.as_ref().unwrap().0, 7);
        assert_eq!(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            fd
        );
        session.cap.buffers[0].state = BufferState::Queued;
        assert_eq!(
            session.bind_decode_target(8, backing.decode_target().unwrap().unwrap()),
            Ok(true)
        );
        assert_eq!(session.direct_target.as_ref().unwrap().0, 7);
        assert!(session.cap.buffers[0].state == BufferState::Queued);
        assert_eq!(session.direct_targets.len(), 1);
        assert_eq!(
            session.bind_decode_target(8, backing.decode_target().unwrap().unwrap()),
            Err(())
        );
        session.cap.buffers[0].state = BufferState::DirectComplete;
        // Binding is deferred until completion. The host fd rejects the
        // final QBUF, after the previous owner has safely left the kernel.
        assert!(session.advance_direct_target().is_err());
        assert_eq!(session.direct_target.as_ref().unwrap().0, 8);
        assert!(session.direct_targets.is_empty());
    }

    #[test]
    fn completed_input_does_not_wake_direct_wait_on_writability() {
        unsafe extern "C" {
            fn pipe(fds: *mut i32) -> i32;
        }
        let mut fds = [-1; 2];
        assert_eq!(unsafe { pipe(fds.as_mut_ptr()) }, 0);
        let reader = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[0]) };
        let mut session = V4l2Session::pending_sync_test_session(fds[1], false);
        let backing = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        session.direct_target = Some((7, backing.decode_target().unwrap().unwrap()));
        let mut pfd = super::super::PollFd {
            fd: session.fd,
            events: session.poll_events(),
            revents: 0,
        };
        assert_eq!(unsafe { super::super::poll(&mut pfd, 1, 10) }, 0);
        let mut input = super::super::V4l2Buffer::new();
        input.state = BufferState::Queued;
        session.out.buffers.push(input);
        pfd.events = session.poll_events();
        assert_eq!(unsafe { super::super::poll(&mut pfd, 1, 0) }, 1);
        assert_ne!(pfd.revents & super::super::POLLOUT, 0);
        session.out.buffers[0].state = BufferState::Free;
        drop(reader);
    }
}
