//! Surface export bookkeeping.
//!
//! `vaExportSurfaceHandle` gives the client a dma-buf fd owned by the client.
//! The driver keeps one internal dup per surface-owned backing, independent of
//! how many times the client exports it. Client handles independently retain
//! the dma-buf after surface destruction. This module owns that fd accounting;
//! DRM PRIME descriptor layout stays in `va_drm.rs`.

use std::ffi::{c_int, c_void};
use std::os::fd::{BorrowedFd, OwnedFd};

use crate::bindings::*;
use crate::state::{DRV_MAX_SURFACE_EXPORTS, DriverState, Surface, SurfaceState, surface_index};
use crate::surface_backing::SurfaceBacking;
use crate::sync::sync_surface;
use crate::va_drm::{DrmPrimeDescriptor, DrmPrimeLayout, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2};
use crate::{err, ok, state_from_ctx, va_debug_enabled};

unsafe extern "C" {
    fn close(fd: c_int) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceExportError {
    InvalidSurface,
    Decoding,
    InvalidContext,
    TooManyExports,
    OperationFailed,
    AllocationBudget,
}

fn export_status(error: SurfaceExportError) -> VAStatus {
    match error {
        SurfaceExportError::InvalidSurface => err(VA_STATUS_ERROR_INVALID_SURFACE),
        SurfaceExportError::Decoding => err(VA_STATUS_ERROR_DECODING_ERROR),
        SurfaceExportError::InvalidContext => err(VA_STATUS_ERROR_INVALID_CONTEXT),
        SurfaceExportError::TooManyExports => err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED),
        SurfaceExportError::OperationFailed => err(VA_STATUS_ERROR_OPERATION_FAILED),
        SurfaceExportError::AllocationBudget => err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED),
    }
}

pub(crate) unsafe extern "C" fn export_surface_handle(
    ctx: VADriverContextP,
    surface_id: VASurfaceID,
    mem_type: u32,
    flags: u32,
    descriptor: *mut c_void,
) -> VAStatus {
    if va_debug_enabled() {
        eprintln!(
            "msm_drv_video_rs: ExportSurfaceHandle surface={} mem_type=0x{:x} flags=0x{:x}",
            surface_id, mem_type, flags
        );
    }
    if descriptor.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if mem_type != VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 {
        return err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE);
    }
    let layout = match DrmPrimeLayout::from_export_flags(flags) {
        Ok(layout) => layout,
        Err(()) => return err(VA_STATUS_ERROR_FLAG_NOT_SUPPORTED),
    };

    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let is_empty = match state.lock.lock() {
        Ok(guard) => surface_index(surface_id)
            .and_then(|idx| guard.surfaces.get(idx).and_then(|surface| surface.as_ref()))
            .is_some_and(|surface| surface.state == SurfaceState::Empty),
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if !is_empty {
        let sync = unsafe { sync_surface(ctx, surface_id) };
        if sync != ok() {
            return sync;
        }
    }
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let desc = match export_ready_surface(&mut guard, surface_id, layout) {
        Ok(desc) => desc,
        Err(error) => {
            if va_debug_enabled() {
                eprintln!(
                    "msm_drv_video_rs: ExportSurfaceHandle failed surface={} reason={:?}",
                    surface_id, error
                );
            }
            return export_status(error);
        }
    };
    unsafe { std::ptr::write(descriptor as *mut DrmPrimeDescriptor, desc) };
    if va_debug_enabled() {
        eprintln!(
            "msm_drv_video_rs: ExportSurfaceHandle succeeded surface={}",
            surface_id
        );
    }
    ok()
}

/// Close the driver-owned dups of exported fds and return how many entries were
/// retired. Client-owned descriptor fds are never closed here.
pub(crate) fn release_export_fds(surf: &mut Surface) -> usize {
    let fds = std::mem::take(&mut surf.export_fds);
    let n = fds.len();
    drop(fds);
    surf.exported = false;
    n
}

fn duplicate_export_fd(fd: c_int) -> Result<OwnedFd, ()> {
    if fd < 0 {
        return Err(());
    }
    // Rust duplicates with CLOEXEC atomically, including in multithreaded
    // media clients which may spawn helper processes while exporting.
    unsafe { BorrowedFd::borrow_raw(fd) }
        .try_clone_to_owned()
        .map_err(|_| ())
}

fn close_export_fd(fd: c_int) {
    if fd >= 0 {
        unsafe { close(fd) };
    }
}

