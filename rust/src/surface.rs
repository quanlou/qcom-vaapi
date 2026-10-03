//! VA surface lifecycle and CAPTURE ownership.
//!
//! A VA surface is the client-visible identity for a V4L2 CAPTURE buffer.
//! This module validates surface attributes, allocates the identity table,
//! publishes completed buffers, and retires a CAPTURE slot when a surface is
//! destroyed or rendered again.

mod attributes;
mod status;

pub(crate) use attributes::{get_surface_attributes, query_surface_attributes};
pub(crate) use status::{query_surface_error, query_surface_status};

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use crate::state::{
    DRV_ID_BASE_SURFACE, DRV_MAX_ATTRIBUTE_LIST, DRV_MAX_DIM, DRV_MAX_SURFACES, DRV_MIN_DIM,
    DriverBox, DriverState, Surface, SurfaceState, context_index, surface_index,
};
use crate::surface_export::release_export_fds;
use crate::{err, ok, state_from_ctx, va_debug_enabled};
use std::ffi::c_int;
use std::slice;

fn create_surfaces_common(
    state: &DriverBox,
    width: i32,
    height: i32,
    format: DecodedFormat,
    num_surfaces: usize,
    surfaces: *mut VASurfaceID,
) -> VAStatus {
    if surfaces.is_null() || num_surfaces == 0 {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&width)
        || !(DRV_MIN_DIM..=DRV_MAX_DIM).contains(&height)
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_surfaces > DRV_MAX_SURFACES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let free: Vec<usize> = guard
        .surfaces
        .iter()
        .enumerate()
        .filter_map(|(i, s)| if s.is_none() { Some(i) } else { None })
        .take(num_surfaces)
        .collect();
    if free.len() != num_surfaces {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    for (out_idx, slot_idx) in free.into_iter().enumerate() {
        guard.surfaces[slot_idx] = Some(Surface {
            backing: None,
            width,
            height,
            format,
            state: SurfaceState::Empty,
            cap_idx: None,
            frame: None,
            owner: VA_INVALID_ID,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        });
        unsafe { *surfaces.add(out_idx) = DRV_ID_BASE_SURFACE + slot_idx as u32 };
    }
    ok()
}

fn validate_surface_creation_attributes(
    format: DecodedFormat,
    attributes: &[VASurfaceAttrib],
) -> VAStatus {
    for attribute in attributes {
        if attribute.flags & VA_SURFACE_ATTRIB_SETTABLE == 0 {
            continue;
        }
        if attribute.type_ == VASurfaceAttribType::VASurfaceAttribExternalBufferDescriptor {
            if attribute.value.type_ != VAGenericValueType::VAGenericValueTypePointer
                || unsafe { attribute.value.value.p }.is_null()
            {
                return err(VA_STATUS_ERROR_INVALID_PARAMETER);
            }
            continue;
        }
        if attribute.type_ == VASurfaceAttribType::VASurfaceAttribDRMFormatModifiers {
            if attribute.value.type_ != VAGenericValueType::VAGenericValueTypePointer
                || unsafe { attribute.value.value.p }.is_null()
            {
                return err(VA_STATUS_ERROR_INVALID_PARAMETER);
            }
            // The V4L2 CAPTURE allocation is exported as a linear surface. The
            // modifier list is an allocation preference, so accepting it here
            // lets clients negotiate against the explicit modifier returned
            // later in VADRMPRIMESurfaceDescriptor.
            continue;
        }
        if attribute.value.type_ != VAGenericValueType::VAGenericValueTypeInteger {
            return err(VA_STATUS_ERROR_INVALID_PARAMETER);
        }
        let value = unsafe { attribute.value.value.i } as u32;
        let status = match attribute.type_ {
            VASurfaceAttribType::VASurfaceAttribPixelFormat if value == format.va_fourcc() => ok(),
            VASurfaceAttribType::VASurfaceAttribPixelFormat => {
                err(VA_STATUS_ERROR_INVALID_PARAMETER)
            }
            VASurfaceAttribType::VASurfaceAttribMemoryType
                if value == VA_SURFACE_ATTRIB_MEM_TYPE_VA
                    || value == crate::va_drm::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME
                    || value == crate::va_drm::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 =>
            {
                ok()
            }
            VASurfaceAttribType::VASurfaceAttribMemoryType => {
                err(VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE)
            }
            // Advisory allocation hint. Chromium's VaapiVideoDecoder passes
            // VASurfaceAttribUsageHint (= DECODER) with the SETTABLE flag on
            // vaCreateSurfaces; it does not constrain the allocation, so accept
            // and ignore it. Rejecting it here fails vaCreateSurfaces with
            // ATTRIBUTE_NOT_SUPPORTED and forces the browser to software decode.
            VASurfaceAttribType::VASurfaceAttribUsageHint => ok(),
            _ => {
                if va_debug_enabled() {
                    eprintln!(
                        "msm_drv_video_rs: rejecting unsupported settable surface attribute type={:?}",
                        attribute.type_
                    );
                }
                err(VA_STATUS_ERROR_ATTR_NOT_SUPPORTED)
            }
        };
        if status != ok() {
            return status;
        }
    }
    ok()
}

pub(crate) unsafe extern "C" fn create_surfaces(
    ctx: VADriverContextP,
    width: c_int,
    height: c_int,
    format: c_int,
    num_surfaces: c_int,
    surfaces: *mut VASurfaceID,
) -> VAStatus {
    let Some(decoded_format) = DecodedFormat::from_rt_format(format as u32) else {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    };
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if num_surfaces <= 0 {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    create_surfaces_common(
        state,
        width,
        height,
        decoded_format,
        num_surfaces as usize,
        surfaces,
    )
}

pub(crate) unsafe extern "C" fn create_surfaces2(
    ctx: VADriverContextP,
    format: u32,
    width: u32,
    height: u32,
    surfaces: *mut VASurfaceID,
    num_surfaces: u32,
    attrib_list: *mut VASurfaceAttrib,
    num_attribs: u32,
) -> VAStatus {
    let Some(decoded_format) = DecodedFormat::from_rt_format(format) else {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    };
    if num_attribs > 0 && attrib_list.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_attribs as usize > DRV_MAX_ATTRIBUTE_LIST {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let attributes = if num_attribs > 0 {
        unsafe { slice::from_raw_parts(attrib_list, num_attribs as usize) }
    } else {
        &[]
    };
    let check = validate_surface_creation_attributes(decoded_format, attributes);
    if check != ok() {
        return check;
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if surfaces.is_null() || num_surfaces == 0 {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_surfaces as usize > DRV_MAX_SURFACES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    if !(DRV_MIN_DIM as u32..=DRV_MAX_DIM as u32).contains(&width)
        || !(DRV_MIN_DIM as u32..=DRV_MAX_DIM as u32).contains(&height)
    {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    match unsafe {
        crate::surface_import::parse_attributes(
            decoded_format,
            width,
            height,
            num_surfaces,
            attributes,
        )
    } {
        Ok(Some(imports)) => {
            return crate::surface_import::create_imported_surfaces(state, imports, surfaces);
        }
        Ok(None) => {}
        Err(status) => return status,
    }
    create_surfaces_common(
        state,
        width as i32,
        height as i32,
        decoded_format,
        num_surfaces as usize,
        surfaces,
    )
}

pub(crate) fn release_surface_capture(guard: &mut DriverState, surf_idx: usize) {
    let Some((owner, cap_idx, exported, tracked_exports)) = guard
        .surfaces
        .get(surf_idx)
        .and_then(|s| s.as_ref())
        .map(|s| (s.owner, s.cap_idx, s.exported, s.export_fds.len()))
    else {
        return;
    };
    if va_debug_enabled() && (cap_idx.is_some() || exported || tracked_exports > 0) {
        eprintln!(
            "msm_drv_video_rs: release surface={} cap_idx={:?} exported={} export_fds={}",
            DRV_ID_BASE_SURFACE + surf_idx as u32,
            cap_idx,
            exported,
            tracked_exports
        );
    }
    // Standalone PRIME storage belongs to this VA surface, not its decoder.
    // Keep its fd identity alive across picture reuse and context teardown.
    let standalone = guard.surfaces[surf_idx]
        .as_ref()
        .is_some_and(|surface| surface.backing.is_some());
    let Some(cap_idx) = cap_idx else {
        if standalone {
            return;
        }

        if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
            release_export_fds(surf);
        }
        return;
    };
    let exported_fds = if standalone {
        0
    } else if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
        release_export_fds(surf)
    } else {
        0
    };
    if exported && va_debug_enabled() {
        eprintln!(
            "msm_drv_video_rs: retiring exported surface={} cap_idx={} fds_closed={}",
            DRV_ID_BASE_SURFACE + surf_idx as u32,
            cap_idx,
            exported_fds
        );
    }
    if let Some(ctx_idx) = context_index(owner)
        && let Some(c) = guard.contexts[ctx_idx].as_mut()
        && let Some(v4l2) = c.v4l2.as_mut()
    {
        // Retire the surface's tracked dups from the slot's export
        // accounting before the slot returns to the kernel queue, so the
        // requeue always follows the retire.
        v4l2.retire_slot_exports(cap_idx, exported_fds);
        v4l2.requeue_capture(cap_idx);
    }
    if let Some(surf) = guard.surfaces[surf_idx].as_mut() {
        surf.cap_idx = None;
    }
}

pub(crate) unsafe extern "C" fn destroy_surfaces(
    ctx: VADriverContextP,
    surface_list: *mut VASurfaceID,
    num_surfaces: c_int,
) -> VAStatus {
    if num_surfaces < 0 || (num_surfaces > 0 && surface_list.is_null()) {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_surfaces as usize > DRV_MAX_SURFACES {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    let ids = unsafe { slice::from_raw_parts(surface_list, num_surfaces as usize) };
    let mut indices = Vec::with_capacity(ids.len());
    for &id in ids {
        if id == VA_INVALID_ID {
            continue;
        }
        let Some(idx) = surface_index(id) else {
            return err(VA_STATUS_ERROR_INVALID_SURFACE);
        };
        if guard.surfaces[idx].is_none() {
            return err(VA_STATUS_ERROR_INVALID_SURFACE);
        }
        if guard
            .contexts
            .iter()
            .flatten()
            .any(|context| context.frame_open && context.render_target == id)
        {
            return err(VA_STATUS_ERROR_OPERATION_FAILED);
        }
        if !indices.contains(&idx) {
            indices.push(idx);
        }
    }
    for idx in indices {
        release_surface_capture(&mut guard, idx);
        guard.surfaces[idx] = None;
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{DRV_MAX_SURFACES, DriverBox};
    use crate::va_drm::VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2;
    use std::ffi::c_void;

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

    fn surface_with(state: SurfaceState) -> Surface {
        Surface {
            backing: None,
            width: 16,
            height: 16,
            format: DecodedFormat::Nv12,
            state,
            cap_idx: None,
            frame: None,
            owner: VA_INVALID_ID,
            exported: false,
            export_count: 0,
            export_fds: Vec::new(),
        }
    }

    #[test]
    fn validates_only_supported_settable_surface_attributes() {
        let fmt = DecodedFormat::Nv12;
        let mut pixel = attr(VASurfaceAttribType::VASurfaceAttribPixelFormat);
        pixel.flags = VA_SURFACE_ATTRIB_SETTABLE;
        pixel.value.value.i = VA_FOURCC_NV12 as i32;
        assert_eq!(validate_surface_creation_attributes(fmt, &[pixel]), ok());

        pixel.value.value.i = VA_FOURCC_YUY2 as i32;
        assert_eq!(
            validate_surface_creation_attributes(fmt, &[pixel]),
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );

        pixel.value.value.i = VA_FOURCC_P010 as i32;
        assert_eq!(
            validate_surface_creation_attributes(DecodedFormat::P010, &[pixel]),
            ok()
        );

        let mut memory = attr(VASurfaceAttribType::VASurfaceAttribMemoryType);
        memory.flags = VA_SURFACE_ATTRIB_SETTABLE;
        memory.value.value.i = VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2 as i32;
        assert_eq!(validate_surface_creation_attributes(fmt, &[memory]), ok());
        memory.value.value.i = VA_SURFACE_ATTRIB_MEM_TYPE_USER_PTR as i32;
        assert_eq!(
            validate_surface_creation_attributes(fmt, &[memory]),
            VA_STATUS_ERROR_UNSUPPORTED_MEMORY_TYPE as VAStatus
        );

        // A non-settable attribute is ignored regardless of type.
        let ignored = attr(VASurfaceAttribType::VASurfaceAttribUsageHint);
        assert_eq!(validate_surface_creation_attributes(fmt, &[ignored]), ok());

        // Chromium's VaapiVideoDecoder passes a SETTABLE usage hint on
        // vaCreateSurfaces; it must be accepted, not rejected as unsupported.
        let mut usage_hint = attr(VASurfaceAttribType::VASurfaceAttribUsageHint);
        usage_hint.flags = VA_SURFACE_ATTRIB_SETTABLE;
        usage_hint.value.value.i = VA_SURFACE_ATTRIB_USAGE_HINT_DECODER as i32;
        assert_eq!(
            validate_surface_creation_attributes(fmt, &[usage_hint]),
            ok()
        );

        // External imports require a non-null pointer descriptor.
        let mut external = attr(VASurfaceAttribType::VASurfaceAttribExternalBufferDescriptor);
        external.flags = VA_SURFACE_ATTRIB_SETTABLE;
        assert_eq!(
            validate_surface_creation_attributes(fmt, &[external]),
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );
    }

    #[test]
    fn create_surfaces2_rejects_null_attributes_before_allocation() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let mut surface_id = VA_INVALID_ID;

        assert_eq!(
            unsafe {
                create_surfaces2(
                    &mut ctx,
                    VA_RT_FORMAT_YUV420,
                    16,
                    16,
                    &mut surface_id,
                    1,
                    std::ptr::null_mut(),
                    1,
                )
            },
            VA_STATUS_ERROR_INVALID_PARAMETER as VAStatus
        );
        assert_eq!(surface_id, VA_INVALID_ID);
        assert!(state.lock.lock().unwrap().surfaces[0].is_none());

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn create_surfaces2_rejects_excessive_surface_count_before_allocation() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        let mut surface_id = VA_INVALID_ID;

        assert_eq!(
            unsafe {
                create_surfaces2(
                    &mut ctx,
                    VA_RT_FORMAT_YUV420,
                    16,
                    16,
                    &mut surface_id,
                    (DRV_MAX_SURFACES + 1) as u32,
                    std::ptr::null_mut(),
                    0,
                )
            },
            VA_STATUS_ERROR_MAX_NUM_EXCEEDED as VAStatus
        );
        assert_eq!(surface_id, VA_INVALID_ID);
        assert!(state.lock.lock().unwrap().surfaces[0].is_none());

        unsafe { drop(Box::from_raw(raw)) };
    }

    #[test]
    fn destroy_surfaces_validates_the_full_list_before_mutating() {
        let raw = Box::into_raw(Box::new(DriverBox::new()));
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = raw as *mut c_void;
        let state = unsafe { &*raw };
        state.lock.lock().unwrap().surfaces[0] = Some(surface_with(SurfaceState::Empty));
        let ids = [
            DRV_ID_BASE_SURFACE,
            DRV_ID_BASE_SURFACE + DRV_MAX_SURFACES as u32,
        ];

        assert_eq!(
            unsafe {
                destroy_surfaces(
                    &mut ctx,
                    ids.as_ptr() as *mut VASurfaceID,
                    ids.len() as c_int,
                )
            },
            VA_STATUS_ERROR_INVALID_SURFACE as VAStatus
        );
        assert!(state.lock.lock().unwrap().surfaces[0].is_some());

        unsafe { drop(Box::from_raw(raw)) };
    }
}
