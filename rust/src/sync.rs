//! Surface readiness publication.
//!
//! V4L2 decode completion is asynchronous: a CAPTURE buffer can finish while
//! the app is submitting more frames, polling surface status, or syncing a
//! surface. These helpers are the single place that turns finished CAPTURE
//! buffers into VA surface state so clients such as GStreamer do not starve the
//! CAPTURE queue by waiting for an explicit `vaSyncSurface`.

use crate::bindings::*;
use crate::state::{DRV_ID_BASE_CONTEXT, DriverState, SurfaceState, context_index, surface_index};
use crate::v4l2::ReadyCapture;
use crate::{err, ok, state_from_ctx, va_debug_enabled};

/// Apply CAPTURE buffers that finished decoding to VA surface state.
///
/// The V4L2 backend accumulates finished captures in `V4l2Session.ready` while
/// it is pumping. Clients that pipeline frames without syncing depend on this
/// state being published as soon as it exists. Idempotent: surfaces already
/// Ready simply get the latest capture slot.
pub(crate) fn apply_ready_captures(
    guard: &mut DriverState,
    owner: VAContextID,
    ready: Vec<ReadyCapture>,
) {
    let DriverState {
        surfaces, contexts, ..
    } = guard;
    let mut session = context_index(owner)
        .and_then(|idx| contexts.get_mut(idx).and_then(Option::as_mut))
        .and_then(|context| context.v4l2.as_mut());
    for r in ready {
        // The working index can already hold the next chosen target. A direct
        // completion owns stable surface storage, independent of that index.
        let zero_copy = r.direct && r.frame.is_none();
        let direct_copy = r.frame.is_none()
            && r.cap_idx.is_some_and(|idx| {
                session
                    .as_ref()
                    .is_some_and(|v| v.capture_is_publishing(idx))
            });
        if let Some(idx) = surface_index(r.surface)
            && let Some(s) = surfaces[idx].as_mut()
            && s.owner == owner
        {
            if r.failed {
                s.frame = None;
                s.state = SurfaceState::Dead;
            } else {
                if std::env::var_os("V4L2_VA_DEBUG").is_some() {
                    eprintln!(
                        "msm_drv_video_rs: publish surface={} cap_idx={:?} previous_state={:?} previous_cap={:?} export_fds={}",
                        r.surface,
                        r.cap_idx,
                        s.state,
                        s.cap_idx,
                        s.export_fds.len()
                    );
                }
                if let Some(cap_idx) = r.cap_idx {
                    s.cap_idx = Some(cap_idx);
                    let copy_failed = match s.backing.as_mut() {
                        Some(_) if zero_copy => false,
                        Some(backing) if direct_copy => session
                            .as_mut()
                            .is_none_or(|v| v.publish_capture_into(cap_idx, backing).is_err()),
                        Some(backing) => r
                            .frame
                            .as_ref()
                            .is_none_or(|frame| backing.copy_frame(frame).is_err()),
                        None => direct_copy || zero_copy,
                    };
                    if copy_failed {
                        s.frame = None;
                        s.state = SurfaceState::Dead;
                    } else {
                        s.frame = r.frame;
                        s.state = SurfaceState::Ready;
                    }
                } else {
                    s.state = SurfaceState::Ready;
                }
                // A PRIME export is a handle to the CAPTURE allocation, not a
                // one-frame lease. Keep the bookkeeping live while the same VA
                // surface is reused so importers can retain the fd across frames.
                s.exported = !s.export_fds.is_empty();
            }
        }
        if direct_copy
            && let Some(cap_idx) = r.cap_idx
            && let Some(v) = session.as_mut()
        {
            v.finish_capture_publication(cap_idx);
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
    apply_ready_captures(guard, DRV_ID_BASE_CONTEXT + ctx_idx as u32, ready);
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
    let deadline = if timeout_ns == VA_TIMEOUT_INFINITE as u64 {
        None
    } else {
        start.checked_add(std::time::Duration::from_nanos(timeout_ns))
    };
    let mut compatibility_drain_started = false;
    loop {
        let mut guard = match state.lock.lock() {
            Ok(g) => g,
            Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
        };
        let Some(surf) = guard.surfaces[surf_idx].as_ref() else {
            return err(VA_STATUS_ERROR_INVALID_SURFACE);
        };
        match surf.state {
            // Empty means no vaBeginPicture ran yet, so there is nothing to
            // wait for. Chromium's VaapiVideoDecodeLinuxGL export flow syncs
            // pool surfaces before their first decode as a validity check;
            // Mesa/Intel drivers succeed there, so we do too.
            SurfaceState::Ready | SurfaceState::Empty => return ok(),
            SurfaceState::Dead => return err(VA_STATUS_ERROR_DECODING_ERROR),
            SurfaceState::InProgress | SurfaceState::Pending => {}
        }
        let owner = surf.owner;
        let Some(ctx_idx) = context_index(owner) else {
            return err(VA_STATUS_ERROR_INVALID_CONTEXT);
        };
        let Some(c) = guard.contexts[ctx_idx].as_mut() else {
            return err(VA_STATUS_ERROR_INVALID_CONTEXT);
        };
        // FFmpeg can sync an early display-order surface on one thread while
        // its decode thread is still submitting the following access units.
        // Only pump while holding the state lock; issuing a midstream STOP
        // breaks reference continuity, and retaining/reacquiring the lock in
        // a tight loop can starve the submitter that will make this surface
        // ready.
        let ready = if let Some(v4l2) = c.v4l2.as_mut() {
            let poll_ms = deadline.map_or(2, |deadline| {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                remaining.as_millis().min(2) as i32
            });
            let mut ready = v4l2.pump(poll_ms);
            if ready.is_empty()
                && timeout_ns != 0
                && !compatibility_drain_started
                // STOP resets the firmware reference chain. Allow the same
                // natural decode grace as keyframe submission and never STOP
                // queued input or mutate a session after this call's deadline.
                && start.elapsed() >= std::time::Duration::from_millis(100)
                && deadline.is_none_or(|end| std::time::Instant::now() < end)
                && v4l2.sync_drain_input_idle()
            {
                compatibility_drain_started = v4l2.maybe_start_sync_drain();
                if compatibility_drain_started {
                    ready.extend(v4l2.pump(0));
                }
            }
            ready
        } else {
            Vec::new()
        };
        let v4l2_eos = c
            .v4l2
            .as_ref()
            .is_some_and(|v| v.eos() && v.pending_count() == 0);
        let v4l2_failed = c.v4l2.as_ref().is_some_and(|v| v.failed());
        apply_ready_captures(&mut guard, owner, ready);
        if guard.surfaces[surf_idx]
            .as_ref()
            .is_some_and(|s| s.state == SurfaceState::Ready)
        {
            return ok();
        }
        if guard.surfaces[surf_idx]
            .as_ref()
            .is_some_and(|surface| surface.state == SurfaceState::Dead)
        {
            // PRIME copy/cache synchronization can fail even when the decode
            // session itself remains healthy. Report that publication failure
            // now rather than waiting for the generic surface timeout.
            return err(VA_STATUS_ERROR_DECODING_ERROR);
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
        let timed_out = deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline);
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
        // Give a concurrent BeginPicture/RenderPicture/EndPicture sequence a
        // chance to acquire the state lock before the next polling iteration.
        std::thread::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_ID_BASE_SURFACE, DRV_MAX_SURFACES, DriverBox, Surface};
    use std::ffi::c_void;
    use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};

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

    fn pending_sync_driver(queued_input: bool) -> Box<DriverBox> {
        use std::os::fd::IntoRawFd;
        let driver = Box::new(DriverBox::new());
        let mut guard = driver.lock.lock().unwrap();
        let mut surface = surface_with(SurfaceState::Pending, None);
        surface.owner = DRV_ID_BASE_CONTEXT;
        guard.surfaces[0] = Some(surface);
        let fd = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")
            .unwrap()
            .into_raw_fd();
        guard.contexts[0] = Some(crate::state::Context {
            config_id: VA_INVALID_ID,
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 64,
            height: 64,
            render_targets: vec![DRV_ID_BASE_SURFACE],
            frame_open: false,
            render_target: VA_INVALID_ID,
            decoder: crate::codec::Decoder::new(VAProfile::VAProfileH264Main).unwrap(),
            out_seq: 0,
            v4l2: Some(crate::v4l2::V4l2Session::pending_sync_test_session(
                fd,
                queued_input,
            )),
        });
        drop(guard);
        driver
    }

    #[test]
    fn direct_publication_pins_capture_and_late_images_read_surface_owned_pixels() {
        use crate::pixel_format::DecodedFormat;
        use crate::surface_backing::SurfaceBacking;
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let driver = pending_sync_driver(false);
            let stride = 128 * format.bytes_per_sample();
            let storage_height = 80;
            let pixels: Vec<u8> = (0..stride * storage_height * 3 / 2)
                .map(|i| (i as u8).wrapping_add((i / stride) as u8))
                .collect();
            let mut guard = driver.lock.lock().unwrap();
            guard.contexts[0].as_mut().unwrap().v4l2 =
                Some(crate::v4l2::V4l2Session::publishing_test_session(
                    &pixels,
                    stride,
                    storage_height,
                    format,
                ));
            let surface = guard.surfaces[0].as_mut().unwrap();
            surface.width = 128;
            surface.format = format;
            surface.backing = Some(SurfaceBacking::allocate_for_test(128, 64, format).unwrap());
            let session = guard.contexts[0].as_mut().unwrap().v4l2.as_mut().unwrap();
            assert!(session.capture_is_publishing(0));
            session.requeue_capture(0);
            assert!(session.capture_is_publishing(0));
            apply_ready_captures(
                &mut guard,
                DRV_ID_BASE_CONTEXT,
                vec![ReadyCapture {
                    surface: DRV_ID_BASE_SURFACE,
                    failed: false,
                    direct: false,
                    cap_idx: Some(0),
                    frame: None,
                }],
            );
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.state, SurfaceState::Ready);
            assert!(
                surface.frame.is_none(),
                "display publication needs no CPU snapshot"
            );
            let snapshot = surface.backing.as_ref().unwrap().download().unwrap();
            assert_eq!((snapshot.stride, snapshot.height), (stride, 64));
            let y_size = stride as usize * 64;
            let src_uv = stride as usize * storage_height as usize;
            let uv_size = stride as usize * 32;
            assert_eq!(&snapshot.data[..y_size], &pixels[..y_size]);
            assert_eq!(
                &snapshot.data[y_size..y_size + uv_size],
                &pixels[src_uv..src_uv + uv_size]
            );
            let session = guard.contexts[0].as_mut().unwrap().v4l2.as_mut().unwrap();
            assert!(!session.capture_is_publishing(0));
            assert!(
                session
                    .publish_capture_into(
                        0,
                        &mut SurfaceBacking::allocate_for_test(128, 64, format).unwrap()
                    )
                    .is_err()
            );
            guard.contexts[0].as_mut().unwrap().v4l2 =
                Some(crate::v4l2::V4l2Session::publishing_test_session(
                    &vec![0xee; pixels.len()],
                    stride,
                    storage_height,
                    format,
                ));
            drop(guard);
            let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
            ctx.pDriverData = (&*driver as *const DriverBox).cast_mut().cast();
            let mut image: VAImage = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { crate::image::derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut image) },
                ok()
            );
            let guard = driver.lock.lock().unwrap();
            let buf_idx = crate::state::buffer_index(image.buf).unwrap();
            let data = &guard.buffers[buf_idx].as_ref().unwrap().data;
            assert_eq!(&data[..y_size], &pixels[..y_size]);
            assert_eq!(
                &data[y_size..y_size + uv_size],
                &pixels[src_uv..src_uv + uv_size]
            );
        }
    }

    #[test]
    fn sync_short_timeout_preserves_pending_decode_without_stop() {
        let driver = pending_sync_driver(false);
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &*driver as *const DriverBox as *mut c_void;
        // The old20ms STOP loop rejected ENOTTY eight times and abandoned
        // this healthy pending owner before its caller's60ms deadline.
        assert_eq!(
            unsafe { sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 60_000_000) },
            err(VA_STATUS_ERROR_TIMEDOUT)
        );
        let guard = driver.lock.lock().unwrap();
        assert_eq!(
            guard.surfaces[0].as_ref().unwrap().state,
            SurfaceState::Pending
        );
        assert!(
            !guard.contexts[0]
                .as_ref()
                .unwrap()
                .v4l2
                .as_ref()
                .unwrap()
                .failed()
        );
    }

    #[test]
    fn imported_publication_omits_snapshot_and_late_images_survive_capture_reuse() {
        use crate::pixel_format::DecodedFormat;
        use crate::surface_backing::SurfaceBacking;
        use crate::surface_import::ImportLayout;
        use std::fs::File;
        use std::os::fd::FromRawFd;
        use std::os::unix::fs::FileExt;
        unsafe extern "C" {
            fn memfd_create(name: *const std::ffi::c_char, flags: u32) -> std::ffi::c_int;
        }
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let driver = pending_sync_driver(false);
            let pixels: Vec<u8> = (0..128 * 32 * 3 / 2)
                .map(|i| (i as u8).wrapping_add((i / 128) as u8))
                .collect();
            let fd = unsafe { memfd_create(c"imported-publication-test".as_ptr(), 1) };
            assert!(fd >= 0);
            let caller = unsafe { File::from_raw_fd(fd) };
            caller.set_len(4096).unwrap();
            caller.write_all_at(&vec![0xa5; 4096], 0).unwrap();
            let layout = ImportLayout {
                width: 17,
                height: 17,
                format,
                size: 4096,
                pitches: [128, 64],
                offsets: [32, 2304],
            };
            let backing =
                SurfaceBacking::import_for_test(caller.try_clone().unwrap().into(), layout)
                    .unwrap();
            assert!(
                backing.can_download(),
                "imported display storage can retain pixels without a frame snapshot"
            );
            let mut guard = driver.lock.lock().unwrap();
            guard.contexts[0].as_mut().unwrap().v4l2 = Some(
                crate::v4l2::V4l2Session::publishing_test_session(&pixels, 128, 32, format),
            );
            let surface = guard.surfaces[0].as_mut().unwrap();
            surface.width = 17;
            surface.height = 17;
            surface.format = format;
            surface.backing = Some(backing);
            apply_ready_captures(
                &mut guard,
                DRV_ID_BASE_CONTEXT,
                vec![ReadyCapture {
                    surface: DRV_ID_BASE_SURFACE,
                    failed: false,
                    direct: false,
                    cap_idx: Some(0),
                    frame: None,
                }],
            );
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.state, SurfaceState::Ready);
            assert!(
                surface.frame.is_none(),
                "display publication must not allocate a snapshot"
            );
            let mut expected = vec![0xa5; 4096];
            for (plane, rows) in [17, 9].into_iter().enumerate() {
                let width = if plane == 0 { 17 } else { 18 } * format.bytes_per_sample() as usize;
                for row in 0..rows {
                    let src = plane * 4096 + row * 128;
                    let dst = layout.offsets[plane] as usize + row * layout.pitches[plane] as usize;
                    expected[dst..dst + width].copy_from_slice(&pixels[src..src + width]);
                }
            }
            let mut actual = vec![0; 4096];
            caller.read_exact_at(&mut actual, 0).unwrap();
            assert_eq!(
                actual, expected,
                "publication must preserve prefixes, gaps, padding and tail bytes"
            );
            assert!(
                !guard.contexts[0]
                    .as_ref()
                    .unwrap()
                    .v4l2
                    .as_ref()
                    .unwrap()
                    .capture_is_publishing(0)
            );
            guard.contexts[0].as_mut().unwrap().v4l2 =
                Some(crate::v4l2::V4l2Session::publishing_test_session(
                    &vec![0xee; pixels.len()],
                    128,
                    32,
                    format,
                ));
            drop(guard);
            let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
            ctx.pDriverData = (&*driver as *const DriverBox).cast_mut().cast();
            let mut image: VAImage = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { crate::image::derive_image(&mut ctx, DRV_ID_BASE_SURFACE, &mut image) },
                ok()
            );
            let guard = driver.lock.lock().unwrap();
            let data = &guard.buffers[crate::state::buffer_index(image.buf).unwrap()]
                .as_ref()
                .unwrap()
                .data;
            for (plane, rows) in [17, 9].into_iter().enumerate() {
                let width = if plane == 0 { 17 } else { 18 } * format.bytes_per_sample() as usize;
                for row in 0..rows {
                    let src = plane * 4096 + row * 128;
                    let dst = image.offsets[plane] as usize + row * image.pitches[plane] as usize;
                    assert_eq!(
                        &data[dst..dst + width],
                        &pixels[src..src + width],
                        "late images must read caller storage, not the recycled capture buffer"
                    );
                }
            }
        }
    }

    #[test]
    fn sync_does_not_stop_input_still_owned_by_firmware() {
        let driver = pending_sync_driver(true);
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = &*driver as *const DriverBox as *mut c_void;
        // Cross the natural grace with a real writable fd and queued input.
        // POLLOUT alone cannot authorize flushing a still-decoding picture.
        assert_eq!(
            unsafe { sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 130_000_000) },
            err(VA_STATUS_ERROR_TIMEDOUT)
        );
        let guard = driver.lock.lock().unwrap();
        assert!(
            !guard.contexts[0]
                .as_ref()
                .unwrap()
                .v4l2
                .as_ref()
                .unwrap()
                .failed()
        );
        assert_eq!(
            guard.surfaces[0].as_ref().unwrap().state,
            SurfaceState::Pending
        );
    }

    #[test]
    fn foreign_context_completions_do_not_change_surface_backing_or_failure_state() {
        let mut guard = state_with_empty_surfaces();
        let mut surface = surface_with(SurfaceState::Pending, Some(9));
        surface.owner = DRV_ID_BASE_CONTEXT + 1;
        surface.frame = Some(crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![7; 4]),
            stride: 2,
            height: 2,
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        guard.surfaces[0] = Some(surface);
        for failed in [false, true] {
            apply_ready_captures(
                &mut guard,
                DRV_ID_BASE_CONTEXT,
                vec![ReadyCapture {
                    surface: DRV_ID_BASE_SURFACE,
                    failed,
                    direct: false,
                    cap_idx: Some(3),
                    frame: None,
                }],
            );
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.owner, DRV_ID_BASE_CONTEXT + 1);
            assert_eq!(surface.state, SurfaceState::Pending);
            assert_eq!(surface.cap_idx, Some(9));
            assert_eq!(surface.frame.as_ref().unwrap().data.as_slice(), &[7; 4]);
        }
        apply_ready_captures(
            &mut guard,
            DRV_ID_BASE_CONTEXT + 1,
            vec![ReadyCapture {
                surface: DRV_ID_BASE_SURFACE,
                failed: false,
                direct: false,
                cap_idx: Some(9),
                frame: None,
            }],
        );
        assert_eq!(
            guard.surfaces[0].as_ref().unwrap().state,
            SurfaceState::Ready
        );
    }

    #[test]
    fn prime_copy_sync_failure_marks_surface_dead_and_sync_returns_prompt_error() {
        for mode in 0..3 {
            let mut guard = state_with_empty_surfaces();
            let mut surface = surface_with(SurfaceState::Pending, None);
            surface.owner = DRV_ID_BASE_CONTEXT;
            let mut backing = crate::surface_backing::SurfaceBacking::allocate_for_test(
                64,
                64,
                crate::pixel_format::DecodedFormat::Nv12,
            )
            .unwrap();
            match mode {
                0 => backing.set_sync_for_test(|_, _| Err(std::io::Error::other("START failed"))),
                1 => backing.set_sync_for_test(|_, flags| {
                    if flags & 4 != 0 {
                        Err(std::io::Error::other("END failed"))
                    } else {
                        Ok(())
                    }
                }),
                _ => backing.set_wait_for_test(|_| Err(std::io::ErrorKind::TimedOut.into())),
            }
            surface.backing = Some(backing);
            surface.exported = true;
            guard.surfaces[0] = Some(surface);
            apply_ready_captures(
                &mut guard,
                DRV_ID_BASE_CONTEXT,
                vec![ReadyCapture {
                    surface: DRV_ID_BASE_SURFACE,
                    failed: false,
                    direct: false,
                    cap_idx: Some(9),
                    frame: Some(crate::state::SurfaceFrame {
                        data: std::sync::Arc::new(vec![2; 64 * 64 * 3 / 2]),
                        stride: 64,
                        height: 64,
                        format: crate::pixel_format::DecodedFormat::Nv12,
                    }),
                }],
            );
            let surface = guard.surfaces[0].as_ref().unwrap();
            assert_eq!(surface.state, SurfaceState::Dead);
            assert!(surface.frame.is_none());
            assert_eq!(surface.owner, DRV_ID_BASE_CONTEXT);
            assert_eq!(surface.cap_idx, Some(9));
            assert!(
                surface
                    .backing
                    .as_ref()
                    .unwrap()
                    .descriptor(crate::va_drm::DrmPrimeLayout::Composed)
                    .is_err()
            );
            let driver = DriverBox::new();
            *driver.lock.lock().unwrap() = guard;
            let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
            ctx.pDriverData = &driver as *const DriverBox as *mut c_void;
            let start = std::time::Instant::now();
            assert_eq!(
                unsafe { sync_surface2(&mut ctx, DRV_ID_BASE_SURFACE, 10_000_000_000) },
                err(VA_STATUS_ERROR_DECODING_ERROR)
            );
            assert!(start.elapsed() < std::time::Duration::from_secs(1));
        }
    }

    #[test]
    fn publish_marks_pending_surface_ready_with_capture() {
        let mut guard = state_with_empty_surfaces();
        let surf_id = DRV_ID_BASE_SURFACE + 7;
        guard.surfaces[7] = Some(surface_with(SurfaceState::Pending, None));

        apply_ready_captures(
            &mut guard,
            VA_INVALID_ID,
            vec![ReadyCapture {
                failed: false,
                direct: false,
                surface: surf_id,
                cap_idx: Some(3),
                frame: None,
            }],
        );

        let s = guard.surfaces[7].as_ref().unwrap();
        assert_eq!(s.state, SurfaceState::Ready);
        assert_eq!(s.cap_idx, Some(3));
        assert!(!s.exported);
    }

    #[test]
    fn discarded_completion_marks_surface_dead_and_preserves_reservation() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[7] = Some(surface_with(SurfaceState::Pending, Some(3)));
        guard.surfaces[7].as_mut().unwrap().frame = Some(crate::state::SurfaceFrame {
            data: std::sync::Arc::new(vec![1; 384]),
            stride: 16,
            height: 16,
            format: crate::pixel_format::DecodedFormat::Nv12,
        });
        apply_ready_captures(
            &mut guard,
            VA_INVALID_ID,
            vec![ReadyCapture {
                surface: DRV_ID_BASE_SURFACE + 7,
                failed: true,
                direct: false,
                cap_idx: None,
                frame: None,
            }],
        );
        let surface = guard.surfaces[7].as_ref().unwrap();
        assert_eq!(surface.state, SurfaceState::Dead);
        assert!(surface.frame.is_none());
        assert_eq!(surface.cap_idx, Some(3));
    }

    #[test]
    fn publish_ignores_unknown_and_destroyed_surfaces() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[2] = Some(surface_with(SurfaceState::Ready, Some(1)));

        apply_ready_captures(
            &mut guard,
            VA_INVALID_ID,
            vec![
                ReadyCapture {
                    failed: false,
                    direct: false,
                    surface: DRV_ID_BASE_SURFACE + 9_999,
                    cap_idx: Some(0),
                    frame: None,
                },
                ReadyCapture {
                    failed: false,
                    direct: false,
                    surface: VA_INVALID_ID,
                    cap_idx: Some(1),
                    frame: None,
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
            VA_INVALID_ID,
            vec![ReadyCapture {
                failed: false,
                direct: false,
                surface: DRV_ID_BASE_SURFACE,
                cap_idx: Some(11),
                frame: None,
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
            backing: None,
            width: 64,
            height: 64,
            format: crate::pixel_format::DecodedFormat::Nv12,
            state: SurfaceState::Pending,
            cap_idx: Some(4),
            frame: None,
            owner: VA_INVALID_ID,
            exported: true,
            export_count: 1,
            export_fds: vec![unsafe { OwnedFd::from_raw_fd(file.into_raw_fd()) }],
        });

        apply_ready_captures(
            &mut guard,
            VA_INVALID_ID,
            vec![ReadyCapture {
                failed: false,
                direct: false,
                surface: DRV_ID_BASE_SURFACE,
                cap_idx: Some(4),
                frame: None,
            }],
        );

        let surface = guard.surfaces[0].as_ref().unwrap();
        assert!(surface.exported);
        assert_eq!(surface.export_fds.len(), 1);
    }

    #[test]
    fn publish_marks_no_output_surface_ready_without_capture_slot() {
        let mut guard = state_with_empty_surfaces();
        guard.surfaces[5] = Some(surface_with(SurfaceState::Pending, None));

        apply_ready_captures(
            &mut guard,
            VA_INVALID_ID,
            vec![ReadyCapture {
                failed: false,
                direct: false,
                surface: DRV_ID_BASE_SURFACE + 5,
                cap_idx: None,
                frame: None,
            }],
        );

        let surface = guard.surfaces[5].as_ref().unwrap();
        assert_eq!(surface.state, SurfaceState::Ready);
        assert_eq!(surface.cap_idx, None);
    }

    #[test]
    fn sync_returns_success_on_empty_and_ready_error_on_dead() {
        // Chromium's VaapiVideoDecodeLinuxGL export flow syncs each pool
        // surface right after `vaExportSurfaceHandle` and BEFORE its first
        // `vaBeginPicture`. Returning DECODING_ERROR there made Chromium tear
        // down the decoder at frame 1 and fall back to software; Mesa and
        // Intel drivers succeed on Empty (no pending work), so we do too.
        // Dead is the only terminal error path that returns DECODING_ERROR.
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        {
            let mut guard = state.lock.lock().unwrap();
            guard.surfaces[0] = Some(surface_with(SurfaceState::Empty, None));
            guard.surfaces[1] = Some(surface_with(SurfaceState::Ready, Some(0)));
            guard.surfaces[2] = Some(surface_with(SurfaceState::Dead, None));
        }

        assert_eq!(
            unsafe { sync_surface(&mut ctx, DRV_ID_BASE_SURFACE) },
            ok(),
            "Empty surface must sync-succeed (Chromium pool validity check)",
        );
        assert_eq!(
            unsafe { sync_surface(&mut ctx, DRV_ID_BASE_SURFACE + 1) },
            ok(),
            "Ready surface stays sync-successful",
        );
        assert_eq!(
            unsafe { sync_surface(&mut ctx, DRV_ID_BASE_SURFACE + 2) },
            err(VA_STATUS_ERROR_DECODING_ERROR),
            "Dead surface returns DECODING_ERROR",
        );

        unsafe { drop(Box::from_raw(raw)) };
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
