//! VA buffer lifecycle callbacks.
//!
//! Decode clients pass picture parameters and slice payloads through VA buffers.
//! This module owns the handle table operations and mapped byte storage; H.264
//! interpretation remains in the decode entrypoint module.

mod handles;

pub(crate) use handles::{acquire_buffer_handle, buffer_info, release_buffer_handle, sync_buffer};

use crate::bindings::*;
use crate::state::{Buffer, DRV_ID_BASE_BUFFER, DRV_MAX_BUFFER_BYTES, buffer_index, context_index};
use crate::{err, ok, state_from_ctx};
use std::ffi::c_void;
use std::ptr;
use std::slice;

fn buffer_type_valid(type_: VABufferType) -> bool {
    matches!(
        type_,
        VABufferType::VAPictureParameterBufferType
            | VABufferType::VAIQMatrixBufferType
            | VABufferType::VASliceParameterBufferType
            | VABufferType::VASliceDataBufferType
            | VABufferType::VAImageBufferType
    )
}

fn buffer_storage_len(elem_size: u32, num_elements: u32) -> Option<usize> {
    (elem_size as usize).checked_mul(num_elements as usize)
}

fn buffer_is_image_backing(guard: &crate::state::DriverState, buffer_id: VABufferID) -> bool {
    guard
        .images
        .iter()
        .flatten()
        .any(|image| image.image.buf == buffer_id)
}

pub(crate) unsafe extern "C" fn create_buffer(
    ctx: VADriverContextP,
    context: VAContextID,
    type_: VABufferType,
    size: u32,
    num_elements: u32,
    data: *mut c_void,
    buf_id: *mut VABufferID,
) -> VAStatus {
    if buf_id.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if size == 0 || num_elements == 0 {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if !buffer_type_valid(type_) {
        return err(VA_STATUS_ERROR_UNSUPPORTED_BUFFERTYPE);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(ctx_idx) = context_index(context) else {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    };
    if guard.contexts[ctx_idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_CONTEXT);
    }
    let Some(total) = buffer_storage_len(size, num_elements) else {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    };
    if total > DRV_MAX_BUFFER_BYTES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let mut bytes = vec![0u8; total];
    if !data.is_null() && total > 0 {
        unsafe { bytes.copy_from_slice(slice::from_raw_parts(data as *const u8, total)) };
    }
    if let Some((idx, slot)) = guard
        .buffers
        .iter_mut()
        .enumerate()
        .find(|(_, v)| v.is_none())
    {
        *slot = Some(Buffer {
            owner: context,
            type_,
            elem_size: size,
            num_elements,
            data: bytes,
            mapped: false,
        });
        unsafe { *buf_id = DRV_ID_BASE_BUFFER + idx as u32 };
        ok()
    } else {
        err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED)
    }
}

