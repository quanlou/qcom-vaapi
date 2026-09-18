//! Buffer metadata, external-handle, and sync callbacks.
//!
//! Decode and image buffers are CPU-owned `Vec<u8>` allocations. They can be
//! mapped through `vaMapBuffer`, but they do not have an exportable external
//! memory handle. These callbacks therefore validate handles precisely and
//! return explicit unsupported-memory status where libva asks for one.

use crate::bindings::*;
use crate::state::buffer_index;
use crate::{err, ok, state_from_ctx};

pub(crate) unsafe extern "C" fn buffer_info(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    type_out: *mut VABufferType,
    size: *mut u32,
    num_elements: *mut u32,
) -> VAStatus {
    if type_out.is_null() || size.is_null() || num_elements.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(buf) = guard.buffers[idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    unsafe {
        *type_out = buf.type_;
        *size = buf.elem_size;
        *num_elements = buf.num_elements;
    }
    ok()
}

pub(crate) unsafe extern "C" fn acquire_buffer_handle(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    buf_info: *mut VABufferInfo,
) -> VAStatus {
    if buf_info.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(buf) = guard.buffers[idx].as_ref() else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    unsafe {
        (*buf_info).handle = 0;
        (*buf_info).type_ = buf.type_ as u32;
        (*buf_info).mem_type = 0;
        (*buf_info).mem_size = buf.data.len();
    }
    err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE)
}

pub(crate) unsafe extern "C" fn release_buffer_handle(
    ctx: VADriverContextP,
    buf_id: VABufferID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if guard.buffers[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    }
    ok()
}

/// Synchronize a driver-owned CPU buffer.
///
/// Decode buffers are copied into the context's packet assembly during
/// `vaRenderPicture`, so they never have an asynchronous device operation to
/// wait for. We still validate the handle at this ABI boundary so callers get
/// the same precise invalid-buffer errors as the other buffer callbacks.
pub(crate) unsafe extern "C" fn sync_buffer(
    ctx: VADriverContextP,
    buf_id: VABufferID,
    _timeout_ns: u64,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = buffer_index(buf_id) else {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if guard.buffers[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_BUFFER);
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{Buffer, DRV_ID_BASE_BUFFER, DriverBox};
    use std::ffi::c_void;

    fn install_buffer(state: &DriverBox) {
        state.lock.lock().unwrap().buffers[0] = Some(Buffer {
            owner: VA_INVALID_ID,
            type_: VABufferType::VASliceDataBufferType,
            elem_size: 4,
            num_elements: 2,
            data: vec![1, 2, 3, 4, 5, 6, 7, 8],
            mapped: false,
        });
    }

    #[test]
    fn buffer_info_reports_owned_cpu_buffer_metadata() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        install_buffer(state);

        let mut type_out = VABufferType::VABufferTypeMax;
        let mut size = 0;
        let mut num_elements = 0;
        assert_eq!(
            unsafe {
                buffer_info(
                    &mut ctx,
                    DRV_ID_BASE_BUFFER,
                    &mut type_out,
                    &mut size,
                    &mut num_elements,
                )
            },
            ok()
        );
        assert_eq!(type_out, VABufferType::VASliceDataBufferType);
        assert_eq!(size, 4);
        assert_eq!(num_elements, 2);

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn acquire_buffer_handle_validates_then_reports_unsupported_memory() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        install_buffer(state);

        let mut info: VABufferInfo = unsafe { std::mem::zeroed() };
        assert_eq!(
            unsafe { acquire_buffer_handle(&mut ctx, DRV_ID_BASE_BUFFER, &mut info) },
            VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE as VAStatus
        );
        assert_eq!(info.handle, 0);
        assert_eq!(info.type_, VABufferType::VASliceDataBufferType as u32);
        assert_eq!(info.mem_type, 0);
        assert_eq!(info.mem_size, 8);

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn release_and_sync_validate_cpu_buffer_handles() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        install_buffer(state);

        assert_eq!(
            unsafe { release_buffer_handle(&mut ctx, DRV_ID_BASE_BUFFER) },
            ok()
        );
        assert_eq!(
            unsafe { sync_buffer(&mut ctx, DRV_ID_BASE_BUFFER, 0) },
            ok()
        );
        assert_eq!(
            unsafe { release_buffer_handle(&mut ctx, DRV_ID_BASE_BUFFER + 1) },
            VA_STATUS_ERROR_INVALID_BUFFER as VAStatus
        );
        assert_eq!(
            unsafe { sync_buffer(&mut ctx, DRV_ID_BASE_BUFFER + 1, 0) },
            VA_STATUS_ERROR_INVALID_BUFFER as VAStatus
        );

        unsafe { drop(Box::from_raw(raw)) };
    }
}
