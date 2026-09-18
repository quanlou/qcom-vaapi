//! Surface readiness publication.
//!
//! V4L2 decode completion is asynchronous: a CAPTURE buffer can finish while
//! the app is submitting more frames, polling surface status, or syncing a
//! surface. These helpers are the single place that turns finished CAPTURE
//! buffers into VA surface state so clients such as GStreamer do not starve the
//! CAPTURE queue by waiting for an explicit `vaSyncSurface`.

use crate::bindings::*;
use crate::state::{DriverState, SurfaceState, context_index, surface_index};
use crate::v4l2::ReadyCapture;
use crate::{err, ok, state_from_ctx, va_debug_enabled};

/// Apply CAPTURE buffers that finished decoding to VA surface state.
///
/// The V4L2 backend accumulates finished captures in `V4l2Session.ready` while
/// it is pumping. Clients that pipeline frames without syncing depend on this
/// state being published as soon as it exists. Idempotent: surfaces already
/// Ready simply get the latest capture slot.
pub(crate) fn apply_ready_captures(guard: &mut DriverState, ready: Vec<ReadyCapture>) {
    for r in ready {
        if let Some(idx) = surface_index(r.surface)
            && let Some(s) = guard.surfaces[idx].as_mut()
        {
            if std::env::var_os("V4L2_VA_DEBUG").is_some() {
                eprintln!(
                    "msm_drv_video_rs: publish surface={} cap_idx={} previous_state={:?} previous_cap={:?} export_fds={}",
                    r.surface,
                    r.cap_idx,
                    s.state,
                    s.cap_idx,
                    s.export_fds.len()
                );
            }
            s.cap_idx = Some(r.cap_idx);
            // A PRIME export is a handle to the CAPTURE allocation, not a
            // one-frame lease. Keep the bookkeeping live while the same VA
            // surface is reused so importers can retain the fd across frames.
            s.exported = !s.export_fds.is_empty();
            s.state = SurfaceState::Ready;
        }
    }
}

/// Non-blocking pump of a context's V4L2 session, publishing any finished
/// captures to VA surface state. Safe to call anywhere the driver lock is held.
pub(crate) fn pump_and_publish(guard: &mut DriverState, ctx_idx: usize, timeout_ms: i32) {
    let ready = guard
        .contexts
        .get_mut(ctx_idx)
        .and_then(|c| c.as_mut())
        .and_then(|c| c.v4l2.as_mut().map(|v| v.pump(timeout_ms)))
        .unwrap_or_default();
    apply_ready_captures(guard, ready);
}

pub(crate) unsafe extern "C" fn sync_surface(
    ctx: VADriverContextP,
    surface: VASurfaceID,
) -> VAStatus {
    unsafe { sync_surface2(ctx, surface, 10_000_000_000) }
}

