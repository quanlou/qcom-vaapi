use crate::bindings::*;
use crate::buffer::{
    acquire_buffer_handle, buffer_info, buffer_set_num_elements, create_buffer, destroy_buffer,
    map_buffer, map_buffer2, release_buffer_handle, sync_buffer, unmap_buffer,
};
use crate::config::{
    create_config, destroy_config, get_config_attributes, get_display_attributes,
    query_config_attributes, query_config_entrypoints, query_config_profiles,
    query_display_attributes, query_subpicture_formats,
};
use crate::context::{create_context, destroy_context};
use crate::decode::{begin_picture, end_picture, render_picture};
use crate::image::{create_image, derive_image, destroy_image, get_image, query_image_formats};
use crate::ok;
use crate::state::DriverBox;
use crate::surface::{
    create_surfaces, create_surfaces2, destroy_surfaces, get_surface_attributes,
    query_surface_attributes, query_surface_error, query_surface_status,
};
use crate::surface_export::export_surface_handle;
use crate::sync::{sync_surface, sync_surface2};
use std::ptr;

mod unsupported;

use unsupported::*;

unsafe extern "C" fn terminate(ctx: VADriverContextP) -> VAStatus {
    if ctx.is_null() {
        return ok();
    }
    let ptr = unsafe { (*ctx).pDriverData as *mut DriverBox };
    if !ptr.is_null() {
        drop(unsafe { Box::from_raw(ptr) });
        unsafe { (*ctx).pDriverData = ptr::null_mut() };
    }
    ok()
}

pub(crate) unsafe fn install_vtable(vt: *mut VADriverVTable) {
    if vt.is_null() {
        return;
    }
    unsafe {
        (*vt).vaPutSurface = Some(reject_put_surface);
        (*vt).vaSetImagePalette = Some(reject_set_image_palette);
        (*vt).vaPutImage = Some(reject_put_image);
        (*vt).vaCreateSubpicture = Some(reject_create_subpicture);
        (*vt).vaDestroySubpicture = Some(reject_destroy_subpicture);
        (*vt).vaSetSubpictureImage = Some(reject_set_subpicture_image);
        (*vt).vaSetSubpictureChromakey = Some(reject_set_subpicture_chromakey);
        (*vt).vaSetSubpictureGlobalAlpha = Some(reject_set_subpicture_global_alpha);
        (*vt).vaAssociateSubpicture = Some(reject_associate_subpicture);
        (*vt).vaDeassociateSubpicture = Some(reject_deassociate_subpicture);
        (*vt).vaLockSurface = Some(reject_lock_surface);
        (*vt).vaUnlockSurface = Some(reject_unlock_surface);
        (*vt).vaCreateMFContext = Some(reject_create_mf_context);
        (*vt).vaMFAddContext = Some(reject_mf_add_context);
        (*vt).vaMFReleaseContext = Some(reject_mf_release_context);
        (*vt).vaMFSubmit = Some(reject_mf_submit);
        (*vt).vaCreateBuffer2 = Some(reject_create_buffer2);
        (*vt).vaQueryProcessingRate = Some(reject_query_processing_rate);
        (*vt).vaCopy = Some(reject_copy);

        (*vt).vaTerminate = Some(terminate);
        (*vt).vaQueryConfigProfiles = Some(query_config_profiles);
        (*vt).vaQueryConfigEntrypoints = Some(query_config_entrypoints);
        (*vt).vaGetConfigAttributes = Some(get_config_attributes);
        (*vt).vaCreateConfig = Some(create_config);
        (*vt).vaDestroyConfig = Some(destroy_config);
        (*vt).vaQueryConfigAttributes = Some(query_config_attributes);
        (*vt).vaGetSurfaceAttributes = Some(get_surface_attributes);
        (*vt).vaQuerySurfaceAttributes = Some(query_surface_attributes);
        (*vt).vaQueryImageFormats = Some(query_image_formats);
        (*vt).vaCreateImage = Some(create_image);
        (*vt).vaDestroyImage = Some(destroy_image);
        (*vt).vaGetImage = Some(get_image);
        (*vt).vaDeriveImage = Some(derive_image);
        (*vt).vaQueryDisplayAttributes = Some(query_display_attributes);
        (*vt).vaGetDisplayAttributes = Some(get_display_attributes);
        (*vt).vaSetDisplayAttributes = Some(get_display_attributes);
        (*vt).vaQuerySubpictureFormats = Some(query_subpicture_formats);
        (*vt).vaCreateSurfaces = Some(create_surfaces);
        (*vt).vaCreateSurfaces2 = Some(create_surfaces2);
        (*vt).vaDestroySurfaces = Some(destroy_surfaces);
        (*vt).vaQuerySurfaceStatus = Some(query_surface_status);
        (*vt).vaQuerySurfaceError = Some(query_surface_error);
        (*vt).vaExportSurfaceHandle = Some(export_surface_handle);
        (*vt).vaCreateContext = Some(create_context);
        (*vt).vaDestroyContext = Some(destroy_context);
        (*vt).vaCreateBuffer = Some(create_buffer);
        (*vt).vaDestroyBuffer = Some(destroy_buffer);
        (*vt).vaBufferSetNumElements = Some(buffer_set_num_elements);
        (*vt).vaMapBuffer = Some(map_buffer);
        (*vt).vaMapBuffer2 = Some(map_buffer2);
        (*vt).vaUnmapBuffer = Some(unmap_buffer);
        (*vt).vaBufferInfo = Some(buffer_info);
        (*vt).vaAcquireBufferHandle = Some(acquire_buffer_handle);
        (*vt).vaReleaseBufferHandle = Some(release_buffer_handle);
        (*vt).vaBeginPicture = Some(begin_picture);
        (*vt).vaRenderPicture = Some(render_picture);
        (*vt).vaEndPicture = Some(end_picture);
        (*vt).vaSyncSurface = Some(sync_surface);
        (*vt).vaSyncSurface2 = Some(sync_surface2);
        (*vt).vaSyncBuffer = Some(sync_buffer);
    }
}

