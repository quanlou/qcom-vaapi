//! External CAPTURE storage. Direct mode imports the one chosen surface;
//! compatibility mode allocates a working pool for detached publication.

use super::{V4l2Session, debug_enabled};
use crate::bindings::*;
use std::os::fd::{AsRawFd, OwnedFd};

// CAPTURE storage and standalone exported surfaces have separate owners.
// Bound the working pool before allocating any of its DMA-BUFs, especially
// when experimental 8K frames require tens of megabytes per slot.
pub(super) const MAX_CAPTURE_POOL_BYTES: usize = 1024 * 1024 * 1024;

/// Validate the negotiated storage geometry, not the visible VA dimensions.
/// In particular, Iris puts chroma after its padded luma storage height.
pub(super) fn capture_size(pix: &v4l2_pix_format_mplane) -> Result<usize, ()> {
    let format = crate::pixel_format::DecodedFormat::from_v4l2_fourcc(pix.pixelformat).ok_or(())?;
    let stride = pix.plane_fmt[0].bytesperline;
    let size = pix.plane_fmt[0].sizeimage;
    let min_stride = pix.width.checked_mul(format.bytes_per_sample()).ok_or(())?;
    let rows = pix.height.checked_add(pix.height.div_ceil(2)).ok_or(())?;
    let required = stride.checked_mul(rows).ok_or(())?;
    if pix.num_planes != 1
        || pix.width == 0
        || pix.height == 0
        || !pix.width.is_multiple_of(2)
        || !pix.height.is_multiple_of(2)
        || stride < min_stride
        || required > size
        || size == 0
        || size > 128 * 1024 * 1024
    {
        return Err(());
    }
    Ok(size as usize)
}

impl V4l2Session {
    pub(super) fn initialize_imported_capture(&mut self) -> Result<(), ()> {
        if self.direct_target.is_some() {
            return self.initialize_direct_capture();
        }
        let fd = self.capture_drm_fd.as_ref().ok_or(())?.as_raw_fd();
        self.initialize_imported_capture_with(|size| {
            crate::surface_backing::allocate_capture_drm(size, fd).map_err(|_| ())
        })
    }

    pub(super) fn prepare_drain_capture(&mut self) -> Result<usize, ()> {
        let fd = self.capture_drm_fd.as_ref().ok_or(())?.as_raw_fd();
        self.prepare_drain_capture_with(|size| {
            crate::surface_backing::allocate_capture_drm(size, fd).map_err(|_| ())
        })
    }

