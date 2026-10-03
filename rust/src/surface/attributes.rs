//! Surface format, memory, and dimension attribute negotiation.
//!
//! These callbacks only describe the surface contract. Allocation and
//! CAPTURE ownership remain in the parent surface lifecycle module.

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use crate::state::{DRV_ID_BASE_CONFIG, DRV_MAX_DIM, DRV_MIN_DIM, config_index};
use crate::va_drm::exported_surface_memory_types;
use crate::{err, ok, state_from_ctx};
use std::ptr;

pub(crate) unsafe extern "C" fn query_surface_attributes(
    ctx: VADriverContextP,
    config_id: VAConfigID,
    attrib_list: *mut VASurfaceAttrib,
    num_attribs: *mut u32,
) -> VAStatus {
    if num_attribs.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if config_id < DRV_ID_BASE_CONFIG {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    }
    let idx = (config_id - DRV_ID_BASE_CONFIG) as usize;
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(format) = guard
        .configs
        .get(idx)
        .and_then(|config| config.as_ref())
        .map(|config| config.format)
    else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    drop(guard);

    const QUERY_ATTRS: [VASurfaceAttribType; 6] = [
        VASurfaceAttribType::VASurfaceAttribPixelFormat,
        VASurfaceAttribType::VASurfaceAttribMemoryType,
        VASurfaceAttribType::VASurfaceAttribMaxWidth,
        VASurfaceAttribType::VASurfaceAttribMaxHeight,
        VASurfaceAttribType::VASurfaceAttribMinWidth,
        VASurfaceAttribType::VASurfaceAttribMinHeight,
    ];
    let cap = unsafe { *num_attribs };
    if attrib_list.is_null() || cap < QUERY_ATTRS.len() as u32 {
        unsafe { *num_attribs = QUERY_ATTRS.len() as u32 };
        return ok();
    }

    for (i, ty) in QUERY_ATTRS.iter().copied().enumerate() {
        let attr = unsafe { &mut *attrib_list.add(i) };
        unsafe { ptr::write_bytes(attr, 0, 1) };
        attr.type_ = ty;
        fill_surface_attr(attr, SurfaceAttrMode::Query, format);
    }
    unsafe { *num_attribs = QUERY_ATTRS.len() as u32 };
    ok()
}

#[derive(Clone, Copy)]
enum SurfaceAttrMode {
    Query,
    Get,
}

fn set_surface_attr_value(a: &mut VASurfaceAttrib, flags: u32, value: i32) {
    a.flags = flags;
    a.value.type_ = VAGenericValueType::VAGenericValueTypeInteger;
    a.value.value.i = value;
}

fn fill_surface_attr(a: &mut VASurfaceAttrib, mode: SurfaceAttrMode, format: DecodedFormat) {
    match a.type_ {
        VASurfaceAttribType::VASurfaceAttribPixelFormat => {
            let flags = match mode {
                SurfaceAttrMode::Query => VA_SURFACE_ATTRIB_GETTABLE | VA_SURFACE_ATTRIB_SETTABLE,
                SurfaceAttrMode::Get => VA_SURFACE_ATTRIB_GETTABLE,
            };
            set_surface_attr_value(a, flags, format.va_fourcc() as i32)
        }
        VASurfaceAttribType::VASurfaceAttribMemoryType => set_surface_attr_value(
            a,
            VA_SURFACE_ATTRIB_GETTABLE | VA_SURFACE_ATTRIB_SETTABLE,
            exported_surface_memory_types(VA_SURFACE_ATTRIB_MEM_TYPE_VA) as i32,
        ),
        VASurfaceAttribType::VASurfaceAttribMinWidth => {
            set_surface_attr_value(a, VA_SURFACE_ATTRIB_GETTABLE, DRV_MIN_DIM)
        }
        VASurfaceAttribType::VASurfaceAttribMaxWidth => {
            set_surface_attr_value(a, VA_SURFACE_ATTRIB_GETTABLE, DRV_MAX_DIM)
        }
        VASurfaceAttribType::VASurfaceAttribMinHeight => {
            set_surface_attr_value(a, VA_SURFACE_ATTRIB_GETTABLE, DRV_MIN_DIM)
        }
        VASurfaceAttribType::VASurfaceAttribMaxHeight => {
            set_surface_attr_value(a, VA_SURFACE_ATTRIB_GETTABLE, DRV_MAX_DIM)
        }
        _ => {
            a.flags = 0;
            a.value.type_ = VAGenericValueType::VAGenericValueTypeInteger;
            a.value.value.i = VA_ATTRIB_NOT_SUPPORTED as i32;
        }
    }
}

pub(crate) unsafe extern "C" fn get_surface_attributes(
    ctx: VADriverContextP,
    config_id: VAConfigID,
    attrib_list: *mut VASurfaceAttrib,
    num_attribs: u32,
) -> VAStatus {
    if num_attribs > 0 && attrib_list.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let Some(idx) = config_index(config_id) else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    let guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let Some(format) = guard.configs[idx].as_ref().map(|config| config.format) else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    drop(guard);
    for i in 0..num_attribs as usize {
        let attr = unsafe { &mut *attrib_list.add(i) };
        fill_surface_attr(attr, SurfaceAttrMode::Get, format);
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attr(type_: VASurfaceAttribType) -> VASurfaceAttrib {
        VASurfaceAttrib {
            type_,
            flags: 0,
            value: VAGenericValue {
                type_: VAGenericValueType::VAGenericValueTypeInteger,
                value: _VAGenericValue__bindgen_ty_1 { i: 0 },
            },
        }
    }

    #[test]
    fn reports_pixel_format_for_query_and_get() {
        let mut query = attr(VASurfaceAttribType::VASurfaceAttribPixelFormat);
        fill_surface_attr(&mut query, SurfaceAttrMode::Query, DecodedFormat::Nv12);
        assert_eq!(unsafe { query.value.value.i } as u32, VA_FOURCC_NV12);
        assert_ne!(query.flags & VA_SURFACE_ATTRIB_GETTABLE, 0);
        assert_ne!(query.flags & VA_SURFACE_ATTRIB_SETTABLE, 0);

        let mut get = attr(VASurfaceAttribType::VASurfaceAttribPixelFormat);
        fill_surface_attr(&mut get, SurfaceAttrMode::Get, DecodedFormat::Nv12);
        assert_eq!(unsafe { get.value.value.i } as u32, VA_FOURCC_NV12);
        assert_eq!(get.flags, VA_SURFACE_ATTRIB_GETTABLE);

        let mut p010 = attr(VASurfaceAttribType::VASurfaceAttribPixelFormat);
        fill_surface_attr(&mut p010, SurfaceAttrMode::Get, DecodedFormat::P010);
        assert_eq!(unsafe { p010.value.value.i } as u32, VA_FOURCC_P010);
    }

    #[test]
    fn reports_va_and_prime_memory_types() {
        let mut memory = attr(VASurfaceAttribType::VASurfaceAttribMemoryType);
        fill_surface_attr(&mut memory, SurfaceAttrMode::Get, DecodedFormat::Nv12);
        let types = unsafe { memory.value.value.i } as u32;
        assert_ne!(types & VA_SURFACE_ATTRIB_MEM_TYPE_VA, 0);
        assert_ne!(
            types & crate::va_drm::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
            0
        );
    }
}