pub(crate) unsafe fn install_vpp_stubs(ctx: VADriverContextP) {
    if ctx.is_null() {
        return;
    }
    let vpp = unsafe { (*ctx).vtable_vpp };
    if vpp.is_null() {
        return;
    }
    unsafe {
        (*vpp).version = VA_DRIVER_VTABLE_VPP_VERSION;
        (*vpp).vaQueryVideoProcFilters = Some(reject_query_video_proc_filters);
        (*vpp).vaQueryVideoProcFilterCaps = Some(reject_query_video_proc_filter_caps);
        (*vpp).vaQueryVideoProcPipelineCaps = Some(reject_query_video_proc_pipeline_caps);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_every_core_callback() {
        // The generated table contains only function-pointer Options and
        // integer reserved storage, so an all-zero value represents the
        // libva-allocated, not-yet-initialized table used by this test.
        let mut vtable: VADriverVTable = unsafe { std::mem::zeroed() };
        unsafe { install_vtable(&mut vtable) };

        macro_rules! assert_installed {
            ($($field:ident),+ $(,)?) => {
                $(assert!(vtable.$field.is_some(), stringify!($field));)+
            };
        }

        assert_installed!(
            vaTerminate,
            vaQueryConfigProfiles,
            vaQueryConfigEntrypoints,
            vaGetConfigAttributes,
            vaCreateConfig,
            vaDestroyConfig,
            vaQueryConfigAttributes,
            vaCreateSurfaces,
            vaDestroySurfaces,
            vaCreateContext,
            vaDestroyContext,
            vaCreateBuffer,
            vaBufferSetNumElements,
            vaMapBuffer,
            vaUnmapBuffer,
            vaDestroyBuffer,
            vaBeginPicture,
            vaRenderPicture,
            vaEndPicture,
            vaSyncSurface,
            vaQuerySurfaceStatus,
            vaQuerySurfaceError,
            vaPutSurface,
            vaQueryImageFormats,
            vaCreateImage,
            vaDeriveImage,
            vaDestroyImage,
            vaSetImagePalette,
            vaGetImage,
            vaPutImage,
            vaQuerySubpictureFormats,
            vaCreateSubpicture,
            vaDestroySubpicture,
            vaSetSubpictureImage,
            vaSetSubpictureChromakey,
            vaSetSubpictureGlobalAlpha,
            vaAssociateSubpicture,
            vaDeassociateSubpicture,
            vaQueryDisplayAttributes,
            vaGetDisplayAttributes,
            vaSetDisplayAttributes,
            vaBufferInfo,
            vaLockSurface,
            vaUnlockSurface,
            vaGetSurfaceAttributes,
            vaCreateSurfaces2,
            vaQuerySurfaceAttributes,
            vaAcquireBufferHandle,
            vaReleaseBufferHandle,
            vaCreateMFContext,
            vaMFAddContext,
            vaMFReleaseContext,
            vaMFSubmit,
            vaCreateBuffer2,
            vaQueryProcessingRate,
            vaExportSurfaceHandle,
            vaSyncSurface2,
            vaSyncBuffer,
            vaCopy,
            vaMapBuffer2,
        );
    }

    #[test]
    fn optional_cpu_display_callbacks_reject_cleanly() {
        let unimplemented = VA_STATUS_ERROR_UNIMPLEMENTED as VAStatus;
        assert_eq!(
            unsafe {
                reject_put_image(
                    std::ptr::null_mut(),
                    VA_INVALID_ID,
                    VA_INVALID_ID,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                )
            },
            unimplemented
        );
        assert_eq!(
            unsafe {
                reject_put_surface(
                    std::ptr::null_mut(),
                    VA_INVALID_ID,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    0,
                    std::ptr::null_mut(),
                    0,
                    0,
                )
            },
            unimplemented
        );
        assert_eq!(
            unsafe {
                reject_lock_surface(
                    std::ptr::null_mut(),
                    VA_INVALID_ID,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            unimplemented
        );
        assert_eq!(
            unsafe { reject_unlock_surface(std::ptr::null_mut(), VA_INVALID_ID) },
            unimplemented
        );
    }
}
