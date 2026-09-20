//! VA decode-context lifecycle.
//!
//! A context owns one codec-specific access-unit assembler and one stateful
//! V4L2 session. The
//! callbacks here only validate VA handles and construct or retire that owner;
//! picture submission remains in `decode.rs`.

use crate::bindings::*;
use crate::codec::{Codec, Decoder};
use crate::state::{
    Context, DRV_ID_BASE_CONTEXT, DRV_MAX_DIM, DRV_MAX_SURFACES, DRV_MIN_DIM, SurfaceState,
    config_index, context_index, surface_index,
};
use crate::surface::release_surface_capture;
use crate::sync::apply_ready_captures;
use crate::v4l2::V4l2Session;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::slice;

pub(crate) unsafe extern "C" fn create_context(
    ctx: VADriverContextP,
    config_id: VAConfigID,
    picture_width: c_int,
    picture_height: c_int,
    _flag: c_int,
    render_targets: *mut VASurfaceID,
    num_render_targets: c_int,
    context: *mut VAContextID,
) -> VAStatus {
    if context.is_null()
        || num_render_targets < 0
        || (num_render_targets > 0 && render_targets.is_null())
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&picture_width)
        || !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&picture_height)
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(cfg_idx) = config_index(config_id) else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    let Some(cfg) = guard.configs.get(cfg_idx).and_then(|c| c.as_ref()).cloned() else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    if cfg.entrypoint != VAEntrypoint::VAEntrypointVLD {
        return err(VA_STATUS_ERROR_UNSUPPORTED_ENTRYPOINT);
    }
    if guard.contexts.iter().any(Option::is_some) {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    if num_render_targets as usize > DRV_MAX_SURFACES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let render_targets = if num_render_targets > 0 {
        let targets = unsafe { slice::from_raw_parts(render_targets, num_render_targets as usize) };
        for &surface_id in targets {
            let Some(surface_idx) = surface_index(surface_id) else {
                return err(VA_STATUS_ERROR_INVALID_SURFACE);
            };
            if guard.surfaces[surface_idx].is_none() {
                return err(VA_STATUS_ERROR_INVALID_SURFACE);
            }
        }
        targets.to_vec()
    } else {
        Vec::new()
    };
    let Some(codec) = Codec::from_profile(cfg.profile) else {
        return err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE);
    };
    let Some(decoder) = Decoder::new(cfg.profile) else {
        return err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE);
    };
    let Ok(v4l2) = V4l2Session::open_and_setup(
        picture_width,
        picture_height,
        codec.fourcc(),
        cfg.format.v4l2_fourcc(),
    ) else {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    };
    if let Some(idx) = guard.contexts.iter().position(Option::is_none) {
        let context_id = DRV_ID_BASE_CONTEXT + idx as u32;
        for &surface_id in &render_targets {
            if let Some(surface_idx) = surface_index(surface_id)
                && let Some(surface) = guard.surfaces[surface_idx].as_mut()
            {
                surface.owner = context_id;
            }
        }
        guard.contexts[idx] = Some(Context {
            config_id,
            profile: cfg.profile,
            entrypoint: cfg.entrypoint,
            width: picture_width,
            height: picture_height,
            render_targets,
            frame_open: false,
            render_target: VA_INVALID_ID,
            decoder,
            out_seq: 0,
            v4l2: Some(v4l2),
        });
        unsafe { *context = context_id };
        ok()
    } else {
        err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED)
    }
}

