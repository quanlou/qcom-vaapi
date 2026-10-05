//! VA decode-context lifecycle.
//!
//! A context owns one codec-specific access-unit assembler and one stateful
//! V4L2 session. The
//! callbacks here only validate VA handles and construct or retire that owner;
//! picture submission remains in `decode.rs`.

use crate::bindings::*;
use crate::codec::{Codec, Decoder};
use crate::state::{
    Context, DRV_ID_BASE_CONTEXT, DRV_MAX_SURFACES, SurfaceState, config_index, context_index,
    surface_index,
};
use crate::surface::release_surface_capture;
use crate::sync::apply_ready_captures;
use crate::v4l2::V4l2Session;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::os::fd::{AsRawFd, RawFd};
use std::slice;

// Permit one decoder replacement or second player while retaining a bounded
// device-session budget. The larger handle table is not a concurrency limit.
const MAX_ACTIVE_DECODE_CONTEXTS: usize = 2;

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
    unsafe {
        create_context_with_setup(
            ctx,
            config_id,
            picture_width,
            picture_height,
            render_targets,
            num_render_targets,
            context,
            |width, height, coded_fourcc, capture_fourcc, drm_fd| {
                V4l2Session::open_and_setup(width, height, coded_fourcc, capture_fourcc, drm_fd)
                    .map(Some)
            },
        )
    }
}