// One GiB bounds all standalone PRIME storage owned by a display, independent
// of the existing per-session CAPTURE and two-context budgets.
const MAX_EXPORT_BACKING_BYTES: usize = 1024 * 1024 * 1024;

pub(crate) fn export_ready_surface(
    guard: &mut DriverState,
    surface_id: VASurfaceID,
    layout: DrmPrimeLayout,
) -> Result<DrmPrimeDescriptor, SurfaceExportError> {
    use std::os::fd::AsRawFd;
    let drm_fd = guard.drm_fd.as_ref().map(AsRawFd::as_raw_fd);
    export_ready_surface_with_allocator(
        guard,
        surface_id,
        layout,
        MAX_EXPORT_BACKING_BYTES,
        |width, height, format| SurfaceBacking::allocate_with_drm(width, height, format, drm_fd),
    )
}

fn export_ready_surface_with_allocator(
    guard: &mut DriverState,
    surface_id: VASurfaceID,
    layout: DrmPrimeLayout,
    allocation_budget: usize,
    allocate: impl FnOnce(
        u32,
        u32,
        crate::pixel_format::DecodedFormat,
    ) -> std::io::Result<SurfaceBacking>,
) -> Result<DrmPrimeDescriptor, SurfaceExportError> {
    export_ready_surface_with_operations(
        guard,
        surface_id,
        layout,
        allocation_budget,
        allocate,
        duplicate_export_fd,
    )
}

fn export_ready_surface_with_operations(
    guard: &mut DriverState,
    surface_id: VASurfaceID,
    layout: DrmPrimeLayout,
    allocation_budget: usize,
    allocate: impl FnOnce(
        u32,
        u32,
        crate::pixel_format::DecodedFormat,
    ) -> std::io::Result<SurfaceBacking>,
    duplicate: impl FnOnce(c_int) -> Result<OwnedFd, ()>,
) -> Result<DrmPrimeDescriptor, SurfaceExportError> {
    let failed = || SurfaceExportError::OperationFailed;
    let surf_idx = surface_index(surface_id).ok_or(SurfaceExportError::InvalidSurface)?;
    let surface = guard
        .surfaces
        .get(surf_idx)
        .and_then(Option::as_ref)
        .ok_or(SurfaceExportError::InvalidSurface)?;
    if !matches!(surface.state, SurfaceState::Empty | SurfaceState::Ready) {
        return Err(SurfaceExportError::Decoding);
    }
    if surface.export_fds.len() >= DRV_MAX_SURFACE_EXPORTS {
        return Err(SurfaceExportError::TooManyExports);
    }
    let mut new_backing = None;
    if surface.backing.is_none() {
        let width = u32::try_from(surface.width).map_err(|_| failed())?;
        let height = u32::try_from(surface.height).map_err(|_| failed())?;
        let format = surface.format;
        let required =
            SurfaceBacking::allocation_size(width, height, format).map_err(|_| failed())?;
        let allocated = guard
            .surfaces
            .iter()
            .flatten()
            .filter_map(|surface| surface.backing.as_ref())
            .try_fold(0usize, |total, backing| total.checked_add(backing.size()))
            .ok_or(SurfaceExportError::AllocationBudget)?;
        if allocated
            .checked_add(required)
            .is_none_or(|total| total > allocation_budget)
        {
            return Err(SurfaceExportError::AllocationBudget);
        }
        new_backing = Some(allocate(width, height, format).map_err(|_| failed())?);
    }
    let surface = guard.surfaces[surf_idx].as_mut().unwrap();
    let newly_allocated = new_backing.is_some();
    let backing = new_backing.as_mut().or(surface.backing.as_mut()).unwrap();
    if surface.state == SurfaceState::Ready && newly_allocated {
        let frame = surface.frame.as_ref().ok_or_else(failed)?;
        if backing.copy_frame(frame).is_err() {
            surface.state = SurfaceState::Dead;
            surface.frame = None;
            return Err(failed());
        }
    }
    // Surface-owned allocation permits export before BeginPicture even when
    // two identical decode contexts coexist. No decoder ownership is guessed.
    let desc = backing.descriptor(layout).map_err(|_| failed())?;
    let tracked = if newly_allocated || surface.export_fds.is_empty() {
        match duplicate(desc.objects[0].fd) {
            Ok(fd) => Some(fd),
            Err(()) => {
                close_export_fd(desc.objects[0].fd);
                return Err(failed());
            }
        }
    } else {
        // Every descriptor refers to the same surface-owned allocation. One
        // internal handle retains it; keeping a dup for every client request
        // grows without bound in clients which export each decoded frame.
        None
    };
    if let Some(backing) = new_backing {
        surface.backing = Some(backing);
        surface.export_fds.clear();
    }
    surface.exported = true;
    surface.export_count = surface.export_count.saturating_add(1);
    if let Some(tracked) = tracked {
        surface.export_fds.push(tracked);
    }
    Ok(desc)
}

