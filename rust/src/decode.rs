//! H.264 picture lifecycle callbacks.
//!
//! VA clients submit parsed H.264 parameter and slice buffers between
//! `vaBeginPicture` and `vaEndPicture`. This module translates those buffers
//! into the Rust H.264 assembler and submits the resulting Annex-B frame to
//! the stateful V4L2 session.

use crate::bindings::*;
use crate::h264::H264Slice;
use crate::state::{
    DRV_MAX_RENDER_BUFFERS, DRV_MAX_SLICES_PER_FRAME, SurfaceState, buffer_index, context_index,
    surface_index,
};
use crate::surface::release_surface_capture;
use crate::sync::pump_and_publish;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::ptr;

pub(crate) unsafe extern "C" fn begin_picture(
    ctx: VADriverContextP,
    context: VAContextID,
    render_target: VASurfaceID,
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
        let cap_idx = guard.contexts[ctx_idx]
            .as_mut()
            .and_then(|context| context.v4l2.as_mut())
            .and_then(|v4l2| v4l2.reserve_capture())
            .ok_or(())
            .map_err(|_| err(VA_STATUS_ERROR_OPERATION_FAILED));
        let cap_idx = match cap_idx {
            Ok(cap_idx) => cap_idx,
            Err(status) => return status,
        };
        if let Some(surface) = guard.surfaces[surf_idx].as_mut() {
            surface.cap_idx = Some(cap_idx);
        }
    }
    let Some(c) = guard.contexts[ctx_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    c.frame_open = true;
    c.render_target = render_target;
    c.slices.clear();
    c.syn.begin_picture();
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

    let mut copied = Vec::with_capacity(num_buffers as usize);
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
        copied.push(buf.clone());
    }

    let Some(c) = guard.contexts[ctx_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    for buf in copied {
        match buf.type_ {
            VABufferType::VAPictureParameterBufferType => {
                if (buf.elem_size as usize) < std::mem::size_of::<VAPictureParameterBufferH264>()
                    || buf.data.len() < std::mem::size_of::<VAPictureParameterBufferH264>()
                {
                    return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                }
                let pp = unsafe {
                    ptr::read_unaligned(buf.data.as_ptr() as *const VAPictureParameterBufferH264)
                };
                c.syn.set_picture_params(pp);
            }
            VABufferType::VAIQMatrixBufferType => {
                if buf.data.len() >= std::mem::size_of::<VAIQMatrixBufferH264>() {
                    let iq = unsafe {
                        ptr::read_unaligned(buf.data.as_ptr() as *const VAIQMatrixBufferH264)
                    };
                    c.syn.set_iq_matrix(iq);
                }
            }
            VABufferType::VASliceParameterBufferType => {
                if (buf.elem_size as usize) < std::mem::size_of::<VASliceParameterBufferH264>() {
                    return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                }
                if c.slices
                    .len()
                    .checked_add(buf.num_elements as usize)
                    .is_none_or(|count| count > DRV_MAX_SLICES_PER_FRAME)
                {
                    return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
                }
                for j in 0..buf.num_elements as usize {
                    let off = j
                        .checked_mul(buf.elem_size as usize)
                        .ok_or(())
                        .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER));
                    let off = match off {
                        Ok(v) => v,
                        Err(e) => return e,
                    };
                    let Some(end) =
                        off.checked_add(std::mem::size_of::<VASliceParameterBufferH264>())
                    else {
                        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                    };
                    if end > buf.data.len() {
                        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                    }
                    let sp = unsafe {
                        ptr::read_unaligned(
                            buf.data.as_ptr().add(off) as *const VASliceParameterBufferH264
                        )
                    };
                    c.slices.push(H264Slice {
                        sp,
                        data: Vec::new(),
                    });
                }
            }
            VABufferType::VASliceDataBufferType => {
                let Some(first) = c.slices.iter().position(|s| s.data.is_empty()) else {
                    return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                };
                let total = (buf.elem_size as usize).saturating_mul(buf.num_elements as usize);
                if total > buf.data.len() {
                    return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                }
                for slice in c.slices.iter_mut().skip(first) {
                    if !slice.data.is_empty() {
                        continue;
                    }
                    let off = slice.sp.slice_data_offset as usize;
                    let size = slice.sp.slice_data_size as usize;
                    let Some(end) = off.checked_add(size) else {
                        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                    };
                    if size == 0 || end > total {
                        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
                    }
                    slice.data = buf.data[off..end].to_vec();
                }
            }
            _ => return err(VA_STATUS_ERROR_UNSUPPORTED_BUFFERTYPE),
        }
    }
    ok()
}

pub(crate) unsafe extern "C" fn end_picture(
    ctx: VADriverContextP,
    context: VAContextID,
) -> VAStatus {
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
    let cap_idx = surface_index(render_target)
        .and_then(|surface_idx| guard.surfaces[surface_idx].as_ref())
        .and_then(|surface| surface.cap_idx);
    let Some(c) = guard.contexts[ctx_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if !c.frame_open {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if !c.syn.have_pp || c.slices.is_empty() || c.slices.iter().any(|s| s.data.is_empty()) {
        c.frame_open = false;
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(frame) = c.syn.assemble_frame(&c.slices) else {
        c.frame_open = false;
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    };
    if let Ok(prefix) = std::env::var("V4L2_VA_DUMP")
        && !prefix.is_empty()
    {
        let path = format!("{}_{:02}.bin", prefix, c.out_seq);
        let _ = std::fs::write(path, &frame.bytes);
    }
    let _ = frame.emitted_headers;
    let poc = c.syn.pp.CurrPic.TopFieldOrderCnt;
    let timestamp_usec = if poc >= 0 {
        ((poc as u64) * 100_000 + 3) / 6
    } else {
        0
    };
    let keyframe = frame.bytes.windows(5).any(|w| w == [0, 0, 0, 1, 0x65]);
    if std::env::var_os("V4L2_VA_DEBUG").is_some() {
        eprintln!(
            "msm_drv_video_rs: EndPicture context={} surface={} seq={} bytes={} ts={} keyframe={}",
            context,
            render_target,
            c.out_seq,
            frame.bytes.len(),
            timestamp_usec,
            keyframe
        );
    }
    let submit = c
        .v4l2
        .as_mut()
        .ok_or(())
        .and_then(|v| {
            v.submit_frame(
                render_target,
                cap_idx,
                &frame.bytes,
                keyframe,
                timestamp_usec,
                &c.syn.header_bytes(),
            )
        })
        .map_err(|_| err(VA_STATUS_ERROR_DECODING_ERROR));
    c.out_seq = c.out_seq.saturating_add(1);
    c.slices.clear();
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
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_ID_BASE_CONTEXT, DRV_ID_BASE_SURFACE, DriverBox, Surface};
    use std::ffi::c_void;

    #[test]
    fn begin_picture_validates_context_before_retiring_surface() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().surfaces[0] = Some(Surface {
            width: 320,
            height: 240,
            state: SurfaceState::Pending,
            cap_idx: Some(2),
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
                slices: Vec::new(),
                syn: crate::h264::H264Synth::new(VAProfile::VAProfileH264Main),
                out_seq: 0,
                v4l2: None,
            });
            guard.surfaces[0] = Some(Surface {
                width: 320,
                height: 240,
                state: SurfaceState::Pending,
                cap_idx: Some(2),
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
}