pub(crate) unsafe extern "C" fn destroy_context(
    ctx: VADriverContextP,
    context: VAContextID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = context_index(context) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if guard.contexts[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    }
    if guard.contexts[idx]
        .as_ref()
        .is_some_and(|context| context.frame_open)
    {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if guard
        .buffers
        .iter()
        .flatten()
        .any(|buffer| buffer.owner == context && buffer.mapped)
    {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }

    // Flush before detaching. FFmpeg can destroy a context at a resolution
    // boundary while display-order surfaces from that context are still
    // pending; publish their CPU snapshots while the CAPTURE mappings exist.
    let ready = guard.contexts[idx]
        .as_mut()
        .and_then(|context| context.v4l2.as_mut())
        .map(V4l2Session::drain_for_context_destroy)
        .unwrap_or_default();
    apply_ready_captures(&mut guard, ready);

    // A surface can outlive its decode context. Detach every owned surface
    // before dropping the V4L2 session, otherwise its cap_idx would point at
    // an mmap region released by V4l2Session::Drop. A ready CPU snapshot is
    // self-contained and remains readable; every other surface becomes dead.
    let owned_surfaces: Vec<usize> = guard
        .surfaces
        .iter()
        .enumerate()
        .filter_map(|(surface_idx, surface)| {
            surface
                .as_ref()
                .is_some_and(|surface| surface.owner == context)
                .then_some(surface_idx)
        })
        .collect();
    for surface_idx in owned_surfaces {
        release_surface_capture(&mut guard, surface_idx);
        if let Some(surface) = guard.surfaces[surface_idx].as_mut() {
            surface.owner = VA_INVALID_ID;
            surface.cap_idx = None;
            if surface.state != SurfaceState::Ready || surface.frame.is_none() {
                surface.state = SurfaceState::Dead;
            }
        }
    }
    for buffer in &mut guard.buffers {
        if buffer
            .as_ref()
            .is_some_and(|buffer| buffer.owner == context)
        {
            *buffer = None;
        }
    }
    guard.contexts[idx] = None;
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{
        Buffer, Config, DRV_ID_BASE_CONFIG, DRV_ID_BASE_SURFACE, DriverBox, Surface, SurfaceFrame,
    };
    use std::ffi::c_void;

    fn context_for_test(config_id: VAConfigID) -> Context {
        Context {
            config_id,
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 320,
            height: 240,
            render_targets: Vec::new(),
            frame_open: false,
            render_target: VA_INVALID_ID,
            decoder: Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
            out_seq: 0,
            v4l2: None,
        }
    }

    #[test]
    fn destroying_context_detaches_owned_surfaces_before_drop() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let context_id = DRV_ID_BASE_CONTEXT;
        let surface_id = DRV_ID_BASE_SURFACE;
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
        state.lock.lock().unwrap().surfaces[0] = Some(Surface {
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Pending,
            cap_idx: Some(4),
            frame: None,
            owner: context_id,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });

        assert_eq!(
            unsafe { destroy_context(&mut ctx, context_id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        let guard = state.lock.lock().unwrap();
        assert!(guard.contexts[0].is_none());
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert_eq!(surface_id, DRV_ID_BASE_SURFACE);
        assert_eq!(surface.owner, VA_INVALID_ID);
        assert_eq!(surface.cap_idx, None);
        assert_eq!(surface.state, SurfaceState::Dead);
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn destroying_context_preserves_ready_cpu_snapshot() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let context_id = DRV_ID_BASE_CONTEXT;
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
        state.lock.lock().unwrap().surfaces[0] = Some(Surface {
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Ready,
            cap_idx: Some(4),
            frame: Some(SurfaceFrame {
                data: vec![1, 2, 3, 4],
                stride: 2,
                height: 2,
                format: crate::pixel_format::DecodedFormat::Nv12,
            }),
            owner: context_id,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });

        assert_eq!(
            unsafe { destroy_context(&mut ctx, context_id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        let guard = state.lock.lock().unwrap();
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert_eq!(surface.owner, VA_INVALID_ID);
        assert_eq!(surface.cap_idx, None);
        assert_eq!(surface.state, SurfaceState::Ready);
        let frame = surface.frame.as_ref().unwrap();
        assert_eq!(frame.data, [1, 2, 3, 4]);
        assert_eq!(frame.stride, 2);
        assert_eq!(frame.height, 2);
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn destroying_context_with_open_picture_is_rejected() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let context_id = DRV_ID_BASE_CONTEXT;
        let mut context = context_for_test(DRV_ID_BASE_CONFIG);
        context.frame_open = true;
        state.lock.lock().unwrap().contexts[0] = Some(context);

        assert_eq!(
            unsafe { destroy_context(&mut ctx, context_id) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert!(state.lock.lock().unwrap().contexts[0].is_some());

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn destroying_context_rejects_mapped_buffers_then_reclaims_owned_buffers() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let context_id = DRV_ID_BASE_CONTEXT;
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
        state.lock.lock().unwrap().buffers[0] = Some(Buffer {
            owner: context_id,
            type_: VABufferType::VASliceDataBufferType,
            elem_size: 4,
            num_elements: 1,
            data: vec![0; 4],
            mapped: true,
        });

        assert_eq!(
            unsafe { destroy_context(&mut ctx, context_id) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert!(state.lock.lock().unwrap().contexts[0].is_some());

        state.lock.lock().unwrap().buffers[0]
            .as_mut()
            .unwrap()
            .mapped = false;
        assert_eq!(
            unsafe { destroy_context(&mut ctx, context_id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        let guard = state.lock.lock().unwrap();
        assert!(guard.contexts[0].is_none());
        assert!(guard.buffers[0].is_none());
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn destroying_config_with_live_context_is_rejected() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().configs[0] = Some(Config {
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            attribs: Vec::new(),
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));

        assert_eq!(
            unsafe { crate::config::destroy_config(&mut ctx, DRV_ID_BASE_CONFIG) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn context_creation_rejects_invalid_dimensions_before_device_setup() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().configs[0] = Some(Config {
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            attribs: Vec::new(),
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        let mut context_id = VA_INVALID_ID;

        assert_eq!(
            unsafe {
                create_context(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    0,
                    240,
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut context_id,
                )
            },
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );
        assert_eq!(context_id, VA_INVALID_ID);
        assert!(state.lock.lock().unwrap().contexts[0].is_none());

        assert_eq!(
            unsafe {
                create_context(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    0,
                    std::ptr::null_mut(),
                    1,
                    &mut context_id,
                )
            },
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );
        assert!(state.lock.lock().unwrap().contexts[0].is_none());

        let invalid_surface = DRV_ID_BASE_SURFACE + 1;
        assert_eq!(
            unsafe {
                create_context(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    0,
                    &invalid_surface as *const VASurfaceID as *mut VASurfaceID,
                    1,
                    &mut context_id,
                )
            },
            VA_STATUS_ERROR_INVALID_SURFACE as VAStatus
        );
        assert!(state.lock.lock().unwrap().contexts[0].is_none());

        state.lock.lock().unwrap().contexts[1] = Some(context_for_test(DRV_ID_BASE_CONFIG));
        assert_eq!(
            unsafe {
                create_context(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut context_id,
                )
            },
            VA_STATUS_ERROR_MAX_NUM_EXCEEDED as VAStatus
        );
        state.lock.lock().unwrap().contexts[1] = None;

        unsafe { drop(Box::from_raw(raw)) };
    }
}