#[cfg(test)]
pub(crate) fn export_ready_surface_for_test(
    guard: &mut DriverState,
    surface_id: VASurfaceID,
    layout: DrmPrimeLayout,
) -> Result<DrmPrimeDescriptor, SurfaceExportError> {
    export_ready_surface_with_allocator(
        guard,
        surface_id,
        layout,
        MAX_EXPORT_BACKING_BYTES,
        SurfaceBacking::allocate_for_test,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd};

    use crate::state::{DRV_ID_BASE_SURFACE, DRV_MAX_SURFACES};

    const F_GETFD: c_int = 1;
    const FD_CLOEXEC: c_int = 1;

    unsafe extern "C" {
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    }

    fn surface_with(state: SurfaceState, cap_idx: Option<usize>) -> Surface {
        Surface {
            backing: None,
            width: 64,
            height: 64,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state,
            cap_idx,
            frame: None,
            owner: VA_INVALID_ID,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        }
    }

    #[test]
    fn release_export_fds_clears_surface_export_state() {
        let mut surf = surface_with(SurfaceState::Ready, Some(0));
        surf.exported = true;
        surf.export_fds.push(duplicate_export_fd(1).unwrap());

        assert_eq!(release_export_fds(&mut surf), 1);
        assert!(!surf.exported);
        assert!(surf.export_fds.is_empty());
    }

    #[test]
    fn duplicate_export_fd_rejects_invalid_fd() {
        assert!(matches!(duplicate_export_fd(-1), Err(())));
    }

    #[test]
    fn tracked_export_fd_is_close_on_exec() {
        let tracked = duplicate_export_fd(1).unwrap();
        assert_ne!(
            unsafe { fcntl(tracked.as_raw_fd(), F_GETFD) } & FD_CLOEXEC,
            0
        );
    }

    #[test]
    fn dropping_surface_closes_tracked_export_fds() {
        use std::io::{ErrorKind, Read};
        use std::os::unix::net::UnixStream;

        let (mut reader, writer) = UnixStream::pair().unwrap();
        reader.set_nonblocking(true).unwrap();
        let tracked = duplicate_export_fd(writer.as_raw_fd()).unwrap();
        drop(writer);
        let mut surf = surface_with(SurfaceState::Ready, Some(0));
        surf.export_fds.push(tracked);

        let mut byte = [0];
        assert_eq!(
            reader.read(&mut byte).unwrap_err().kind(),
            ErrorKind::WouldBlock
        );
        drop(surf);
        // EOF proves the last peer was closed, even if another test reuses
        // its numeric fd between Drop and this read.
        assert_eq!(reader.read(&mut byte).unwrap(), 0);
    }

    #[test]
    fn export_rejects_missing_surface() {
        let mut guard = state_with_empty_surfaces();
        assert!(matches!(
            export_ready_surface(&mut guard, DRV_ID_BASE_SURFACE, DrmPrimeLayout::Composed),
            Err(SurfaceExportError::InvalidSurface)
        ));
    }

    #[test]
    fn export_rejects_non_ready_surface() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Pending, None));

        assert!(matches!(
            export_ready_surface(&mut guard, DRV_ID_BASE_SURFACE, DrmPrimeLayout::Composed),
            Err(SurfaceExportError::Decoding)
        ));
    }

    #[test]
    fn export_rejects_unbounded_fd_retention() {
        let mut guard = state_with_empty_surfaces();
        let mut surf = surface_with(SurfaceState::Ready, Some(0));
        for _ in 0..DRV_MAX_SURFACE_EXPORTS {
            surf.export_fds.push(duplicate_export_fd(1).unwrap());
        }
        guard.surfaces[0] = Some(surf);

        assert!(matches!(
            export_ready_surface(&mut guard, DRV_ID_BASE_SURFACE, DrmPrimeLayout::Composed),
            Err(SurfaceExportError::TooManyExports)
        ));
    }

    #[test]
    fn predecode_exports_do_not_require_or_guess_a_context() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Empty, None));
        let desc = export_ready_surface_for_test(
            &mut guard,
            DRV_ID_BASE_SURFACE,
            DrmPrimeLayout::Composed,
        )
        .unwrap();
        assert_eq!(guard.surfaces[0].as_ref().unwrap().owner, VA_INVALID_ID);
        assert_eq!(guard.surfaces[0].as_ref().unwrap().cap_idx, None);
        assert_eq!((desc.width, desc.height), (64, 64));
        assert_eq!(desc.num_layers, 1);
        let client = unsafe { std::os::fd::OwnedFd::from_raw_fd(desc.objects[0].fd) };
        let repeated = export_ready_surface_for_test(
            &mut guard,
            DRV_ID_BASE_SURFACE,
            DrmPrimeLayout::Separate,
        )
        .unwrap();
        let second = unsafe { std::os::fd::OwnedFd::from_raw_fd(repeated.objects[0].fd) };
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            std::fs::File::from(client).metadata().unwrap().ino(),
            std::fs::File::from(second).metadata().unwrap().ino()
        );
        assert_eq!(repeated.num_layers, 2);
        assert_eq!(guard.surfaces[0].as_ref().unwrap().export_fds.len(), 1);
    }

    #[test]
    fn repeated_frame_exports_are_bounded_and_clients_outlive_the_surface() {
        use crate::pixel_format::DecodedFormat;
        use crate::state::SurfaceFrame;
        use crate::v4l2::ReadyCapture;
        use std::os::unix::fs::{FileExt, MetadataExt};
        use std::sync::Arc;

        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let mut guard = state_with_empty_surfaces();
            let mut surface = surface_with(SurfaceState::Empty, None);
            surface.format = format;
            guard.surfaces[0] = Some(surface);
            let first = export_ready_surface_for_test(
                &mut guard,
                DRV_ID_BASE_SURFACE,
                DrmPrimeLayout::Composed,
            )
            .unwrap();
            let first = std::fs::File::from(unsafe { OwnedFd::from_raw_fd(first.objects[0].fd) });
            let inode = first.metadata().unwrap().ino();
            let mut last = None;

            for sequence in 0..1024 {
                crate::surface::release_surface_capture(&mut guard, 0);
                guard.surfaces[0].as_mut().unwrap().state = SurfaceState::Pending;
                let stride = 64 * format.bytes_per_sample();
                crate::sync::apply_ready_captures(
                    &mut guard,
                    VA_INVALID_ID,
                    vec![ReadyCapture {
                        failed: false,
                        surface: DRV_ID_BASE_SURFACE,
                        cap_idx: Some(0),
                        frame: Some(SurfaceFrame {
                            data: Arc::new(vec![(sequence % 251) as u8; (stride * 96) as usize]),
                            stride,
                            height: 64,
                            format,
                        }),
                    }],
                );
                let layout = if sequence % 2 == 0 {
                    DrmPrimeLayout::Separate
                } else {
                    DrmPrimeLayout::Composed
                };
                let desc =
                    export_ready_surface_for_test(&mut guard, DRV_ID_BASE_SURFACE, layout).unwrap();
                let client =
                    std::fs::File::from(unsafe { OwnedFd::from_raw_fd(desc.objects[0].fd) });
                assert_eq!(client.metadata().unwrap().ino(), inode);
                assert_eq!(guard.surfaces[0].as_ref().unwrap().export_fds.len(), 1);
                let mut pixel = [0];
                first.read_exact_at(&mut pixel, 0).unwrap();
                assert_eq!(pixel[0], (sequence % 251) as u8);
                last = Some(client);
            }
            assert_eq!(guard.surfaces[0].as_ref().unwrap().export_count, 1025);
            // Driver teardown closes its handles and mapping. The client's
            // handles independently retain the last published storage.
            drop(guard);
            for client in [first, last.unwrap()] {
                let mut pixel = [0];
                client.read_exact_at(&mut pixel, 0).unwrap();
                assert_eq!(pixel[0], (1023 % 251) as u8);
                assert_eq!(client.metadata().unwrap().ino(), inode);
            }
        }
    }

    #[test]
    fn repeated_export_needs_no_new_internal_duplicate_or_allocation() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Empty, None));
        let first = export_ready_surface_for_test(
            &mut guard,
            DRV_ID_BASE_SURFACE,
            DrmPrimeLayout::Separate,
        )
        .unwrap();
        let first = unsafe { OwnedFd::from_raw_fd(first.objects[0].fd) };
        drop(first);
        let repeated = export_ready_surface_with_operations(
            &mut guard,
            DRV_ID_BASE_SURFACE,
            DrmPrimeLayout::Composed,
            0,
            |_, _, _| panic!("existing storage must be reused"),
            |_| panic!("one internal duplicate already retains the storage"),
        )
        .unwrap();
        let _client = unsafe { OwnedFd::from_raw_fd(repeated.objects[0].fd) };
        assert_eq!(guard.surfaces[0].as_ref().unwrap().export_fds.len(), 1);
    }

    #[test]
    fn export_allocation_budget_and_failed_allocator_do_not_claim_storage() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Empty, None));
        let size =
            SurfaceBacking::allocation_size(64, 64, crate::pixel_format::DecodedFormat::Nv12)
                .unwrap();
        assert!(matches!(
            export_ready_surface_with_allocator(
                &mut guard,
                DRV_ID_BASE_SURFACE,
                DrmPrimeLayout::Composed,
                size - 1,
                |_, _, _| panic!("budget must be checked before allocation"),
            ),
            Err(SurfaceExportError::AllocationBudget)
        ));
        assert!(guard.surfaces[0].as_ref().unwrap().backing.is_none());
        assert!(matches!(
            export_ready_surface_with_allocator(
                &mut guard,
                DRV_ID_BASE_SURFACE,
                DrmPrimeLayout::Composed,
                size,
                |_, _, _| Err(std::io::Error::other("allocation failed")),
            ),
            Err(SurfaceExportError::OperationFailed)
        ));
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert!(surface.backing.is_none());
        assert!(surface.export_fds.is_empty());
        assert_eq!(surface.state, SurfaceState::Empty);
        assert_eq!(surface.owner, VA_INVALID_ID);
    }

    #[test]
    fn failed_export_duplication_does_not_commit_new_backing_or_budget() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Empty, None));
        let allocated = std::cell::Cell::new(false);
        let duplicated = std::cell::Cell::new(false);
        assert!(matches!(
            export_ready_surface_with_operations(
                &mut guard,
                DRV_ID_BASE_SURFACE,
                DrmPrimeLayout::Composed,
                MAX_EXPORT_BACKING_BYTES,
                |width, height, format| {
                    allocated.set(true);
                    SurfaceBacking::allocate_for_test(width, height, format)
                },
                |_| {
                    duplicated.set(true);
                    Err(())
                },
            ),
            Err(SurfaceExportError::OperationFailed)
        ));
        assert!(allocated.get());
        assert!(duplicated.get());
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert!(surface.backing.is_none());
        assert!(surface.export_fds.is_empty());
        assert!(!surface.exported);
        assert_eq!(surface.export_count, 0);
        assert_eq!(surface.state, SurfaceState::Empty);
        assert_eq!(surface.owner, VA_INVALID_ID);
    }

    #[test]
    fn failed_ready_copy_drops_new_storage_before_budget_commit() {
        let mut guard = state_with_empty_surfaces();
        let mut surface = surface_with(SurfaceState::Ready, None);
        surface.frame = Some(crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![0; 1]),
            stride: 1,
            height: 64,
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        guard.surfaces[0] = Some(surface);
        assert!(matches!(
            export_ready_surface_for_test(
                &mut guard,
                DRV_ID_BASE_SURFACE,
                DrmPrimeLayout::Composed,
            ),
            Err(SurfaceExportError::OperationFailed)
        ));
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert!(surface.backing.is_none());
        assert!(surface.export_fds.is_empty());
        assert_eq!(surface.state, SurfaceState::Dead);
        assert!(surface.frame.is_none());
    }

    fn state_with_empty_surfaces() -> DriverState {
        DriverState {
            drm_fd: None,
            configs: Vec::new(),
            contexts: Vec::new(),
            buffers: Vec::new(),
            images: Vec::new(),
            surfaces: empty_surfaces(),
        }
    }

    fn empty_surfaces() -> Vec<Option<Surface>> {
        let mut v: Vec<Option<Surface>> = Vec::with_capacity(DRV_MAX_SURFACES);
        v.resize_with(DRV_MAX_SURFACES, || None);
        v
    }
}
