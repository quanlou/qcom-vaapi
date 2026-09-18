//! CPU-copy image helpers.
//!
//! The browser-grade path should eventually use dma-buf export, but FFmpeg,
//! mpv `vaapi-copy`, and fallback client paths still rely on VAImage buffers.
//! Keep the NV12 layout math here so the libva callbacks only validate handles
//! and move data between driver objects.

mod layout;

pub(crate) use layout::{
    aligned_nv12_pitch, copy_nv12_region, is_nv12, make_nv12_image, nv12_data_size, nv12_format,
    region_within,
};

use crate::bindings::*;
use crate::state::{
    Buffer, DRV_ID_BASE_BUFFER, DRV_ID_BASE_IMAGE, DRV_MAX_DIM, DRV_MIN_DIM, Image, SurfaceState,
    buffer_index, context_index, image_index, surface_index,
};
use crate::sync::sync_surface;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::ptr;

pub(crate) unsafe extern "C" fn query_image_formats(
    _ctx: VADriverContextP,
    format_list: *mut VAImageFormat,
    num_formats: *mut c_int,
) -> VAStatus {
    if format_list.is_null() || num_formats.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    unsafe {
        ptr::write_bytes(format_list, 0, 1);
        (*format_list).fourcc = VA_FOURCC_NV12;
        (*format_list).byte_order = VA_LSB_FIRST;
        (*format_list).bits_per_pixel = 12;
        *num_formats = 1;
    }
    ok()
}

pub(crate) unsafe extern "C" fn create_image(
    ctx: VADriverContextP,
    format: *mut VAImageFormat,
    width: c_int,
    height: c_int,
    image: *mut VAImage,
) -> VAStatus {
    if format.is_null() || image.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let fmt = unsafe { *format };
    if !is_nv12(&fmt) {
        return err(VA_STATUS_ERROR_INVALID_IMAGE_FORMAT);
    }
    if !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&width)
        || !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&height)
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
    let pitch = aligned_nv12_pitch(width as u32);
    let data_size = nv12_data_size(pitch, height as u32);
    let Some(buf_idx) = guard.buffers.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let Some(img_idx) = guard.images.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let buf_id = DRV_ID_BASE_BUFFER + buf_idx as u32;
    let image_id = DRV_ID_BASE_IMAGE + img_idx as u32;
    let img = make_nv12_image(
        image_id,
        buf_id,
        width as u16,
        height as u16,
        pitch,
        height as u32,
        fmt,
    );
    guard.buffers[buf_idx] = Some(Buffer {
        owner: VA_INVALID_ID,
        type_: VABufferType::VAImageBufferType,
        elem_size: data_size,
        num_elements: 1,
        data: vec![0; data_size as usize],
        mapped: false,
    });
    guard.images[img_idx] = Some(Image { image: img });
    unsafe { *image = img };
    ok()
}

pub(crate) unsafe extern "C" fn destroy_image(
    ctx: VADriverContextP,
    image_id: VAImageID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(img_idx) = image_index(image_id) else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(img_ref) = guard.images[img_idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE);
    };
    let Some(buf_idx) = buffer_index(img_ref.image.buf) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let Some(buf) = guard.buffers[buf_idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    if buf.mapped {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    let Some(_img) = guard.images[img_idx].take() else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE);
    };
    guard.buffers[buf_idx] = None;
    ok()
}