pub(crate) unsafe extern "C" fn sync_surface2(
    ctx: VADriverContextP,
    surface: VASurfaceID,
    timeout_ns: u64,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(surf_idx) = surface_index(surface) else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let start = std::time::Instant::now();
    let deadline = start + std::time::Duration::from_nanos(timeout_ns.max(1));
    loop {
        let mut guard = match state.lock.lock() {
            Ok(g) => g,
            Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
        };
        let Some(surf) = guard.surfaces[surf_idx].as_ref() else {
            return err(VA_STATUS_ERROR_INVALID_SURFACE);
        };
        match surf.state {
            SurfaceState::Ready => return ok(),
            SurfaceState::Dead | SurfaceState::Empty => return err(VA_STATUS_ERROR_DECODING_ERROR),
            SurfaceState::InProgress | SurfaceState::Pending => {}
        }
        let owner = surf.owner;
        let Some(ctx_idx) = context_index(owner) else {
            return err(VA_STATUS_ERROR_INVALID_CONTEXT);
        };
        let Some(c) = guard.contexts[ctx_idx].as_mut() else {
            return err(VA_STATUS_ERROR_INVALID_CONTEXT);
        };
        // A sync call can be made while the client is still feeding the same
        // context. Let the queue drain naturally; issuing DECODER_CMD STOP
        // here would race a later vaEndPicture and strand its OUTPUT packet.
        let ready = if let Some(v4l2) = c.v4l2.as_mut() {
            v4l2.pump(5)
        } else {
            Vec::new()
        };
        let v4l2_eos = c
            .v4l2
            .as_ref()
            .is_some_and(|v| v.eos() && v.pending_count() == 0);
        let v4l2_failed = c.v4l2.as_ref().is_some_and(|v| v.failed());
        apply_ready_captures(&mut guard, ready);
        if guard.surfaces[surf_idx]
            .as_ref()
            .is_some_and(|s| s.state == SurfaceState::Ready)
        {
            return ok();
        }
        if v4l2_eos {
            if let Some(s) = guard.surfaces[surf_idx].as_mut()
                && s.state == SurfaceState::Pending
            {
                s.state = SurfaceState::Dead;
            }
            return err(VA_STATUS_ERROR_DECODING_ERROR);
        }
        if v4l2_failed {
            if let Some(s) = guard.surfaces[surf_idx].as_mut()
                && matches!(s.state, SurfaceState::InProgress | SurfaceState::Pending)
            {
                s.state = SurfaceState::Dead;
            }
            return err(VA_STATUS_ERROR_DECODING_ERROR);
        }
        let timed_out = std::time::Instant::now() >= deadline;
        if timed_out && va_debug_enabled() {
            // Snapshot while the lock is still held: surface bookkeeping plus
            // the owning session's OUTPUT/CAPTURE queue state. This is the
            // error path, so an extra allocation is acceptable.
            let (state, cap_idx) = guard.surfaces[surf_idx]
                .as_ref()
                .map(|s| (s.state, s.cap_idx))
                .unwrap_or((SurfaceState::Empty, None));
            let session = guard.contexts[ctx_idx]
                .as_ref()
                .and_then(|c| c.v4l2.as_ref())
                .map(|v| v.debug_snapshot())
                .unwrap_or_else(|| "no v4l2 session".to_string());
            eprintln!(
                "msm_drv_video_rs: vaSyncSurface timed out surface=0x{:x} state={:?} cap_idx={:?} ctx_idx={} elapsed_ms={} timeout_ns={} session={}",
                surface,
                state,
                cap_idx,
                ctx_idx,
                start.elapsed().as_millis(),
                timeout_ns,
                session,
            );
        }
        drop(guard);
        if timed_out {
            return err(VA_STATUS_ERROR_TIMEDOUT);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_ID_BASE_SURFACE, DRV_MAX_SURFACES, Surface};
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

    fn surface_with(state: SurfaceState, cap_idx: Option<usize>) -> Surface {
        Surface {
            width: 64,
            height: 64,
            state,
            cap_idx,
            owner: VA_INVALID_ID,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        }
    }

    #[test]
    fn publish_marks_pending_surface_ready_with_capture() {
        let mut guard = state_with_empty_surfaces();
        let surf_id = DRV_ID_BASE_SURFACE + 7;
        guard.surfaces[7] = Some(surface_with(SurfaceState::Pending, None));

        apply_ready_captures(
            &mut guard,
            vec![ReadyCapture {
                surface: surf_id,
                cap_idx: 3,
            }],
        );

        let s = guard.surfaces[7].as_ref().unwrap();
        assert_eq!(s.state, SurfaceState::Ready);
        assert_eq!(s.cap_idx, Some(3));
        assert!(!s.exported);
    }

    #[test]
    fn publish_ignores_unknown_and_destroyed_surfaces() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[2] = Some(surface_with(SurfaceState::Ready, Some(1)));

        apply_ready_captures(
            &mut guard,
            vec![
                ReadyCapture {
                    surface: DRV_ID_BASE_SURFACE + 9_999,
                    cap_idx: 0,
                },
                ReadyCapture {
                    surface: VA_INVALID_ID,
                    cap_idx: 1,
                },
            ],
        );

        let s = guard.surfaces[2].as_ref().unwrap();
        assert_eq!(s.state, SurfaceState::Ready);
        assert_eq!(s.cap_idx, Some(1));
    }

    #[test]
    fn publish_supersedes_previous_capture_slot() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[0] = Some(surface_with(SurfaceState::Ready, Some(10)));

        apply_ready_captures(
            &mut guard,
            vec![ReadyCapture {
                surface: DRV_ID_BASE_SURFACE,
                cap_idx: 11,
            }],
        );

        let s = guard.surfaces[0].as_ref().unwrap();
        assert_eq!(s.cap_idx, Some(11));
    }

    #[test]
    fn publish_preserves_exported_surface_backing() {
        let mut guard = state_with_empty_surfaces();
        let file = std::fs::File::open("/dev/null").unwrap();
        guard.surfaces[0] = Some(Surface {
            width: 64,
            height: 64,
            state: SurfaceState::Pending,
            cap_idx: Some(4),
            owner: VA_INVALID_ID,
            exported: true,
            export_count: 1,
            export_fds: vec![unsafe { OwnedFd::from_raw_fd(file.into_raw_fd()) }],
        });

        apply_ready_captures(
            &mut guard,
            vec![ReadyCapture {
                surface: DRV_ID_BASE_SURFACE,
                cap_idx: 4,
            }],
        );

        let surface = guard.surfaces[0].as_ref().unwrap();
        assert!(surface.exported);
        assert_eq!(surface.export_fds.len(), 1);
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
