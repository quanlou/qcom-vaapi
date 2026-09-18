//! VA surface status and decode-error reporting.
//!
//! `vaQuerySurfaceStatus` exposes only coarse readiness: libva has no failed
//! status enum variant. Decode failures are reported through
//! `vaQuerySurfaceError`, so both callbacks need to stay consistent.

use crate::bindings::*;
use crate::state::{DRV_ID_BASE_SURFACE, SurfaceState, context_index, surface_index};
use crate::sync::pump_and_publish;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_void;
use std::ptr;

pub(crate) unsafe extern "C" fn query_surface_status(
    ctx: VADriverContextP,
    render_target: VASurfaceID,
    status: *mut VASurfaceStatus,
) -> VAStatus {
    if status.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if render_target < DRV_ID_BASE_SURFACE {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    }
    let idx = (render_target - DRV_ID_BASE_SURFACE) as usize;
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if idx >= guard.surfaces.len() || guard.surfaces[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    }
    if let Some(owner) = guard.surfaces[idx].as_ref().map(|s| s.owner)
        && let Some(ctx_idx) = context_index(owner)
    {
        pump_and_publish(&mut guard, ctx_idx, 0);
        if guard.contexts[ctx_idx]
            .as_ref()
            .and_then(|c| c.v4l2.as_ref())
            .is_some_and(|v| v.failed())
            && let Some(surf) = guard.surfaces[idx].as_mut()
            && matches!(surf.state, SurfaceState::InProgress | SurfaceState::Pending)
        {
            surf.state = SurfaceState::Dead;
        }
    }
    let surf = guard.surfaces[idx].as_ref().unwrap();
    unsafe {
        *status = if surf.state == SurfaceState::Ready {
            VASurfaceStatus::VASurfaceReady
        } else {
            VASurfaceStatus::VASurfaceRendering
        }
    };
    ok()
}

pub(crate) unsafe extern "C" fn query_surface_error(
    ctx: VADriverContextP,
    render_target: VASurfaceID,
    _error_status: VAStatus,
    error_info: *mut *mut c_void,
) -> VAStatus {
    if !error_info.is_null() {
        unsafe { *error_info = ptr::null_mut() };
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = surface_index(render_target) else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(surf) = guard.surfaces[idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let session_failed = context_index(surf.owner)
        .and_then(|ctx_idx| guard.contexts[ctx_idx].as_ref())
        .and_then(|context| context.v4l2.as_ref())
        .is_some_and(|v| v.failed());
    if surf.state == SurfaceState::Dead || session_failed {
        err(VA_STATUS_ERROR_DECODING_ERROR)
    } else {
        ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DriverBox, Surface};

    fn surface_with(state: SurfaceState) -> Surface {
        Surface {
            width: 16,
            height: 16,
            state,
            cap_idx: None,
            owner: VA_INVALID_ID,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        }
    }

    #[test]
    fn query_surface_status_reports_ready_only_for_ready_surfaces() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[0] = Some(surface_with(SurfaceState::Ready));
            guard.surfaces[1] = Some(surface_with(SurfaceState::Pending));
            guard.surfaces[2] = Some(surface_with(SurfaceState::Dead));
        }

        let mut status = VASurfaceStatus::VASurfaceRendering;
        assert_eq!(
            unsafe { query_surface_status(&mut ctx, DRV_ID_BASE_SURFACE, &mut status) },
            ok()
        );
        assert_eq!(status, VASurfaceStatus::VASurfaceReady);

        assert_eq!(
            unsafe { query_surface_status(&mut ctx, DRV_ID_BASE_SURFACE + 1, &mut status) },
            ok()
        );
        assert_eq!(status, VASurfaceStatus::VASurfaceRendering);

        assert_eq!(
            unsafe { query_surface_status(&mut ctx, DRV_ID_BASE_SURFACE + 2, &mut status) },
            ok()
        );
        assert_eq!(status, VASurfaceStatus::VASurfaceRendering);

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn query_surface_error_reports_dead_surface_decode_failure() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[0] = Some(surface_with(SurfaceState::Ready));
            guard.surfaces[1] = Some(surface_with(SurfaceState::Dead));
        }

        let mut error_info = std::ptr::without_provenance_mut::<c_void>(usize::MAX);
        assert_eq!(
            unsafe {
                query_surface_error(
                    &mut ctx,
                    DRV_ID_BASE_SURFACE,
                    VA_STATUS_ERROR_DECODING_ERROR as VAStatus,
                    &mut error_info,
                )
            },
            ok()
        );
        assert!(error_info.is_null());

        error_info = std::ptr::without_provenance_mut::<c_void>(usize::MAX);
        assert_eq!(
            unsafe {
                query_surface_error(
                    &mut ctx,
                    DRV_ID_BASE_SURFACE + 1,
                    VA_STATUS_ERROR_DECODING_ERROR as VAStatus,
                    &mut error_info,
                )
            },
            VA_STATUS_ERROR_DECODING_ERROR as VAStatus
        );
        assert!(error_info.is_null());

        unsafe { drop(Box::from_raw(raw)) };
    }
}
