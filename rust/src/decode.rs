//! Codec-neutral VA picture lifecycle callbacks.
//!
//! Codec modules translate VA buffers into complete compressed access units;
//! this module validates handles, owns surface state transitions, and submits
//! those access units to the stateful V4L2 session.

use crate::bindings::*;
use crate::state::{
    DRV_MAX_RENDER_BUFFERS, DriverState, SurfaceState, buffer_index, context_index, surface_index,
};
use crate::surface::release_surface_capture;
use crate::sync::{pump_and_publish, sync_surface};
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::os::fd::AsRawFd;

pub(crate) unsafe extern "C" fn begin_picture(
    ctx: VADriverContextP,
    context: VAContextID,
    render_target: VASurfaceID,
) -> VAStatus {
    unsafe { begin_picture_inner(ctx, context, render_target, true) }
}

unsafe fn begin_picture_inner(
    ctx: VADriverContextP,
    context: VAContextID,
    render_target: VASurfaceID,
    wait_for_pending: bool,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(ctx_idx) = context_index(context) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let Some(surf_idx) = surface_index(render_target) else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if guard.surfaces[surf_idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    }
    if guard.contexts[ctx_idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    }
    if guard.contexts[ctx_idx]
        .as_ref()
        .is_some_and(|c| !c.render_targets.is_empty() && !c.render_targets.contains(&render_target))
    {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    }
    if guard.contexts[ctx_idx]
        .as_ref()
        .is_some_and(|c| c.frame_open)
    {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    let format = crate::pixel_format::DecodedFormat::from_profile(
        guard.contexts[ctx_idx].as_ref().unwrap().profile,
    );
    if guard.surfaces[surf_idx].as_ref().unwrap().format != format {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    }
    // A READY/EMPTY surface can still own a reservation or retained export in
    // another live session. Never transfer it merely because no frame is pending.
    if guard.surfaces[surf_idx]
        .as_ref()
        .is_some_and(|surface| surface.owner != VA_INVALID_ID && surface.owner != context)
    {
        return err(VA_STATUS_ERROR_SURFACE_BUSY);
    }
    pump_and_publish(&mut guard, ctx_idx, 0);
    if wait_for_pending
        && guard.surfaces[surf_idx].as_ref().is_some_and(|surface| {
            surface.state == SurfaceState::Pending && surface.owner == context
        })
    {
        // A client flush can release its old references and reuse their VA
        // surfaces while stateful firmware still owns the previous pictures.
        // Complete the old decode before releasing its CAPTURE reservation or
        // accepting the new IDR. Rejecting it loses the seek's reference chain.
        drop(guard);
        let status = unsafe { sync_surface(ctx, render_target) };
        if status != ok() {
            return status;
        }
        // Sync releases the lock. Revalidate all handles and picture state;
        // permit only one wait so another caller cannot make this unbounded.
        return unsafe { begin_picture_inner(ctx, context, render_target, false) };
    }
    if guard.surfaces[surf_idx].as_ref().is_some_and(|surface| {
        matches!(
            surface.state,
            SurfaceState::InProgress | SurfaceState::Pending
        )
    }) {
        // Reusing a still-pending ID lets an old completion publish into the
        // new picture, and can return its CAPTURE slot to firmware too early.
        return err(VA_STATUS_ERROR_SURFACE_BUSY);
    }
    if std::env::var_os("V4L2_VA_DEBUG").is_some()
        && let Some(surf) = guard.surfaces[surf_idx].as_ref()
    {
        eprintln!(
            "msm_drv_video_rs: BeginPicture context={} surface={} state={:?} cap_idx={:?} exported={} export_fds={}",
            context,
            render_target,
            surf.state,
            surf.cap_idx,
            surf.exported,
            surf.export_fds.len()
        );
    }
    let stable_capture = guard.contexts[ctx_idx]
        .as_ref()
        .and_then(|context| context.v4l2.as_ref())
        .is_some_and(|v4l2| v4l2.stable_capture_mode());
    let preserve_capture = stable_capture
        && guard.surfaces[surf_idx].as_ref().is_some_and(|surface| {
            matches!(surface.state, SurfaceState::Empty | SurfaceState::Ready)
                && surface.cap_idx.is_some()
                && surface.cap_idx.is_some_and(|cap_idx| {
                    guard.contexts[ctx_idx]
                        .as_ref()
                        .and_then(|context| context.v4l2.as_ref())
                        .is_some_and(|v4l2| v4l2.capture_is_live(cap_idx))
                })
        });
    if !preserve_capture {
        release_surface_capture(&mut guard, surf_idx);
    }
    if stable_capture
        && guard.surfaces[surf_idx]
            .as_ref()
            .and_then(|surface| surface.cap_idx)
            .is_none()
    {
        // Born-stable sessions always have reservation slack. A session
        // converted mid-flight (first post-decode export while a legacy
        // queue still saturates CAPTURE) starves until completions drain
        // the kernel queue; that must not fail the frame. The surface
        // decodes without a reservation (dequeue then publishes the
        // working slot directly) and can still be stabilized later at
        // export time once slack exists.
        if let Some(cap_idx) = guard.contexts[ctx_idx]
            .as_mut()
            .and_then(|context| context.v4l2.as_mut())
            .and_then(|v4l2| v4l2.reserve_capture(render_target))
        {
            if let Some(surface) = guard.surfaces[surf_idx].as_mut() {
                surface.cap_idx = Some(cap_idx);
            }
        } else if std::env::var_os("V4L2_VA_DEBUG").is_some() {
            eprintln!(
                "msm_drv_video_rs: BeginPicture surface={} stable reservation starved; decoding without a reservation",
                render_target
            );
        }
    }
    let previous_frame = guard.surfaces[surf_idx]
        .as_mut()
        .and_then(|surface| surface.frame.take());
    let Some(c) = guard.contexts[ctx_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if let Some(frame) = previous_frame
        && let Some(session) = c.v4l2.as_mut()
    {
        session.recycle_snapshot(frame);
    }
    c.frame_open = true;
    c.render_target = render_target;
    c.decoder.begin_picture();
    if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
        surf.state = SurfaceState::InProgress;
        surf.owner = context;
    }
    ok()
}

pub(crate) unsafe extern "C" fn render_picture(
    ctx: VADriverContextP,
    context: VAContextID,
    buffers: *mut VABufferID,
    num_buffers: c_int,
) -> VAStatus {
    if num_buffers < 0
        || (num_buffers > 0 && buffers.is_null())
        || (num_buffers as usize) > DRV_MAX_RENDER_BUFFERS
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(ctx_idx) = context_index(context) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(c) = guard.contexts[ctx_idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if !c.frame_open {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }

    let mut indices = Vec::with_capacity(num_buffers as usize);
    for i in 0..num_buffers as usize {
        let id = unsafe { *buffers.add(i) };
        let Some(idx) = buffer_index(id) else {
            return err(VA_STATUS_ERROR_INVALID_BUFFER);
        };
        let Some(buf) = guard.buffers[idx].as_ref() else {
            return err(VA_STATUS_ERROR_INVALID_BUFFER);
        };
        if buf.owner != context {
            return err(VA_STATUS_ERROR_INVALID_BUFFER);
        }
        if buf.mapped {
            return err(VA_STATUS_ERROR_OPERATION_FAILED);
        }
        indices.push(idx);
    }

    // Borrow the decoder and buffer table separately; cloning every payload
    // doubles peak memory and copies large compressed frames unnecessarily.
    let DriverState {
        contexts, buffers, ..
    } = &mut *guard;
    let c = contexts[ctx_idx].as_mut().unwrap();
    for idx in indices {
        if let Err(status) = c.decoder.render_buffer(buffers[idx].as_ref().unwrap()) {
            fail_picture(&mut guard, ctx_idx);
            return status;
        }
    }
    ok()
}

pub(crate) unsafe extern "C" fn end_picture(
    ctx: VADriverContextP,
    context: VAContextID,
) -> VAStatus {
    let timing = std::env::var_os("V4L2_VA_DEBUG").is_some();
    let started = std::time::Instant::now();
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(ctx_idx) = context_index(context) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let render_target = guard.contexts[ctx_idx]
        .as_ref()
        .map(|context| context.render_target)
        .unwrap_or(VA_INVALID_ID);
    let Some(open_context) = guard.contexts[ctx_idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if !open_context.frame_open {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if let Err(status) = validate_av1_transport_surfaces(&guard, ctx_idx, context, render_target) {
        fail_picture(&mut guard, ctx_idx);
        return status;
    }
    let cap_idx = surface_index(render_target)
        .and_then(|surface_idx| guard.surfaces[surface_idx].as_ref())
        .and_then(|surface| surface.cap_idx);
    let Some(c) = guard.contexts[ctx_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if !c.frame_open {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    let frame = match c.decoder.finish_picture(c.out_seq) {
        Ok(frame) => frame,
        Err(status) => {
            fail_picture(&mut guard, ctx_idx);
            return status;
        }
    };
    if frame.bytes.is_empty() {
        fail_picture(&mut guard, ctx_idx);
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if let Ok(prefix) = std::env::var("V4L2_VA_DUMP")
        && !prefix.is_empty()
    {
        let path = format!("{}_{:02}.bin", prefix, c.out_seq);
        let _ = std::fs::write(path, &frame.bytes);
    }
    if std::env::var_os("V4L2_VA_DEBUG").is_some() {
        eprintln!(
            "msm_drv_video_rs: EndPicture context={} surface={} seq={} bytes={} ts={} keyframe={}",
            context,
            render_target,
            c.out_seq,
            frame.bytes.len(),
            frame.timestamp_usec,
            frame.keyframe
        );
    }
    let surf_idx = surface_index(render_target).unwrap();
    let can_direct = c.v4l2.is_some()
        && !matches!(
            c.profile,
            VAProfile::VAProfileAV1Profile0 | VAProfile::VAProfileAV1Profile1
        );
    // All declared targets must fit the same direct layout before streaming.
    // One incompatible caller import keeps this whole context in copy mode.
    let can_direct = can_direct
        && !guard.contexts[ctx_idx]
            .as_ref()
            .unwrap()
            .render_targets
            .iter()
            .filter_map(|&id| surface_index(id).and_then(|idx| guard.surfaces[idx].as_ref()))
            .any(|s| {
                s.backing
                    .as_ref()
                    .is_some_and(|b| b.is_imported() && !b.supports_direct_decode())
            });
    if can_direct && guard.surfaces[surf_idx].as_ref().unwrap().backing.is_none() {
        let surface = guard.surfaces[surf_idx].as_ref().unwrap();
        let required = match crate::surface_backing::SurfaceBacking::allocation_size(
            surface.width as u32,
            surface.height as u32,
            surface.format,
        ) {
            Ok(size) => size,
            Err(_) => {
                fail_picture(&mut guard, ctx_idx);
                return err(VA_STATUS_ERROR_ALLOCATION_FAILED);
            }
        };
        let allocated = guard
            .surfaces
            .iter()
            .flatten()
            .filter_map(|s| s.backing.as_ref())
            .map(|b| b.size())
            .sum::<usize>();
        if allocated
            .checked_add(required)
            .is_none_or(|n| n > crate::surface_export::MAX_EXPORT_BACKING_BYTES)
        {
            fail_picture(&mut guard, ctx_idx);
            return err(VA_STATUS_ERROR_ALLOCATION_FAILED);
        }
        let drm_fd = guard.drm_fd.as_ref().map(AsRawFd::as_raw_fd);
        let backing = match crate::surface_backing::SurfaceBacking::allocate_with_drm(
            surface.width as u32,
            surface.height as u32,
            surface.format,
            drm_fd,
        ) {
            Ok(backing) => backing,
            Err(_) => {
                fail_picture(&mut guard, ctx_idx);
                return err(VA_STATUS_ERROR_ALLOCATION_FAILED);
            }
        };
        guard.surfaces[surf_idx].as_mut().unwrap().backing = Some(backing);
    }
    let prepared = started.elapsed();
    let target = if can_direct {
        match guard.surfaces[surf_idx]
            .as_ref()
            .unwrap()
            .backing
            .as_ref()
            .unwrap()
            .decode_target()
        {
            Ok(target) => target,
            Err(_) => {
                fail_picture(&mut guard, ctx_idx);
                return err(VA_STATUS_ERROR_DECODING_ERROR);
            }
        }
    } else {
        None
    };
    let target_ready = started.elapsed();
    let direct_copy = guard.surfaces[surf_idx]
        .as_ref()
        .unwrap()
        .backing
        .as_ref()
        .is_some_and(|backing| backing.can_download());
    let c = guard.contexts[ctx_idx].as_mut().unwrap();
    if target.is_none() && c.v4l2.as_ref().is_some_and(|v| v.direct_capture_mode()) {
        // Undeclared targets cannot switch an active direct session to an
        // incompatible imported layout or reuse the preceding surface's fd.
        fail_picture(&mut guard, ctx_idx);
        return err(VA_STATUS_ERROR_DECODING_ERROR);
    }
    if let Some(target) = target
        && c.v4l2
            .as_mut()
            .unwrap()
            .bind_decode_target(render_target, target)
            .is_err()
    {
        fail_picture(&mut guard, ctx_idx);
        return err(VA_STATUS_ERROR_DECODING_ERROR);
    }
    let direct_decode = c.v4l2.as_ref().is_some_and(|v| v.direct_capture_mode());
    let submit = c
        .v4l2
        .as_mut()
        .ok_or(())
        .and_then(|v| {
            v.submit_frame(
                render_target,
                cap_idx,
                &frame.bytes,
                frame.keyframe,
                frame.expects_output,
                frame.timestamp_usec,
                &frame.headers,
                direct_copy,
            )?;
            if let Some(show) = frame.vp9_show_existing {
                // Stateful Iris suppresses hidden reference output. The
                // standard show_existing command exports that decoded reference
                // while retaining the original bitstream's entropy/MV state.
                v.submit_frame(
                    render_target,
                    cap_idx,
                    &[show],
                    false,
                    true,
                    frame.timestamp_usec,
                    &[],
                    direct_copy,
                )?;
            }
            Ok(())
        })
        .map_err(|_| err(VA_STATUS_ERROR_DECODING_ERROR));
    c.out_seq = c.out_seq.saturating_add(1);
    c.frame_open = false;
    if let Err(e) = submit {
        if let Some(idx) = surface_index(c.render_target)
            && let Some(surf) = guard.surfaces[idx].as_mut()
        {
            surf.state = SurfaceState::Dead;
        }
        return e;
    }
    if let Some(idx) = surface_index(c.render_target)
        && let Some(surf) = guard.surfaces[idx].as_mut()
    {
        surf.state = SurfaceState::Pending;
        surf.owner = context;
    }
    // Submit pacing may have completed earlier frames inside the V4L2 session;
    // surface that progress now so pipelining clients can recycle surfaces.
    pump_and_publish(&mut guard, ctx_idx, 0);
    let surface = guard.surfaces[surf_idx].as_ref().unwrap();
    let sync_submission = if direct_decode {
        surface
            .backing
            .as_ref()
            .is_some_and(|b| b.requires_submission_sync())
    } else {
        surface.exported
    };
    drop(guard);
    let submitted = started.elapsed();
    if sync_submission {
        // Iris does not install a completion fence in the surface's dma_resv.
        // Finish pre-exported targets before a client can sample them. Other
        // clients wait through SyncSurface/export; CAPTURE rebinds after each
        // completion in decode order. Release the driver lock while waiting.
        let status = unsafe { sync_surface(ctx, render_target) };
        if timing {
            eprintln!(
                "msm_drv_video_rs: decode_timing surface={} prepare_us={} target_us={} submit_us={} sync_us={} total_us={} epoch_us={}",
                render_target,
                prepared.as_micros(),
                target_ready.saturating_sub(prepared).as_micros(),
                submitted.saturating_sub(target_ready).as_micros(),
                started.elapsed().saturating_sub(submitted).as_micros(),
                started.elapsed().as_micros(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros(),
            );
        }
        return status;
    }
    ok()
}

/// Validate original VA surface identities under the same driver lock as
/// EndPicture. The paired transport never accepts another context's references
/// or a current picture that would overwrite a still-referenced VA surface.
fn validate_av1_transport_surfaces(
    state: &DriverState,
    ctx_idx: usize,
    context: VAContextID,
    target: VASurfaceID,
) -> Result<(), VAStatus> {
    let Some(ctx) = state.contexts[ctx_idx].as_ref() else {
        return Ok(());
    };
    let Some(pp) = ctx.decoder.transport_picture() else {
        return Ok(());
    };
    // Without film grain, standard producers may omit the separate display
    // picture. The reconstruction surface must still be this BeginPicture target.
    if pp.current_frame != target
        || (pp.current_display_picture != VA_INVALID_ID && pp.current_display_picture != target)
    {
        return Err(err(VA_STATUS_ERROR_INVALID_SURFACE));
    }
    let width = i32::from(pp.frame_width_minus1) + 1;
    let height = i32::from(pp.frame_height_minus1) + 1;
    let format = crate::pixel_format::DecodedFormat::from_profile(ctx.profile);
    let current = surface_index(target)
        .and_then(|idx| state.surfaces[idx].as_ref())
        .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_SURFACE))?;
    if current.owner != context
        || current.state != SurfaceState::InProgress
        || current.format != format
        || current.width < width
        || current.height < height
    {
        return Err(err(VA_STATUS_ERROR_INVALID_SURFACE));
    }
    for id in pp
        .ref_frame_map
        .iter()
        .copied()
        .filter(|id| *id != VA_INVALID_ID)
    {
        let surface = surface_index(id)
            .and_then(|idx| state.surfaces[idx].as_ref())
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_SURFACE))?;
        if id == target
            || surface.owner != context
            || surface.format != format
            || !matches!(surface.state, SurfaceState::Pending | SurfaceState::Ready)
        {
            return Err(err(VA_STATUS_ERROR_INVALID_SURFACE));
        }
    }
    Ok(())
}

// An unsuccessful picture has no completion to publish. Close it and discard
// stale pixels so sync/status report an error and teardown or the next picture
// can proceed, including after a malformed codec buffer.
fn fail_picture(guard: &mut DriverState, ctx_idx: usize) {
    if let Some(context) = guard.contexts[ctx_idx].as_mut() {
        context.frame_open = false;
        if let Some(surface_idx) = surface_index(context.render_target)
            && let Some(surface) = guard.surfaces[surface_idx].as_mut()
        {
            surface.state = SurfaceState::Dead;
            surface.frame = None;
        }
        context.render_target = VA_INVALID_ID;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE, DriverBox, Surface};
    use std::ffi::c_void;

    #[test]
    fn paired_av1_reference_surfaces_reject_cross_context_dead_missing_and_current_alias() {
        let driver = DriverBox::new();
        let mut state = driver.lock.lock().unwrap();
        let mut decoder = crate::codec::RawDecoder::new_cbs_transport_for_test();
        let mut pp: VADecPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pp.current_frame = DRV_ID_BASE_SURFACE;
        pp.current_display_picture = VA_INVALID_ID;
        pp.frame_width_minus1 = 1279;
        pp.frame_height_minus1 = 719;
        pp.ref_frame_map = [VA_INVALID_ID; 8];
        pp.ref_frame_map[0] = DRV_ID_BASE_SURFACE + 1;
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&pp as *const VADecPictureParameterBufferAV1).cast::<u8>(),
                std::mem::size_of_val(&pp),
            )
        };
        decoder
            .render_buffer(&crate::state::Buffer {
                owner: DRV_ID_BASE_CONTEXT,
                type_: VABufferType::VAPictureParameterBufferType,
                elem_size: bytes.len() as u32,
                num_elements: 1,
                data: bytes.to_vec(),
                mapped: false,
            })
            .unwrap();
        state.contexts[0] = Some(crate::state::Context {
            config_id: VA_INVALID_ID,
            profile: VAProfile::VAProfileAV1Profile0,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 1280,
            height: 720,
            render_targets: vec![DRV_ID_BASE_SURFACE],
            frame_open: true,
            render_target: DRV_ID_BASE_SURFACE,
            decoder: crate::codec::Decoder::Raw(Box::new(decoder)),
            out_seq: 0,
            v4l2: None,
        });
        for index in 0..2 {
            state.surfaces[index] = Some(Surface {
                backing: None,
                width: 1280,
                height: 720,
                format: crate::pixel_format::DecodedFormat::Nv12,
                state: if index == 0 {
                    SurfaceState::InProgress
                } else {
                    SurfaceState::Pending
                },
                cap_idx: None,
                frame: None,
                owner: DRV_ID_BASE_CONTEXT,
                exported: false,
                export_count: 0,
                export_fds: Vec::new(),
            });
        }
        assert_eq!(
            validate_av1_transport_surfaces(&state, 0, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE),
            Ok(())
        );
        state.surfaces[1].as_mut().unwrap().owner += 1;
        assert!(
            validate_av1_transport_surfaces(&state, 0, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE)
                .is_err()
        );
        state.surfaces[1].as_mut().unwrap().owner = DRV_ID_BASE_CONTEXT;
        state.surfaces[1].as_mut().unwrap().state = SurfaceState::Dead;
        assert!(
            validate_av1_transport_surfaces(&state, 0, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE)
                .is_err()
        );
        state.surfaces[1].as_mut().unwrap().state = SurfaceState::Ready;
        assert_eq!(
            validate_av1_transport_surfaces(&state, 0, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE),
            Ok(())
        );
        assert!(
            validate_av1_transport_surfaces(
                &state,
                0,
                DRV_ID_BASE_CONTEXT,
                DRV_ID_BASE_SURFACE + 1
            )
            .is_err()
        );
        state.surfaces[1] = None;
        assert!(
            validate_av1_transport_surfaces(&state, 0, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE)
                .is_err()
        );
    }

    #[test]
    fn begin_picture_validates_context_before_retiring_surface() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().surfaces[0] = Some(Surface {
            backing: None,
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Pending,
            cap_idx: Some(2),
            frame: None,
            owner: DRV_ID_BASE_CONTEXT,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });

        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            VA_STATUS_ERROR_INVALID_CONTEXT as VAStatus
        );
        assert_eq!(
            state.lock.lock().unwrap().surfaces[0]
                .as_ref()
                .and_then(|surface| surface.cap_idx),
            Some(2)
        );

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn nested_begin_does_not_retire_the_current_surface() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            guard.contexts[0] = Some(crate::state::Context {
                config_id: VA_INVALID_ID,
                profile: VAProfile::VAProfileH264Main,
                entrypoint: VAEntrypoint::VAEntrypointVLD,
                width: 320,
                height: 240,
                render_targets: vec![DRV_ID_BASE_SURFACE],
                frame_open: true,
                render_target: DRV_ID_BASE_SURFACE,
                decoder: crate::codec::Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
                out_seq: 0,
                v4l2: None,
            });
            guard.surfaces[0] = Some(Surface {
                backing: None,
                width: 320,
                height: 240,
                format: crate::pixel_format::DecodedFormat::Nv12,
                state: SurfaceState::Pending,
                cap_idx: Some(2),
                frame: None,
                owner: DRV_ID_BASE_CONTEXT,
                exported: true,
                export_count: 1,
                export_fds: Vec::new(),
            });
        }

        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        {
            let guard = state.lock.lock().unwrap();
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.cap_idx, Some(2));
            assert!(surface.exported);
            assert_eq!(
                guard.contexts[0].as_ref().unwrap().render_target,
                DRV_ID_BASE_SURFACE
            );
        }

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn render_picture_rejects_oversized_buffer_lists() {
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };

        assert_eq!(
            unsafe {
                render_picture(
                    &mut ctx,
                    VA_INVALID_ID,
                    std::ptr::null_mut(),
                    (DRV_MAX_RENDER_BUFFERS + 1) as c_int,
                )
            },
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );
    }
    fn picture_fixture() -> DriverBox {
        let state = DriverBox::new();
        let mut guard = state.lock.lock().unwrap();
        guard.contexts[0] = Some(crate::state::Context {
            config_id: VA_INVALID_ID,
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 320,
            height: 240,
            render_targets: Vec::new(),
            frame_open: false,
            render_target: VA_INVALID_ID,
            decoder: crate::codec::Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
            out_seq: 0,
            v4l2: None,
        });
        guard.surfaces[0] = Some(Surface {
            backing: None,
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Empty,
            cap_idx: None,
            frame: None,
            owner: DRV_ID_BASE_CONTEXT,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });
        drop(guard);
        state
    }

    #[test]
    fn begin_picture_cannot_steal_foreign_ready_or_empty_capture() {
        let state = picture_fixture();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &state as *const DriverBox as *mut c_void;
        for surface_state in [SurfaceState::Empty, SurfaceState::Ready] {
            {
                let mut guard = state.lock.lock().unwrap();
                let surface = guard.surfaces[0].as_mut().unwrap();
                surface.owner = DRV_ID_BASE_CONTEXT + 1;
                surface.state = surface_state;
                surface.cap_idx = Some(9);
                surface.exported = true;
                surface.export_count = 1;
            }
            assert_eq!(
                unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
                err(VA_STATUS_ERROR_SURFACE_BUSY)
            );
            let guard = state.lock.lock().unwrap();
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.owner, DRV_ID_BASE_CONTEXT + 1);
            assert_eq!(surface.state, surface_state);
            assert_eq!(surface.cap_idx, Some(9));
            assert!(surface.exported);
            assert_eq!(surface.export_count, 1);
            assert!(!guard.contexts[0].as_ref().unwrap().frame_open);
        }
    }

    #[test]
    fn incomplete_picture_fails_sync_and_can_be_restarted_or_destroyed() {
        let state = picture_fixture();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &state as *const DriverBox as *mut c_void;
        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            ok()
        );
        assert_eq!(
            unsafe { end_picture(&mut ctx, DRV_ID_BASE_CONTEXT) },
            err(VA_STATUS_ERROR_INVALID_PARAMETER)
        );
        assert_eq!(
            unsafe { crate::sync::sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 0) },
            err(VA_STATUS_ERROR_DECODING_ERROR)
        );
        assert!(
            !state.lock.lock().unwrap().contexts[0]
                .as_ref()
                .unwrap()
                .frame_open
        );
        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            ok()
        );
        assert_eq!(
            unsafe { end_picture(&mut ctx, DRV_ID_BASE_CONTEXT) },
            err(VA_STATUS_ERROR_INVALID_PARAMETER)
        );
        assert_eq!(
            unsafe { crate::context::destroy_context(&mut ctx, DRV_ID_BASE_CONTEXT) },
            ok()
        );
    }

    #[test]
    fn malformed_render_aborts_picture_but_mapped_buffer_can_be_retried() {
        let state = picture_fixture();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &state as *const DriverBox as *mut c_void;
        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            ok()
        );
        state.lock.lock().unwrap().buffers[0] = Some(crate::state::Buffer {
            owner: DRV_ID_BASE_CONTEXT,
            type_: VABufferType::VAPictureParameterBufferType,
            elem_size: 1,
            num_elements: 1,
            data: vec![0],
            mapped: true,
        });
        let mut id = crate::state::DRV_ID_BASE_BUFFER;
        assert_eq!(
            unsafe { render_picture(&mut ctx, DRV_ID_BASE_CONTEXT, &mut id, 1) },
            err(VA_STATUS_ERROR_OPERATION_FAILED)
        );
        assert!(
            state.lock.lock().unwrap().contexts[0]
                .as_ref()
                .unwrap()
                .frame_open
        );
        state.lock.lock().unwrap().buffers[0]
            .as_mut()
            .unwrap()
            .mapped = false;
        assert_eq!(
            unsafe { render_picture(&mut ctx, DRV_ID_BASE_CONTEXT, &mut id, 1) },
            err(VA_STATUS_ERROR_INVALID_PARAMETER)
        );
        assert_eq!(
            unsafe { crate::sync::sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 0) },
            err(VA_STATUS_ERROR_DECODING_ERROR)
        );
        assert_eq!(
            unsafe { crate::context::destroy_context(&mut ctx, DRV_ID_BASE_CONTEXT) },
            ok()
        );
    }

    #[test]
    fn begin_rejects_wrong_format_and_pending_surface_without_changing_ownership() {
        let state = picture_fixture();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &state as *const DriverBox as *mut c_void;
        state.lock.lock().unwrap().surfaces[0]
            .as_mut()
            .unwrap()
            .format = crate::pixel_format::DecodedFormat::P010;
        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            err(VA_STATUS_ERROR_INVALID_SURFACE)
        );
        {
            let mut guard = state.lock.lock().unwrap();
            let surface = guard.surfaces[0].as_mut().unwrap();
            surface.format = crate::pixel_format::DecodedFormat::Nv12;
            surface.state = SurfaceState::Pending;
            surface.cap_idx = Some(3);
        }
        assert_eq!(
            unsafe {
                begin_picture_inner(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE, false)
            },
            err(VA_STATUS_ERROR_SURFACE_BUSY)
        );
        assert_eq!(
            state.lock.lock().unwrap().surfaces[0]
                .as_ref()
                .unwrap()
                .cap_idx,
            Some(3)
        );
    }

    #[test]
    fn seek_surface_reuse_waits_for_prior_completion_without_holding_lock() {
        let state = picture_fixture();
        {
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[0].as_mut().unwrap().state = SurfaceState::Pending;
        }
        // The scoped thread cannot outlive the driver. Access its raw VA
        // state only through the same mutex used by the C callbacks.
        let state_addr = (&state as *const DriverBox) as usize;
        std::thread::scope(|scope| {
            scope.spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(5));
                let state = unsafe { &*(state_addr as *const DriverBox) };
                state.lock.lock().unwrap().surfaces[0]
                    .as_mut()
                    .unwrap()
                    .state = SurfaceState::Ready;
            });
            let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
            ctx.pDriverData = &state as *const DriverBox as *mut c_void;
            assert_eq!(
                unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
                ok()
            );
        });
        let guard = state.lock.lock().unwrap();
        assert_eq!(
            guard.surfaces[0].as_ref().unwrap().state,
            SurfaceState::InProgress
        );
        assert!(guard.contexts[0].as_ref().unwrap().frame_open);
    }

    #[test]
    fn zero_timeout_does_not_start_or_complete_an_open_picture() {
        let state = picture_fixture();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &state as *const DriverBox as *mut c_void;
        assert_eq!(
            unsafe { begin_picture(&mut ctx, DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE) },
            ok()
        );
        assert_eq!(
            unsafe { crate::sync::sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 0) },
            err(VA_STATUS_ERROR_TIMEDOUT)
        );
        assert!(
            state.lock.lock().unwrap().contexts[0]
                .as_ref()
                .unwrap()
                .frame_open
        );
    }
}
