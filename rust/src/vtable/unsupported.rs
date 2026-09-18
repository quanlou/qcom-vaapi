//! Exact-signature unsupported VA callbacks.
//!
//! Unsupported entries still need their real C ABI signatures. A single generic
//! function pointer would be undefined behavior; these stubs reject cleanly
//! while keeping the vtable installer focused on callback wiring.

use crate::bindings::*;
use crate::err;
use std::ffi::c_void;
use std::os::raw::{c_int, c_short, c_uchar, c_uint, c_ushort};

macro_rules! unsupported_callback {
    ($name:ident, ($($arg:ident: $ty:ty),* $(,)?)) => {
        pub(super) unsafe extern "C" fn $name($($arg: $ty),*) -> VAStatus {
            err(VA_STATUS_ERROR_UNIMPLEMENTED)
        }
    };
}

unsupported_callback!(reject_put_surface, (
    _ctx: VADriverContextP,
    _surface: VASurfaceID,
    _draw: *mut c_void,
    _srcx: c_short,
    _srcy: c_short,
    _srcw: c_ushort,
    _srch: c_ushort,
    _destx: c_short,
    _desty: c_short,
    _destw: c_ushort,
    _desth: c_ushort,
    _cliprects: *mut VARectangle,
    _number_cliprects: c_uint,
    _flags: c_uint,
));
unsupported_callback!(reject_set_image_palette, (
    _ctx: VADriverContextP,
    _image: VAImageID,
    _palette: *mut c_uchar,
));
unsupported_callback!(reject_put_image, (
    _ctx: VADriverContextP,
    _surface: VASurfaceID,
    _image: VAImageID,
    _src_x: c_int,
    _src_y: c_int,
    _src_width: c_uint,
    _src_height: c_uint,
    _dest_x: c_int,
    _dest_y: c_int,
    _dest_width: c_uint,
    _dest_height: c_uint,
));
unsupported_callback!(reject_create_subpicture, (
    _ctx: VADriverContextP,
    _image: VAImageID,
    _subpicture: *mut VASubpictureID,
));
unsupported_callback!(reject_destroy_subpicture, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
));
unsupported_callback!(reject_set_subpicture_image, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
    _image: VAImageID,
));
unsupported_callback!(reject_set_subpicture_chromakey, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
    _chromakey_min: c_uint,
    _chromakey_max: c_uint,
    _chromakey_mask: c_uint,
));
unsupported_callback!(reject_set_subpicture_global_alpha, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
    _global_alpha: f32,
));
unsupported_callback!(reject_associate_subpicture, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
    _target_surfaces: *mut VASurfaceID,
    _num_surfaces: c_int,
    _src_x: c_short,
    _src_y: c_short,
    _src_width: c_ushort,
    _src_height: c_ushort,
    _dest_x: c_short,
    _dest_y: c_short,
    _dest_width: c_ushort,
    _dest_height: c_ushort,
    _flags: c_uint,
));
unsupported_callback!(reject_deassociate_subpicture, (
    _ctx: VADriverContextP,
    _subpicture: VASubpictureID,
    _target_surfaces: *mut VASurfaceID,
    _num_surfaces: c_int,
));
unsupported_callback!(reject_lock_surface, (
    _ctx: VADriverContextP,
    _surface: VASurfaceID,
    _fourcc: *mut c_uint,
    _luma_stride: *mut c_uint,
    _chroma_u_stride: *mut c_uint,
    _chroma_v_stride: *mut c_uint,
    _luma_offset: *mut c_uint,
    _chroma_u_offset: *mut c_uint,
    _chroma_v_offset: *mut c_uint,
    _buffer_name: *mut c_uint,
    _buffer: *mut *mut c_void,
));
unsupported_callback!(reject_unlock_surface, (
    _ctx: VADriverContextP,
    _surface: VASurfaceID,
));
unsupported_callback!(reject_create_mf_context, (
    _ctx: VADriverContextP,
    _mf_context: *mut VAMFContextID,
));
unsupported_callback!(reject_mf_add_context, (
    _ctx: VADriverContextP,
    _mf_context: VAMFContextID,
    _context: VAContextID,
));
unsupported_callback!(reject_mf_release_context, (
    _ctx: VADriverContextP,
    _mf_context: VAMFContextID,
    _context: VAContextID,
));
unsupported_callback!(reject_mf_submit, (
    _ctx: VADriverContextP,
    _mf_context: VAMFContextID,
    _contexts: *mut VAContextID,
    _num_contexts: c_int,
));
unsupported_callback!(reject_create_buffer2, (
    _ctx: VADriverContextP,
    _context: VAContextID,
    _type: VABufferType,
    _width: c_uint,
    _height: c_uint,
    _unit_size: *mut c_uint,
    _pitch: *mut c_uint,
    _buf_id: *mut VABufferID,
));
unsupported_callback!(reject_query_processing_rate, (
    _ctx: VADriverContextP,
    _config_id: VAConfigID,
    _proc_buf: *mut VAProcessingRateParameter,
    _processing_rate: *mut c_uint,
));
unsupported_callback!(reject_copy, (
    _ctx: VADriverContextP,
    _dst: *mut VACopyObject,
    _src: *mut VACopyObject,
    _option: VACopyOption,
));
unsupported_callback!(reject_query_video_proc_filters, (
    _ctx: VADriverContextP,
    _context: VAContextID,
    _filters: *mut VAProcFilterType,
    _num_filters: *mut c_uint,
));
unsupported_callback!(reject_query_video_proc_filter_caps, (
    _ctx: VADriverContextP,
    _context: VAContextID,
    _type: VAProcFilterType,
    _filter_caps: *mut c_void,
    _num_filter_caps: *mut c_uint,
));
unsupported_callback!(reject_query_video_proc_pipeline_caps, (
    _ctx: VADriverContextP,
    _context: VAContextID,
    _filters: *mut VABufferID,
    _num_filters: c_uint,
    _pipeline_caps: *mut VAProcPipelineCaps,
));