pub(crate) unsafe extern "C" fn destroy_buffer(
    ctx: VADriverContextP,
    buffer_id: VABufferID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buffer_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if guard.buffers[idx].as_ref().is_some_and(|buf| buf.mapped) {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if buffer_is_image_backing(&guard, buffer_id) {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if guard.buffers[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    }
    guard.buffers[idx] = None;
    ok()
}

pub(crate) unsafe extern "C" fn buffer_set_num_elements(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    num_elements: u32,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if buffer_is_image_backing(&guard, buf_id) {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    let Some(buf) = guard.buffers[idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    if buf.mapped {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    if num_elements > buf.num_elements {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(new_len) = buffer_storage_len(buf.elem_size, num_elements) else {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    };
    if new_len > DRV_MAX_BUFFER_BYTES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    buf.num_elements = num_elements;
    buf.data.truncate(new_len);
    ok()
}

pub(crate) unsafe extern "C" fn map_buffer(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    pbuf: *mut *mut c_void,
) -> VAStatus {
    unsafe { map_buffer2(ctx, buf_id, pbuf, VA_MAPBUFFER_FLAG_DEFAULT) }
}

pub(crate) unsafe extern "C" fn map_buffer2(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    pbuf: *mut *mut c_void,
    _flags: u32,
) -> VAStatus {
    if pbuf.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    unsafe { *pbuf = ptr::null_mut() };
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(buf) = guard.buffers[idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    if buf.mapped {
        unsafe { *pbuf = buf.data.as_mut_ptr() as *mut c_void };
        return ok();
    }
    buf.mapped = true;
    unsafe { *pbuf = buf.data.as_mut_ptr() as *mut c_void };
    ok()
}

pub(crate) unsafe extern "C" fn unmap_buffer(
    ctx: VADriverContextP,
    buf_id: VABufferID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(buf) = guard.buffers[idx].as_mut() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    buf.mapped = false;
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::h264::H264Synth;
    use crate::state::{
        Context, DRV_ID_BASE_BUFFER, DRV_ID_BASE_CONFIG, DRV_ID_BASE_CONTEXT, DRV_MAX_BUFFER_BYTES,
        DriverBox,
    };
    use std::ffi::c_void;

    fn context_for_test() -> Context {
        Context {
            config_id: DRV_ID_BASE_CONFIG,
            profile: VAProfile::VAProfileH264Main,
            entrypoint: VAEntrypoint::VAEntrypointVLD,
            width: 320,
            height: 240,
            render_targets: Vec::new(),
            frame_open: false,
            render_target: VA_INVALID_ID,
            slices: Vec::new(),
            syn: H264Synth::new(VAProfile::VAProfileH264Main),
            out_seq: 0,
            first_poc: None,
            poc_epoch_usec: 0,
            max_timestamp_usec: 0,
            v4l2: None,
        }
    }

    #[test]
    fn buffer_storage_length_is_checked() {
        assert_eq!(buffer_storage_len(128, 4), Some(512));
        assert_eq!(buffer_storage_len(0, 0), Some(0));
    }

    #[test]
    fn create_buffer_records_its_context_owner() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test());
        let mut buffer_id = VA_INVALID_ID;
        let mut initial = [1_u8, 2, 3, 4];

        assert_eq!(
            unsafe {
                create_buffer(
                    &mut ctx,
                    DRV_ID_BASE_CONTEXT,
                    VABufferType::VASliceDataBufferType,
                    initial.len() as u32,
                    1,
                    initial.as_mut_ptr() as *mut c_void,
                    &mut buffer_id,
                )
            },
            VA_STATUS_SUCCESS as VAStatus
        );
        let guard = state.lock.lock().unwrap();
        assert_eq!(
            guard.buffers[0].as_ref().unwrap().owner,
            DRV_ID_BASE_CONTEXT
        );
        assert_eq!(guard.buffers[0].as_ref().unwrap().data, initial);
        drop(guard);
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn create_buffer_rejects_excessive_allocations() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().contexts[0] = Some(context_for_test());
        let mut buffer_id = VA_INVALID_ID;

        assert_eq!(
            unsafe {
                create_buffer(
                    &mut ctx,
                    DRV_ID_BASE_CONTEXT,
                    VABufferType::VASliceDataBufferType,
                    (DRV_MAX_BUFFER_BYTES as u32).saturating_add(1),
                    1,
                    std::ptr::null_mut(),
                    &mut buffer_id,
                )
            },
            VA_STATUS_ERROR_MAX_NUM_EXCEEDED as VAStatus
        );
        assert_eq!(buffer_id, VA_INVALID_ID);
        assert!(state.lock.lock().unwrap().buffers[0].is_none());

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn mapped_buffer_cannot_be_resized_or_destroyed_until_unmapped() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().buffers[0] = Some(Buffer {
            owner: VA_INVALID_ID,
            type_: VABufferType::VASliceDataBufferType,
            elem_size: 4,
            num_elements: 2,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            mapped: false,
        });
        let id = DRV_ID_BASE_BUFFER;
        let mut first = ptr::null_mut();
        assert_eq!(
            unsafe { map_buffer(&mut ctx, id, &mut first) },
            VA_STATUS_SUCCESS as VAStatus
        );
        let mut second = ptr::null_mut();
        assert_eq!(
            unsafe { map_buffer2(&mut ctx, id, &mut second, VA_MAPBUFFER_FLAG_DEFAULT) },
            VA_STATUS_SUCCESS as VAStatus
        );
        assert_eq!(first, second);
        assert_eq!(
            unsafe { buffer_set_num_elements(&mut ctx, id, 1) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert_eq!(
            unsafe { destroy_buffer(&mut ctx, id) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert_eq!(
            unsafe { unmap_buffer(&mut ctx, id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        assert_eq!(
            unsafe { buffer_set_num_elements(&mut ctx, id, 1) },
            VA_STATUS_SUCCESS as VAStatus
        );
        assert_eq!(
            unsafe { destroy_buffer(&mut ctx, id) },
            VA_STATUS_SUCCESS as VAStatus
        );
        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn image_backing_buffer_cannot_be_destroyed_or_resized_directly() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let id = DRV_ID_BASE_BUFFER;
        state.lock.lock().unwrap().buffers[0] = Some(Buffer {
            owner: VA_INVALID_ID,
            type_: VABufferType::VAImageBufferType,
            elem_size: 4,
            num_elements: 2,
            data: vec![0; 8],
            mapped: false,
        });
        let mut image: VAImage = unsafe { std::mem::zeroed() };
        image.buf = id;
        state.lock.lock().unwrap().images[0] = Some(crate::state::Image { image });

        assert_eq!(
            unsafe { buffer_set_num_elements(&mut ctx, id, 1) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );
        assert_eq!(
            unsafe { destroy_buffer(&mut ctx, id) },
            VA_STATUS_ERROR_OPERATION_FAILED as VAStatus
        );

        unsafe { drop(Box::from_raw(raw)) };
    }
}
