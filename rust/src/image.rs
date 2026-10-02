//! CPU-copy image helpers.
//!
//! The browser-grade path should eventually use dma-buf export, but FFmpeg,
//! mpv `vaapi-copy`, and fallback client paths still rely on VAImage buffers.
//! Keep the semiplanar layout math here so the libva callbacks only validate
//! handles and move data between driver objects.

mod layout;

pub(crate) use layout::{
    aligned_pitch, copy_semiplanar_region, decoded_format_from_image, image_data_size,
    image_format, make_image, region_within,
};

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use crate::state::{
    Buffer, DRV_ID_BASE_BUFFER, DRV_ID_BASE_IMAGE, DRV_MAX_DIM, DRV_MIN_DIM, DriverState, Image,
    SurfaceState, buffer_index, image_index, surface_index,
};
use crate::sync::sync_surface;
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::ptr;

/// Image formats advertised via `vaQueryImageFormats`. Keep this in sync with
/// `VADriverContext::max_image_formats`: FFmpeg allocates exactly that many
/// entries before calling the query callback, and its VAAPI Main10 path also
/// requires P010 to be present here before it creates P010 render surfaces.
pub(crate) const SUPPORTED_IMAGE_FORMATS: [DecodedFormat; 2] =
    [DecodedFormat::Nv12, DecodedFormat::P010];

pub(crate) unsafe extern "C" fn query_image_formats(
    _ctx: VADriverContextP,
    format_list: *mut VAImageFormat,
    num_formats: *mut c_int,
) -> VAStatus {
    if format_list.is_null() || num_formats.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    unsafe {
        ptr::write_bytes(format_list, 0, SUPPORTED_IMAGE_FORMATS.len());
        for (idx, format) in SUPPORTED_IMAGE_FORMATS.iter().copied().enumerate() {
            *format_list.add(idx) = image_format(format);
        }
        *num_formats = SUPPORTED_IMAGE_FORMATS.len() as c_int;
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
    let Some(decoded_format) = decoded_format_from_image(&fmt) else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE_FORMAT);
    };
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
    let pitch = aligned_pitch(decoded_format, width as u32);
    let data_size = image_data_size(pitch, height as u32);
    let Some(buf_idx) = guard.buffers.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let Some(img_idx) = guard.images.iter().position(|v| v.is_none()) else {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    };
    let buf_id = DRV_ID_BASE_BUFFER + buf_idx as u32;
    let image_id = DRV_ID_BASE_IMAGE + img_idx as u32;
    let img = make_image(
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
    // Surfaces and image buffers are disjoint tables. Borrow the immutable
    // published snapshot while writing the image instead of cloning a whole
    // decoded frame (about 12 MiB for 4K NV12) under the driver lock.
    let DriverState {
        surfaces,
        images,
        buffers,
        ..
    } = &mut *guard;
    let Some((surf_width, surf_height, surf_state, surf_frame)) = surfaces[surf_idx]
        .as_ref()
        .map(|s| (s.width, s.height, s.state, s.frame.as_ref()))
    else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };
    let Some(img) = images[img_idx].as_ref().cloned() else {
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
    // Read the snapshot taken when the frame was dequeued. The CAPTURE slot
    // itself may already have been requeued and overwritten by the decoder.
    // AV1 hidden-reference frames complete without display pixels; keep
    // vaGetImage consistent with vaDeriveImage by exposing a zeroed image
    // rather than turning a successful no-output decode into a hard error.
    let zeroed_frame;
    let (cap, cap_stride, cap_h, frame_format) = if let Some(frame) = surf_frame {
        (
            frame.data.as_slice(),
            frame.stride,
            frame.height,
            frame.format,
        )
    } else {
        let format = surfaces[surf_idx]
            .as_ref()
            .map(|surface| surface.format)
            .unwrap_or(DecodedFormat::Nv12);
        let stride = aligned_pitch(format, surf_width as u32);
        let height = surf_height as u32;
        zeroed_frame = vec![0; image_data_size(stride, height) as usize];
        (zeroed_frame.as_slice(), stride, height, format)
    };
    let Some(buf_idx) = buffer_index(img.image.buf) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let Some(buf) = buffers[buf_idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    // vaGetImage writes the destination image. Do not race a client that still
    // owns a mapped pointer into the same backing allocation.
    if buf.mapped {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    let Some(image_format) = decoded_format_from_image(&img.image.format) else {
        return err(VA_STATUS_ERROR_INVALID_IMAGE_FORMAT);
    };
    if image_format != frame_format {
        return err(VA_STATUS_ERROR_INVALID_IMAGE_FORMAT);
    }
    if !copy_semiplanar_region(
        frame_format,
        cap,
        cap_stride,
        cap_h,
        &mut buf.data,
        img.image.pitches[0],
        img.image.offsets[1],
        x as usize,
        y as usize,
        width as usize,
        height as usize,
    ) {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
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
    let Some((surf_width, surf_height, surf_state, surf_frame)) = guard.surfaces[surf_idx]
        .as_ref()
        .map(|s| (s.width, s.height, s.state, s.frame.as_ref()))
    else {
        return err(VA_STATUS_ERROR_INVALID_SURFACE);
    };

    let mut frame_format = guard.surfaces[surf_idx]
        .as_ref()
        .map(|surface| surface.format)
        .unwrap_or(DecodedFormat::Nv12);
    let mut pitch = aligned_pitch(frame_format, surf_width as u32);
    let mut cap_h = surf_height as u32;
    let mut data: Vec<u8> = Vec::new();

    // Read the snapshot taken when the frame was dequeued; the CAPTURE slot
    // itself may already have been requeued and overwritten by the decoder.
    if surf_state == SurfaceState::Ready
        && let Some(frame) = surf_frame
    {
        pitch = frame.stride;
        cap_h = frame.height;
        frame_format = frame.format;
        let data_size = image_data_size(pitch, cap_h);
        if pitch < (surf_width as u32).div_ceil(2) * 2 * frame_format.bytes_per_sample()
            || cap_h < surf_height as u32
            || data_size as usize > crate::state::DRV_MAX_BUFFER_BYTES
            || frame.data.len() < data_size as usize
        {
            return err(VA_STATUS_ERROR_DECODING_ERROR);
        }
        data = frame.data[..data_size as usize].to_vec();
    }

    let data_size = image_data_size(pitch, cap_h);
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
    let img = make_image(
        image_id,
        buf_id,
        surf_width as u16,
        surf_height as u16,
        pitch,
        cap_h,
        image_format(frame_format),
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
mod tests;