// The session opener is injected only by host tests; production always opens
// one separately owned V4L2 session for each accepted decode context.
#[allow(clippy::too_many_arguments)]
unsafe fn create_context_with_setup(
    ctx: VADriverContextP,
    config_id: VAConfigID,
    picture_width: c_int,
    picture_height: c_int,
    render_targets: *mut VASurfaceID,
    num_render_targets: c_int,
    context: *mut VAContextID,
    setup: impl FnOnce(i32, i32, u32, u32, Option<RawFd>) -> Result<Option<V4l2Session>, ()>,
) -> VAStatus {
    if context.is_null()
        || num_render_targets < 0
        || (num_render_targets > 0 && render_targets.is_null())
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if !crate::geometry::valid_dimensions(picture_width as u32, picture_height as u32) {
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
    if guard.contexts.iter().flatten().count() >= MAX_ACTIVE_DECODE_CONTEXTS {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    if num_render_targets as usize > DRV_MAX_SURFACES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let Some(idx) = guard.contexts.iter().position(Option::is_none) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let render_targets = if num_render_targets > 0 {
        let targets = unsafe { slice::from_raw_parts(render_targets, num_render_targets as usize) };
        for &surface_id in targets {
            let Some(surface_idx) = surface_index(surface_id) else {
                return err(VA_STATUS_ERROR_INVALID_SURFACE);
            };
            let Some(surface) = guard.surfaces[surface_idx].as_ref() else {
                return err(VA_STATUS_ERROR_INVALID_SURFACE);
            };
            if surface.owner != VA_INVALID_ID {
                return err(VA_STATUS_ERROR_SURFACE_BUSY);
            }
            if surface.format != cfg.format
                || surface.width < picture_width
                || surface.height < picture_height
            {
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
    let Ok(v4l2) = setup(
        picture_width,
        picture_height,
        codec.fourcc(),
        cfg.format.v4l2_fourcc(),
        guard.drm_fd.as_ref().map(AsRawFd::as_raw_fd),
    ) else {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    };
    {
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
            v4l2,
        });
        unsafe { *context = context_id };
        ok()
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
    // pending; publish completions while the CAPTURE queue still exists.
    let ready = guard.contexts[idx]
        .as_mut()
        .and_then(|context| context.v4l2.as_mut())
        .map(V4l2Session::drain_for_context_destroy)
        .unwrap_or_default();
    apply_ready_captures(&mut guard, context, ready);

    // A surface can outlive its decode context. Detach every owned surface
    // before dropping the V4L2 session, otherwise its cap_idx would point at
    // an mmap region released by V4l2Session::Drop. Ready snapshots and owned
    // display allocations remain readable independently of that session.
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
            let readable = surface.frame.is_some()
                || surface.backing.as_ref().is_some_and(|b| b.can_download());
            if surface.state != SurfaceState::Ready || !readable {
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

    fn driver_for_context_test() -> (*mut DriverBox, VADriverContext) {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw.cast::<c_void>();
        unsafe { &*raw }.lock.lock().unwrap().configs[0] = Some(Config {
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            attribs: Vec::new(),
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        (raw, ctx)
    }

    #[test]
    fn advertised_eight_k_geometry_and_context_creation_agree() {
        let (raw, mut ctx) = driver_for_context_test();
        unsafe { &mut *raw }.profiles = vec![VAProfile::VAProfileAV1Profile0];
        unsafe { &*raw }.lock.lock().unwrap().configs[0]
            .as_mut()
            .unwrap()
            .profile = VAProfile::VAProfileAV1Profile0;
        let mut attrs = [
            VAConfigAttrib {
                type_: VAConfigAttribType::VAConfigAttribMaxPictureWidth,
                value: 0,
            },
            VAConfigAttrib {
                type_: VAConfigAttribType::VAConfigAttribMaxPictureHeight,
                value: 0,
            },
        ];
        assert_eq!(
            unsafe {
                crate::config::get_config_attributes(
                    &mut ctx,
                    VAProfile::VAProfileAV1Profile0,
                    VAEntrypoint::VAEntrypointVLD,
                    attrs.as_mut_ptr(),
                    attrs.len() as i32,
                )
            },
            ok()
        );
        assert!(
            attrs
                .iter()
                .all(|a| a.value == crate::geometry::MAX_DIM as u32)
        );
        for (width, height, accepted) in [
            (3840, 2160, true),
            (7680, 4320, cfg!(feature = "experimental-8k")),
            (8192, 8192, false),
            (-1, 2160, false),
        ] {
            let mut target = VA_INVALID_ID;
            let status = unsafe {
                crate::surface::create_surfaces2(
                    &mut ctx,
                    VA_RT_FORMAT_YUV420,
                    width as u32,
                    height as u32,
                    &mut target,
                    1,
                    std::ptr::null_mut(),
                    0,
                )
            };
            assert_eq!(
                status,
                if accepted {
                    ok()
                } else {
                    err(VA_STATUS_ERROR_INVALID_PARAMETER)
                }
            );
            let mut context = VA_INVALID_ID;
            assert_eq!(
                unsafe {
                    create_context_with_setup(
                        &mut ctx,
                        DRV_ID_BASE_CONFIG,
                        width,
                        height,
                        &mut target,
                        1,
                        &mut context,
                        |w, h, coded, capture, _| {
                            assert!(accepted, "invalid geometry reached session setup");
                            assert_eq!((w, h), (width, height));
                            assert_eq!(coded, crate::v4l2::V4L2_PIX_FMT_AV1);
                            assert_eq!(
                                capture,
                                crate::pixel_format::DecodedFormat::Nv12.v4l2_fourcc()
                            );
                            Ok(None)
                        },
                    )
                },
                if accepted {
                    ok()
                } else {
                    err(VA_STATUS_ERROR_INVALID_PARAMETER)
                }
            );
            if accepted {
                assert_eq!(unsafe { destroy_context(&mut ctx, context) }, ok());
                assert_eq!(
                    unsafe { crate::surface::destroy_surfaces(&mut ctx, &mut target, 1) },
                    ok()
                );
            } else {
                assert_eq!((context, target), (VA_INVALID_ID, VA_INVALID_ID));
            }
        }
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn second_context_uses_an_independent_slot_before_prior_context_teardown() {
        let (raw, mut ctx) = driver_for_context_test();
        let mut first = VA_INVALID_ID;
        let mut second = VA_INVALID_ID;
        let setups = std::cell::Cell::new(0);
        for output in [&mut first, &mut second] {
            assert_eq!(
                unsafe {
                    create_context_with_setup(
                        &mut ctx,
                        DRV_ID_BASE_CONFIG,
                        320,
                        240,
                        std::ptr::null_mut(),
                        0,
                        output,
                        |_, _, _, _, _| {
                            setups.set(setups.get() + 1);
                            Ok(None)
                        },
                    )
                },
                ok()
            );
        }
        assert_eq!(
            (first, second),
            (DRV_ID_BASE_CONTEXT, DRV_ID_BASE_CONTEXT + 1)
        );
        assert_eq!(setups.get(), 2);
        assert_eq!(unsafe { destroy_context(&mut ctx, first) }, ok());
        assert!(unsafe { &*raw }.lock.lock().unwrap().contexts[1].is_some());
        assert_eq!(unsafe { destroy_context(&mut ctx, second) }, ok());
        unsafe { drop(Box::from_raw(raw)) };
    }

    fn surface_for_context_test(owner: VAContextID) -> Surface {
        Surface {
            backing: None,
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Empty,
            cap_idx: None,
            frame: None,
            owner,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        }
    }

    #[test]
    fn context_budget_and_setup_failure_leave_other_owners_intact() {
        let (raw, mut ctx) = driver_for_context_test();
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            for index in 0..2 {
                guard.contexts[index] = Some(context_for_test(DRV_ID_BASE_CONFIG));
            }
            guard.surfaces[0] = Some(surface_for_context_test(VA_INVALID_ID));
        }
        let mut target = DRV_ID_BASE_SURFACE;
        let mut output = VA_INVALID_ID;
        assert_eq!(
            unsafe {
                create_context_with_setup(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    &mut target,
                    1,
                    &mut output,
                    |_, _, _, _, _| panic!("budget must be checked before opening a session"),
                )
            },
            err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED)
        );
        assert_eq!(output, VA_INVALID_ID);
        assert_eq!(
            unsafe { destroy_context(&mut ctx, DRV_ID_BASE_CONTEXT) },
            ok()
        );
        assert_eq!(
            unsafe {
                create_context_with_setup(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    &mut target,
                    1,
                    &mut output,
                    |_, _, _, _, _| Err(()),
                )
            },
            err(VA_STATUS_ERROR_OPERATION_FAILED)
        );
        let guard = state.lock.lock().unwrap();
        assert_eq!(output, VA_INVALID_ID);
        assert!(guard.contexts[0].is_none());
        assert!(guard.contexts[1].is_some());
        assert_eq!(guard.surfaces[0].as_ref().unwrap().owner, VA_INVALID_ID);
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn foreign_render_target_rejected_before_setup_and_teardown_keeps_other_context() {
        let (raw, mut ctx) = driver_for_context_test();
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            guard.contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
            guard.surfaces[0] = Some(surface_for_context_test(DRV_ID_BASE_CONTEXT));
        }
        let mut target = DRV_ID_BASE_SURFACE;
        let mut output = VA_INVALID_ID;
        assert_eq!(
            unsafe {
                create_context_with_setup(
                    &mut ctx,
                    DRV_ID_BASE_CONFIG,
                    320,
                    240,
                    &mut target,
                    1,
                    &mut output,
                    |_, _, _, _, _| panic!("foreign surface must be rejected before setup"),
                )
            },
            err(VA_STATUS_ERROR_SURFACE_BUSY)
        );
        assert_eq!(output, VA_INVALID_ID);
        {
            let mut guard = state.lock.lock().unwrap();
            guard.contexts[1] = Some(context_for_test(DRV_ID_BASE_CONFIG));
            guard.surfaces[1] = Some(surface_for_context_test(DRV_ID_BASE_CONTEXT + 1));
            for index in 0..2 {
                guard.buffers[index] = Some(Buffer {
                    owner: DRV_ID_BASE_CONTEXT + index as u32,
                    type_: VABufferType::VASliceDataBufferType,
                    elem_size: 1,
                    num_elements: 1,
                    data: vec![index as u8],
                    mapped: false,
                });
            }
        }
        assert_eq!(
            unsafe { destroy_context(&mut ctx, DRV_ID_BASE_CONTEXT) },
            ok()
        );
        let guard = state.lock.lock().unwrap();
        assert!(guard.contexts[0].is_none());
        assert!(guard.contexts[1].is_some());
        assert!(guard.buffers[0].is_none());
        assert!(guard.buffers[1].is_some());
        assert_eq!(guard.surfaces[0].as_ref().unwrap().owner, VA_INVALID_ID);
        assert_eq!(
            guard.surfaces[1].as_ref().unwrap().owner,
            DRV_ID_BASE_CONTEXT + 1
        );
        assert_eq!(
            guard.surfaces[1].as_ref().unwrap().state,
            SurfaceState::Empty
        );
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn identical_chrome_contexts_export_before_begin_and_keep_backing_through_teardown() {
        use crate::surface_export::export_ready_surface_for_test;
        use crate::sync::{apply_ready_captures, sync_surface2};
        use crate::va_drm::DrmPrimeLayout;
        use std::os::fd::FromRawFd;
        use std::os::unix::fs::FileExt;
        let (raw, mut ctx) = driver_for_context_test();
        let state = unsafe { &*raw };
        let mut clients = Vec::new();
        for index in 0..2 {
            let mut context = VA_INVALID_ID;
            // Exact Chromium context contract: no render-target array.
            assert_eq!(
                unsafe {
                    create_context_with_setup(
                        &mut ctx,
                        DRV_ID_BASE_CONFIG,
                        3840,
                        2160,
                        std::ptr::null_mut(),
                        0,
                        &mut context,
                        |_, _, _, _, _| Ok(None),
                    )
                },
                ok()
            );
            assert_eq!(context, DRV_ID_BASE_CONTEXT + index as u32);
            let mut surface = surface_for_context_test(VA_INVALID_ID);
            surface.width = 3840;
            surface.height = 2160;
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[index] = Some(surface);
            let desc = export_ready_surface_for_test(
                &mut guard,
                DRV_ID_BASE_SURFACE + index as u32,
                DrmPrimeLayout::Composed,
            )
            .unwrap();
            assert_eq!(guard.surfaces[index].as_ref().unwrap().owner, VA_INVALID_ID);
            clients.push(unsafe { std::fs::File::from_raw_fd(desc.objects[0].fd) });
        }
        for (index, client) in clients.iter().enumerate() {
            let context = DRV_ID_BASE_CONTEXT + index as u32;
            let surface = DRV_ID_BASE_SURFACE + index as u32;
            assert_eq!(
                unsafe { crate::decode::begin_picture(&mut ctx, context, surface) },
                ok()
            );
            {
                let mut guard = state.lock.lock().unwrap();
                // Model an actual decoded completion without a device open.
                guard.contexts[index].as_mut().unwrap().frame_open = false;
                guard.surfaces[index].as_mut().unwrap().state = SurfaceState::Pending;
                let stride = 3840;
                let height = 2176;
                let value = index as u8 + 3;
                apply_ready_captures(
                    &mut guard,
                    context,
                    vec![crate::v4l2::ReadyCapture {
                        surface,
                        failed: false,
                        direct: false,
                        cap_idx: Some(9),
                        frame: Some(SurfaceFrame {
                            data: std::sync::Arc::new(vec![value; stride * height * 3 / 2]),
                            stride: stride as u32,
                            height: height as u32,
                            format: crate::pixel_format::DecodedFormat::Nv12,
                        }),
                    }],
                );
            }
            assert_eq!(unsafe { sync_surface2(&mut ctx, surface, 0) }, ok());
            let mut byte = [0];
            client.read_exact_at(&mut byte, 0).unwrap();
            assert_eq!(byte[0], index as u8 + 3);
            assert_eq!(unsafe { destroy_context(&mut ctx, context) }, ok());
            {
                let guard = state.lock.lock().unwrap();
                let surface = guard.surfaces[index].as_ref().unwrap();
                assert_eq!(surface.owner, VA_INVALID_ID);
                assert!(surface.backing.is_some());
                assert!(surface.exported);
                assert_eq!(surface.state, SurfaceState::Ready);
            }
            let mut surface_id = surface;
            assert_eq!(
                unsafe { crate::surface::destroy_surfaces(&mut ctx, &mut surface_id, 1) },
                ok()
            );
            client.read_exact_at(&mut byte, 0).unwrap();
            assert_eq!(byte[0], index as u8 + 3);
        }
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    #[ignore = "graphics buffer allocation only; no Iris decoder opens"]
    fn actual_render_buffers_survive_two_contexts_without_decoder_opens() {
        use crate::surface_export::export_ready_surface;
        use crate::sync::{apply_ready_captures, sync_surface2};
        use crate::va_drm::DrmPrimeLayout;
        use std::os::fd::AsRawFd;
        use std::os::fd::FromRawFd;
        fn read_export_byte(client: &std::fs::File) -> u8 {
            unsafe extern "C" {
                fn mmap(
                    addr: *mut c_void,
                    len: usize,
                    prot: c_int,
                    flags: c_int,
                    fd: c_int,
                    offset: isize,
                ) -> *mut c_void;
                fn munmap(addr: *mut c_void, len: usize) -> c_int;
            }
            let addr = unsafe { mmap(std::ptr::null_mut(), 4096, 1, 1, client.as_raw_fd(), 0) };
            assert_ne!(addr as isize, -1);
            let value = unsafe { addr.cast::<u8>().read_volatile() };
            assert_eq!(unsafe { munmap(addr, 4096) }, 0);
            value
        }
        let (raw, mut ctx) = driver_for_context_test();
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().drm_fd = Some(
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/dri/renderD128")
                .unwrap()
                .into(),
        );
        let mut clients = Vec::new();
        for index in 0..2 {
            let mut context = VA_INVALID_ID;
            // Exact Chromium context contract: no render-target array.
            assert_eq!(
                unsafe {
                    create_context_with_setup(
                        &mut ctx,
                        DRV_ID_BASE_CONFIG,
                        3840,
                        2160,
                        std::ptr::null_mut(),
                        0,
                        &mut context,
                        |_, _, _, _, _| Ok(None),
                    )
                },
                ok()
            );
            assert_eq!(context, DRV_ID_BASE_CONTEXT + index as u32);
            let mut surface = surface_for_context_test(VA_INVALID_ID);
            surface.width = 3840;
            surface.height = 2160;
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[index] = Some(surface);
            let desc = export_ready_surface(
                &mut guard,
                DRV_ID_BASE_SURFACE + index as u32,
                DrmPrimeLayout::Composed,
            )
            .unwrap();
            assert_eq!(guard.surfaces[index].as_ref().unwrap().owner, VA_INVALID_ID);
            clients.push(unsafe { std::fs::File::from_raw_fd(desc.objects[0].fd) });
        }
        for (index, client) in clients.iter().enumerate() {
            let context = DRV_ID_BASE_CONTEXT + index as u32;
            let surface = DRV_ID_BASE_SURFACE + index as u32;
            assert_eq!(
                unsafe { crate::decode::begin_picture(&mut ctx, context, surface) },
                ok()
            );
            {
                let mut guard = state.lock.lock().unwrap();
                // Model an actual decoded completion without a device open.
                guard.contexts[index].as_mut().unwrap().frame_open = false;
                guard.surfaces[index].as_mut().unwrap().state = SurfaceState::Pending;
                let stride = 3840;
                let height = 2176;
                let value = index as u8 + 3;
                apply_ready_captures(
                    &mut guard,
                    context,
                    vec![crate::v4l2::ReadyCapture {
                        surface,
                        failed: false,
                        direct: false,
                        cap_idx: Some(9),
                        frame: Some(SurfaceFrame {
                            data: std::sync::Arc::new(vec![value; stride * height * 3 / 2]),
                            stride: stride as u32,
                            height: height as u32,
                            format: crate::pixel_format::DecodedFormat::Nv12,
                        }),
                    }],
                );
            }
            assert_eq!(unsafe { sync_surface2(&mut ctx, surface, 0) }, ok());
            assert_eq!(read_export_byte(client), index as u8 + 3);
            assert_eq!(unsafe { destroy_context(&mut ctx, context) }, ok());
            {
                let guard = state.lock.lock().unwrap();
                let surface = guard.surfaces[index].as_ref().unwrap();
                assert_eq!(surface.owner, VA_INVALID_ID);
                assert!(surface.backing.is_some());
                assert!(surface.exported);
                assert_eq!(surface.state, SurfaceState::Ready);
            }
            let mut surface_id = surface;
            assert_eq!(
                unsafe { crate::surface::destroy_surfaces(&mut ctx, &mut surface_id, 1) },
                ok()
            );
            assert_eq!(read_export_byte(client), index as u8 + 3);
        }
        unsafe { drop(Box::from_raw(raw)) };
    }

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
            backing: None,
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
            backing: None,
            width: 320,
            height: 240,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Ready,
            cap_idx: Some(4),
            frame: Some(SurfaceFrame {
                data: std::sync::Arc::new(vec![1, 2, 3, 4]),
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
        assert_eq!(frame.data.as_slice(), [1, 2, 3, 4]);
        assert_eq!(frame.stride, 2);
        assert_eq!(frame.height, 2);
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn completed_display_allocation_remains_readable_after_context_destroy() {
        use crate::pixel_format::DecodedFormat;
        use crate::surface_backing::SurfaceBacking;
        let (raw, mut ctx) = driver_for_context_test();
        let state = unsafe { &*raw };
        let mut backing = SurfaceBacking::allocate_for_test(320, 240, DecodedFormat::Nv12).unwrap();
        let mut data = vec![19; 320 * 240 * 3 / 2];
        data[320 * 240..].fill(103);
        backing
            .copy_frame(&SurfaceFrame {
                data: std::sync::Arc::new(data),
                stride: 320,
                height: 240,
                format: DecodedFormat::Nv12,
            })
            .unwrap();
        {
            let mut guard = state.lock.lock().unwrap();
            guard.contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
            let mut surface = surface_for_context_test(DRV_ID_BASE_CONTEXT);
            surface.backing = Some(backing);
            surface.state = SurfaceState::Ready;
            surface.cap_idx = Some(0);
            // Direct decode has no CPU snapshot at publication time.
            guard.surfaces[0] = Some(surface);
        }
        assert_eq!(
            unsafe { destroy_context(&mut ctx, DRV_ID_BASE_CONTEXT) },
            ok()
        );
        let mut image: VAImage = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { crate::image::derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut image) },
            ok()
        );
        let guard = state.lock.lock().unwrap();
        let surface = guard.surfaces[0].as_ref().unwrap();
        assert_eq!(surface.owner, VA_INVALID_ID);
        assert_eq!(surface.cap_idx, None);
        assert_eq!(surface.state, SurfaceState::Ready);
        let bytes = &guard.buffers[crate::state::buffer_index(image.buf).unwrap()]
            .as_ref()
            .unwrap()
            .data;
        for row in 0..240 {
            let offset = row * image.pitches[0] as usize;
            assert!(bytes[offset..offset + 320].iter().all(|b| *b == 19));
        }
        for row in 0..120 {
            let offset = image.offsets[1] as usize + row * image.pitches[1] as usize;
            assert!(bytes[offset..offset + 320].iter().all(|b| *b == 103));
        }
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

        state.lock.lock().unwrap().contexts[0] = Some(context_for_test(DRV_ID_BASE_CONFIG));
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