    fn prepare_drain_capture_with(
        &mut self,
        allocate: impl FnOnce(usize) -> Result<OwnedFd, ()>,
    ) -> Result<usize, ()> {
        // Slot zero belongs to the published direct surface. A separate,
        // unowned slot receives LAST without risking those retained pixels.
        let size = capture_size(&unsafe { self.cap.fmt.fmt.pix_mp })?;
        let idx = self
            .cap
            .buffers
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(idx, b)| {
                (b.state == super::BufferState::Free
                    && b.reserved_for.is_none()
                    && b.export_refs == 0
                    && b.num_planes == 0)
                    .then_some(idx)
            })
            .ok_or(())?;
        let fd = allocate(size)?;
        let b = &mut self.cap.buffers[idx];
        b.num_planes = 1;
        b.len[0] = size;
        b.planes[0].length = size as u32;
        b.planes[0].m.fd = fd.as_raw_fd();
        b.import_fd = Some(fd);
        Ok(idx)
    }

    fn initialize_imported_capture_with(
        &mut self,
        mut allocate: impl FnMut(usize) -> Result<OwnedFd, ()>,
    ) -> Result<(), ()> {
        let pix = unsafe { self.cap.fmt.fmt.pix_mp };
        let size = capture_size(&pix)?;
        if size
            .checked_mul(self.cap.buffers.len())
            .is_none_or(|bytes| bytes > MAX_CAPTURE_POOL_BYTES)
        {
            return Err(());
        }
        let stride = pix.plane_fmt[0].bytesperline;
        let storage_height = pix.height;
        // Allocate the entire new tail before changing slot metadata. An
        // allocation failure drops temporary owners and leaves live slots alone.
        let tail: Vec<usize> = self
            .cap
            .buffers
            .iter()
            .enumerate()
            .filter_map(|(i, b)| (b.num_planes == 0).then_some(i))
            .collect();
        let allocations: Vec<OwnedFd> = tail
            .iter()
            .map(|_| allocate(size))
            .collect::<Result<_, _>>()?;
        for (idx, fd) in tail.into_iter().zip(allocations) {
            let b = &mut self.cap.buffers[idx];
            b.num_planes = 1;
            b.len[0] = size;
            b.planes[0].length = size as u32;
            b.planes[0].m.fd = fd.as_raw_fd();
            b.import_fd = Some(fd);
            if debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: imported CAPTURE idx={idx} size={size} stride={stride} storage_height={storage_height}"
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixel_format::DecodedFormat;
    use std::fs::File;
    use std::os::fd::IntoRawFd;

    fn geometry(format: DecodedFormat) -> v4l2_pix_format_mplane {
        let mut pix: v4l2_pix_format_mplane = unsafe { std::mem::zeroed() };
        pix.width = 3840;
        pix.height = 2176;
        pix.pixelformat = format.v4l2_fourcc();
        pix.num_planes = 1;
        pix.plane_fmt[0].bytesperline = pix.width * format.bytes_per_sample();
        pix.plane_fmt[0].sizeimage = pix.plane_fmt[0].bytesperline * pix.height * 3 / 2;
        pix
    }

    #[test]
    fn drain_scratch_does_not_replace_a_surface_or_a_busy_capture_slot() {
        use super::super::{BufferState, V4l2Buffer};
        let mut session = super::super::submit::tests::streaming_session_with_pending_fifo(-1);
        session.out.streaming = false;
        session.fifo.clear();
        session.cap.fmt.fmt.pix_mp = geometry(DecodedFormat::Nv12);
        session.cap.buffers = (0..4).map(|_| V4l2Buffer::new()).collect();
        let surface = File::open("/dev/null").unwrap();
        let surface_fd = surface.as_raw_fd();
        session.cap.buffers[0].import_fd = Some(surface.into());
        session.cap.buffers[0].num_planes = 1;
        session.cap.buffers[0].state = BufferState::DirectComplete;
        session.cap.buffers[1].reserved_for = Some(9);
        session.cap.buffers[1].state = BufferState::Reserved;
        session.cap.buffers[2].state = BufferState::Publishing;
        assert!(session.prepare_drain_capture_with(|_| Err(())).is_err());
        assert_eq!(session.cap.buffers[3].num_planes, 0);
        assert!(session.cap.buffers[3].import_fd.is_none());
        assert_eq!(
            session.prepare_drain_capture_with(|size| {
                assert_eq!(size, 12_533_760);
                Ok(File::open("/dev/null").unwrap().into())
            }),
            Ok(3)
        );
        assert_eq!(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            surface_fd
        );
        assert!(session.cap.buffers[0].state == BufferState::DirectComplete);
        assert_eq!(session.cap.buffers[1].reserved_for, Some(9));
        assert!(session.cap.buffers[2].state == BufferState::Publishing);
        assert_eq!(session.cap.buffers[3].len[0], 12_533_760);
        assert!(session.cap.buffers[3].import_fd.is_some());
        assert!(session.cap.buffers[3].state == BufferState::Free);
    }

    #[test]
    fn capture_requires_display_drm_fd_before_opening_decoder() {
        for fd in [None, Some(-1)] {
            assert!(
                V4l2Session::open_and_setup(
                    3840,
                    2160,
                    super::super::V4L2_PIX_FMT_H264,
                    DecodedFormat::Nv12.v4l2_fourcc(),
                    fd,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn imported_storage_uses_padded_geometry_and_sample_depth() {
        assert_eq!(capture_size(&geometry(DecodedFormat::Nv12)), Ok(12_533_760));
        assert_eq!(capture_size(&geometry(DecodedFormat::P010)), Ok(25_067_520));
    }

    #[test]
    fn imported_storage_rejects_undersized_or_invalid_geometry() {
        let valid = geometry(DecodedFormat::Nv12);
        let mut bad = valid;
        bad.plane_fmt[0].sizeimage = 3840 * 2160 * 3 / 2;
        assert!(capture_size(&bad).is_err());
        bad = valid;
        bad.plane_fmt[0].bytesperline = 3839;
        assert!(capture_size(&bad).is_err());
        bad = valid;
        bad.num_planes = 2;
        assert!(capture_size(&bad).is_err());
        bad = valid;
        bad.height = u32::MAX;
        assert!(capture_size(&bad).is_err());
        bad = valid;
        bad.pixelformat = 0;
        assert!(capture_size(&bad).is_err());
        bad = valid;
        bad.plane_fmt[0].sizeimage = 128 * 1024 * 1024 + 1;
        assert!(capture_size(&bad).is_err());
    }

    #[test]
    fn growing_import_pool_is_transactional_and_preserves_live_slots() {
        let file = File::open("/dev/null").unwrap();
        let mut session = super::super::submit::tests::streaming_session_with_pending_fifo(
            file.try_clone().unwrap().into_raw_fd(),
        );
        session.fifo.clear();
        session.out.streaming = false;
        session.cap.fmt.fmt.pix_mp = geometry(DecodedFormat::Nv12);
        session.cap.buffers.clear();
        for _ in 0..3 {
            session.cap.buffers.push(super::super::V4l2Buffer::new());
        }
        let mut allocated = 0;
        assert!(
            session
                .initialize_imported_capture_with(|_| {
                    allocated += 1;
                    if allocated == 2 {
                        Err(())
                    } else {
                        Ok(file.try_clone().unwrap().into())
                    }
                })
                .is_err()
        );
        assert!(
            session
                .cap
                .buffers
                .iter()
                .all(|b| b.num_planes == 0 && b.import_fd.is_none())
        );
        session
            .initialize_imported_capture_with(|_| Ok(file.try_clone().unwrap().into()))
            .unwrap();
        let live_fd = session.cap.buffers[0]
            .import_fd
            .as_ref()
            .unwrap()
            .as_raw_fd();
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        let mut allocations = 0;
        session
            .initialize_imported_capture_with(|size| {
                assert_eq!(size, 12_533_760);
                allocations += 1;
                Ok(file.try_clone().unwrap().into())
            })
            .unwrap();
        assert_eq!(allocations, 1);
        assert_eq!(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            live_fd
        );
        assert!(
            session
                .cap
                .buffers
                .iter()
                .all(|b| b.addr[0].is_null() && b.len[0] == 12_533_760)
        );
    }

    #[test]
    fn eight_k_pool_budget_is_checked_before_allocating_or_replacing_slots() {
        let file = File::open("/dev/null").unwrap();
        let mut session = super::super::submit::tests::streaming_session_with_pending_fifo(
            file.try_clone().unwrap().into_raw_fd(),
        );
        session.fifo.clear();
        session.out.streaming = false;
        let mut pix = geometry(DecodedFormat::Nv12);
        pix.width = 7680;
        pix.height = 4320;
        pix.plane_fmt[0].bytesperline = 7680;
        pix.plane_fmt[0].sizeimage = 7680 * 4320 * 3 / 2;
        session.cap.fmt.fmt.pix_mp = pix;
        // 22 frames exceed 1 GiB; no allocation may occur before rejecting it.
        session.cap.buffers = (0..22).map(|_| super::super::V4l2Buffer::new()).collect();
        assert!(
            session
                .initialize_imported_capture_with(|_| panic!("over budget"))
                .is_err()
        );
        assert!(session.cap.buffers.iter().all(|b| b.import_fd.is_none()));
        session.cap.buffers.truncate(10);
        let mut allocations = 0;
        session
            .initialize_imported_capture_with(|size| {
                assert_eq!(size, 49_766_400);
                allocations += 1;
                Ok(file.try_clone().unwrap().into())
            })
            .unwrap();
        assert_eq!(allocations, 10);
        let live_fd = session.cap.buffers[0]
            .import_fd
            .as_ref()
            .unwrap()
            .as_raw_fd();
        session
            .cap
            .buffers
            .extend((0..12).map(|_| super::super::V4l2Buffer::new()));
        assert!(
            session
                .initialize_imported_capture_with(|_| panic!("over budget growth"))
                .is_err()
        );
        assert_eq!(
            session.cap.buffers[0]
                .import_fd
                .as_ref()
                .unwrap()
                .as_raw_fd(),
            live_fd
        );
    }

    #[test]
    fn imported_mapping_and_export_use_storage_fd_and_outlive_session() {
        use std::os::fd::FromRawFd;
        use std::os::unix::fs::FileExt;
        unsafe extern "C" {
            fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> i32;
        }
        let fd = unsafe { memfd_create(c"capture-import-test".as_ptr(), 1) };
        assert!(fd >= 0);
        let storage = unsafe { File::from_raw_fd(fd) };
        storage.set_len(12_533_760).unwrap();
        storage.write_all_at(&[0x51, 0x67], 0).unwrap();
        let mut session = super::super::submit::tests::streaming_session_with_pending_fifo(
            File::open("/dev/null").unwrap().into_raw_fd(),
        );
        session.fifo.clear();
        session.out.streaming = false;
        session.cap.fmt.fmt.pix_mp = geometry(DecodedFormat::Nv12);
        session.cap.buffers.push(super::super::V4l2Buffer::new());
        session
            .initialize_imported_capture_with(|_| Ok(storage.try_clone().unwrap().into()))
            .unwrap();
        // Mapping /dev/null would fail: the mapping must use the imported fd.
        session.map_buffer(false, 0).unwrap();
        let mapped =
            unsafe { std::slice::from_raw_parts(session.cap.buffers[0].addr[0].cast::<u8>(), 2) };
        assert_eq!(mapped, &[0x51, 0x67]);
        let export = session.export_capture(0).unwrap();
        assert_eq!(export.uv_offset, 3840 * 2176);
        assert_ne!(export.fd, storage.as_raw_fd());
        let exported = unsafe { File::from_raw_fd(export.fd) };
        drop(session);
        drop(storage);
        let mut data = [0; 2];
        exported.read_exact_at(&mut data, 0).unwrap();
        assert_eq!(data, [0x51, 0x67]);
    }
}
