#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]
#![allow(dead_code)]

#[allow(clippy::all)]
mod bindings;
mod buffer;
mod codec;
mod config;
mod context;
mod decode;
mod h264;
mod h265;
mod image;
mod state;
mod surface;
mod surface_export;
mod sync;
mod v4l2;
mod va_drm;
mod vtable;

use bindings::*;
use state::{DriverBox, VENDOR};
use std::ffi::{c_char, c_int, c_void};

fn ok() -> VAStatus {
    VA_STATUS_SUCCESS as VAStatus
}

fn err(code: u32) -> VAStatus {
    code as VAStatus
}

fn va_debug_enabled() -> bool {
    std::env::var_os("V4L2_VA_DEBUG").is_some()
}

pub(crate) unsafe fn state_from_ctx<'a>(ctx: VADriverContextP) -> Option<&'a DriverBox> {
    if ctx.is_null() || unsafe { (*ctx).pDriverData }.is_null() {
        return None;
    }
    Some(unsafe { &*((*ctx).pDriverData as *mut DriverBox) })
}

unsafe fn driver_init(ctx: VADriverContextP) -> VAStatus {
    if ctx.is_null() || unsafe { (*ctx).vtable }.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }

    let state = Box::new(DriverBox::new());
    unsafe {
        (*ctx).pDriverData = Box::into_raw(state) as *mut c_void;
        vtable::install_vtable((*ctx).vtable);
        vtable::install_vpp_stubs(ctx);
        (*ctx).version_major = VA_MAJOR_VERSION as c_int;
        (*ctx).version_minor = VA_MINOR_VERSION as c_int;
        // Profile-table wiring: the advertised count comes from the
        // V4L2-gated capability table in config.rs, not a static constant.
        (*ctx).max_profiles = config::advertised_profiles().len() as c_int;
        (*ctx).max_entrypoints = 1;
        (*ctx).max_attributes = 16;
        (*ctx).max_image_formats = 1;
        (*ctx).max_subpic_formats = 1;
        (*ctx).max_display_attributes = 1;
        (*ctx).str_vendor = VENDOR.as_ptr() as *const c_char;
    }
    ok()
}

#[unsafe(no_mangle)]
/// # Safety
///
/// libva calls this entry point with a valid driver context whose vtables are
/// writable for initialization.
pub unsafe extern "C" fn __vaDriverInit_1_24(ctx: VADriverContextP) -> VAStatus {
    unsafe { driver_init(ctx) }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// libva calls this compatibility entry point with a valid driver context whose
/// vtables are writable for initialization.
pub unsafe extern "C" fn __vaDriverInit_1_0(ctx: VADriverContextP) -> VAStatus {
    unsafe { driver_init(ctx) }
}