pub(crate) unsafe extern "C" fn get_image(
    ctx: VADriverContextP,
    surface: VASurfaceID,
    x: c_int,
    y: c_int,
    width: u32,
    height: u32,
    image_id: VAImageID,
) -> VAStatus {
    let sync = unsafe { sync_surface(ctx, surface) };
    if sync != ok() {
        return sync;
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(surf_idx) = surface_index(surface) else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let Some(img_idx) = image_index(image_id) else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some((surf_width, surf_height, surf_state, surf_cap_idx, surf_owner)) = guard.surfaces
        [surf_idx]
        .as_ref()
        .map(|s| (s.width, s.height, s.state, s.cap_idx, s.owner))
    else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let Some(img) = guard.images[img_idx].as_ref().cloned() else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE);
    };
    if !region_within(
        (surf_width as u32, surf_height as u32),
        (img.image.width as u32, img.image.height as u32),
        (x, y),
        (width, height),
    ) {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if surf_state != SurfaceState::Ready {
        return err(VA_STATUS_ERROR_DECODING_ERROR);
    }
    let Some(cap_idx) = surf_cap_idx else {
        return err(VA_STATUS_ERROR_DECODING_ERROR);
    };
    let Some(ctx_idx) = context_index(surf_owner) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let Some(c) = guard.contexts[ctx_idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    let Some((cap, cap_stride, cap_h)) = c.v4l2.as_ref().and_then(|v| v.capture_copy(cap_idx))
    else {
        return err(VA_STATUS_ERROR_DECODING_ERROR);
    };
    let Some(buf_idx) = buffer_index(img.image.buf) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let Some(buf) = guard.buffers[buf_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    // vaGetImage writes the destination image. Do not race a client that still
    // owns a mapped pointer into the same backing allocation.
    if buf.mapped {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    copy_nv12_region(
        &cap,
        cap_stride,
        cap_h,
        &mut buf.data,
        img.image.pitches[0],
        img.image.offsets[1],
        x as usize,
        y as usize,
        width as usize,
        height as usize,
    );
    ok()
}

pub(crate) unsafe extern "C" fn derive_image(
    ctx: VADriverContextP,
    surface: VASurfaceID,
    image: *mut VAImage,
) -> VAStatus {
    if image.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let sync = unsafe { sync_surface(ctx, surface) };
    if sync != ok() {
        return sync;
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(surf_idx) = surface_index(surface) else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some((surf_width, surf_height, surf_state, surf_cap_idx, surf_owner)) = guard.surfaces
        [surf_idx]
        .as_ref()
        .map(|s| (s.width, s.height, s.state, s.cap_idx, s.owner))
    else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };

    let mut pitch = aligned_nv12_pitch(surf_width as u32);
    let mut cap_h = surf_height as u32;
    let mut data: Vec<u8> = Vec::new();

    if surf_state == SurfaceState::Ready
        && let (Some(cap_idx), Some(ctx_idx)) = (surf_cap_idx, context_index(surf_owner))
        && let Some(c) = guard.contexts[ctx_idx].as_ref()
        && let Some((cap, cap_stride, cap_height)) =
            c.v4l2.as_ref().and_then(|v| v.capture_copy(cap_idx))
    {
        pitch = cap_stride;
        cap_h = cap_height;
        let data_size = nv12_data_size(pitch, cap_h);
        data = cap.into_iter().take(data_size as usize).collect();
    }

    let data_size = nv12_data_size(pitch, cap_h);
    if data.is_empty() {
        data = vec![0; data_size as usize];
    } else if data.len() < data_size as usize {
        data.resize(data_size as usize, 0);
    }

    let Some(buf_idx) = guard.buffers.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let Some(img_idx) = guard.images.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let buf_id = DRV_ID_BASE_BUFFER + buf_idx as u32;
    let image_id = DRV_ID_BASE_IMAGE + img_idx as u32;
    let img = make_nv12_image(
        image_id,
        buf_id,
        surf_width as u16,
        surf_height as u16,
        pitch,
        cap_h,
        nv12_format(),
    );

    guard.buffers[buf_idx] = Some(Buffer {
        owner: VA_INVALID_ID,
        type_: VABufferType::VAImageBufferType,
        elem_size: data_size,
        num_elements: 1,
        data,
        mapped: false,
    });
    guard.images[img_idx] = Some(Image { image: img });
    unsafe { *image = img };
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Buffer, DRV_ID_BASE_BUFFER, DriverBox, Image};
    use std::ffi::c_void;

    #[test]
    fn image_destroy_waits_for_mapped_backing_buffer() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let image_id = DRV_ID_BASE_IMAGE;
        let buffer_id = DRV_ID_BASE_BUFFER;
        state.lock.lock().unwrap().buffers[0] = Some(Buffer {
            owner: VA_INVALID_ID,
            type_: VABufferType::VAImageBufferType,
            elem_size: 16,
            num_elements: 1,
            data: vec![0; 16],
            mapped: true,
        });
        state.lock.lock().unwrap().images[0] = Some(Image {
            image: make_nv12_image(
                image_id,
                buffer_id,
                16,
                16,
                aligned_nv12_pitch(16),
                16,
                nv12_format(),
            ),
        });

        assert_eq!(
            unsafe { destroy_image(&mut ctx, image_id) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert!(state.lock.lock().unwrap().images[0].is_some());

        state.lock.lock().unwrap().buffers[0]
            .as_mut()
            .unwrap()
            .mapped = false;
        assert_eq!(
            unsafe { destroy_image(&mut ctx, image_id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        let guard = state.lock.lock().unwrap();
        assert!(guard.images[0].is_none());
        assert!(guard.buffers[0].is_none());
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn derive_image_rejects_null_output_before_syncing() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;

        assert_eq!(
            unsafe { derive_image(&mut ctx, VA_INVALID_ID, std::ptr::null_mut()) },
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );

        unsafe { drop(Box::from_raw(raw)) };
    }
}
