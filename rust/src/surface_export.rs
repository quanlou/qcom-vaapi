//! Surface export bookkeeping.
//!
//! `vaExportSurfaceHandle` gives the client a dma-buf fd owned by the client.
//! The driver also keeps a dup of that fd in the surface so the exported
//! dma-buf object stays alive until the VA surface is destroyed or reused. This
//! module owns that fd accounting; DRM PRIME descriptor layout stays in
//! `va_drm.rs`.

use std::ffi::{c_int, c_void};
use std::os::fd::{FromRawFd, OwnedFd};

use crate::bindings::*;
use crate::state::{
    DRV_ID_BASE_CONTEXT, DRV_MAX_SURFACE_EXPORTS, DriverState, Surface, SurfaceState,
    context_index, surface_index,
};
use crate::sync::sync_surface;
use crate::va_drm::{DrmPrimeDescriptor, DrmPrimeLayout, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2};
use crate::{err, ok, state_from_ctx, va_debug_enabled};

unsafe extern "C" {
    fn dup(fd: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SurfaceExportError {
    InvalidSurface,
    Decoding,
    InvalidContext,
    TooManyExports,
    OperationFailed,
}

fn export_status(error: SurfaceExportError) -> VAStatus {
    match error {
        SurfaceExportError::InvalidSurface => err(VA_STATUS_ERROR_INVALID_SURFACE),
        SurfaceExportError::Decoding => err(VA_STATUS_ERROR_DECODING_ERROR),
        SurfaceExportError::InvalidContext => err(VA_STATUS_ERROR_INVALID_CONTEXT),
        SurfaceExportError::TooManyExports => err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED),
        SurfaceExportError::OperationFailed => err(VA_STATUS_ERROR_OPERATION_FAILED),
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
    let owned = unsafe { dup(fd) };
    if owned >= 0 {
        Ok(unsafe { OwnedFd::from_raw_fd(owned) })
    } else {
        Err(())
    }
}

fn close_export_fd(fd: c_int) {
    if fd >= 0 {
        unsafe { close(fd) };
    }
}

pub(crate) fn export_ready_surface(
    guard: &mut DriverState,
    surface_id: VASurfaceID,
    layout: DrmPrimeLayout,
) -> Result<DrmPrimeDescriptor, SurfaceExportError> {
    let Some(surf_idx) = surface_index(surface_id) else {
        return Err(SurfaceExportError::InvalidSurface);
    };
    let Some((surface_state, owner, existing_cap_idx)) = guard.surfaces[surf_idx]
        .as_ref()
        .map(|surf| (surf.state, surf.owner, surf.cap_idx))
    else {
        return Err(SurfaceExportError::InvalidSurface);
    };
    if !matches!(surface_state, SurfaceState::Empty | SurfaceState::Ready) {
        return Err(SurfaceExportError::Decoding);
    }
    if guard.surfaces[surf_idx]
        .as_ref()
        .is_some_and(|surf| surf.export_fds.len() >= DRV_MAX_SURFACE_EXPORTS)
    {
        return Err(SurfaceExportError::TooManyExports);
    }
    let ctx_idx = if let Some(ctx_idx) = context_index(owner)
        && guard.contexts.get(ctx_idx).is_some_and(Option::is_some)
    {
        ctx_idx
    } else if surface_state == SurfaceState::Empty {
        // GStreamer creates its VA surface pool without render targets, then
        // exports those surfaces before the first BeginPicture associates one
        // with the sole decoder context. Bind that pre-decode export here so
        // the reserved CAPTURE allocation is the one later queued for decode.
        let mut live_contexts = guard
            .contexts
            .iter()
            .enumerate()
            .filter_map(|(idx, context)| context.as_ref().map(|_| idx));
        let Some(ctx_idx) = live_contexts.next() else {
            return Err(SurfaceExportError::InvalidContext);
        };
        if live_contexts.next().is_some() {
            return Err(SurfaceExportError::InvalidContext);
        }
        if let Some(surface) = guard.surfaces[surf_idx].as_mut() {
            surface.owner = DRV_ID_BASE_CONTEXT + ctx_idx as u32;
        }
        ctx_idx
    } else {
        return Err(SurfaceExportError::InvalidContext);
    };
    let stable_capture = guard.contexts[ctx_idx]
        .as_ref()
        .and_then(|context| context.v4l2.as_ref())
        .is_some_and(|v4l2| v4l2.stable_capture_mode());
    if surface_state == SurfaceState::Ready && !stable_capture {
        return Err(SurfaceExportError::OperationFailed);
    }
    let cap_idx = if let Some(cap_idx) = existing_cap_idx {
        cap_idx
    } else if surface_state == SurfaceState::Empty {
        let Some(cap_idx) = guard.contexts[ctx_idx]
            .as_mut()
            .and_then(|context| context.v4l2.as_mut())
            .and_then(|v4l2| v4l2.reserve_capture())
        else {
            return Err(SurfaceExportError::OperationFailed);
        };
        if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
            surf.cap_idx = Some(cap_idx);
        }
        cap_idx
    } else {
        return Err(SurfaceExportError::Decoding);
    };
    let Some(capture) = guard.contexts[ctx_idx]
        .as_ref()
        .and_then(|c| c.v4l2.as_ref())
        .and_then(|v| v.export_capture(cap_idx))
    else {
        return Err(SurfaceExportError::OperationFailed);
    };

    let owned = match duplicate_export_fd(capture.fd) {
        Ok(fd) => fd,
        Err(()) => {
            close_export_fd(capture.fd);
            return Err(SurfaceExportError::OperationFailed);
        }
    };
    let desc = DrmPrimeDescriptor::from_nv12_capture(capture, layout);
    if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
        surf.exported = true;
        surf.export_count = surf.export_count.saturating_add(1);
        surf.export_fds.push(owned);
        if std::env::var_os("V4L2_VA_DEBUG").is_some() {
            eprintln!(
                "msm_drv_video_rs: export surface={} cap_idx={} export_count={} tracked_fds={}",
                surface_id,
                cap_idx,
                surf.export_count,
                surf.export_fds.len()
            );
        }
    }
    Ok(desc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;

    use crate::state::{DRV_ID_BASE_SURFACE, DRV_MAX_SURFACES};

    const F_GETFD: c_int = 1;

    unsafe extern "C" {
        fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
    }

    fn surface_with(state: SurfaceState, cap_idx: Option<usize>) -> Surface {
        Surface {
            width: 64,
            height: 64,
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
    fn dropping_surface_closes_tracked_export_fds() {
        let tracked = duplicate_export_fd(1).unwrap();
        let tracked_raw = tracked.as_raw_fd();
        let mut surf = surface_with(SurfaceState::Ready, Some(0));
        surf.export_fds.push(tracked);

        assert!(unsafe { fcntl(tracked_raw, F_GETFD) } >= 0);
        drop(surf);
        assert_eq!(unsafe { fcntl(tracked_raw, F_GETFD) }, -1);
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

    fn state_with_empty_surfaces() -> DriverState {
        DriverState {
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
