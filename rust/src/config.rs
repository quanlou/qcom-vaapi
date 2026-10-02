//! VA configuration and capability callbacks.
//!
//! These entrypoints describe the profiles and formats the Rust V4L2 M2M
//! backend can actually service. The reported profile table is gated at
//! driver init on what the V4L2 decoder node's OUTPUT queue exposes (a
//! read-only VIDIOC_ENUM_FMT walk): a codec V4L2 does not enumerate never
//! appears in `vaQueryConfigProfiles`, and any config/entrypoint request for
//! it fails cleanly with `VA_STATUS_ERROR_UNSUPPORTED_PROFILE`. Keeping
//! capability negotiation separate from the vtable and lifecycle wiring makes
//! unsupported combinations explicit at the FFI boundary.

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use crate::state::{
    Config, DRV_ID_BASE_CONFIG, DRV_MAX_ATTRIBUTE_LIST, DRV_MAX_DIM, SUPPORTED_PROFILES,
};
use crate::v4l2::{
    V4L2_PIX_FMT_AV1, V4L2_PIX_FMT_H264, V4L2_PIX_FMT_HEVC, V4L2_PIX_FMT_P010, V4L2_PIX_FMT_VP9,
};
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::ptr;
use std::slice;

/// Coded formats mapped to the VA profiles they allow this driver to report,
/// in fixed advertisement order. A codec's profiles are advertised only when
/// the V4L2 OUTPUT queue actually enumerated its coded format.
///
/// Only profiles with a complete userspace translation path belong here. Kernel
/// format enumeration is necessary capability evidence, but by itself is not
/// enough to promise a working VA profile.
///
/// Pure mapping from an enumerated OUTPUT fourcc list to the profile table
/// this driver reports. Fixed codec order (H.264, HEVC, VP9, AV1); codecs
/// whose V4L2 format was not enumerated are absent from the result. An
/// enumeration with no recognized format reports no supported profiles.
fn advertised_profiles_from(
    output_fourccs: &[u32],
    capture_fourccs: &[u32],
    experimental_av1: bool,
) -> Vec<VAProfile> {
    let mut profiles: Vec<VAProfile> = Vec::new();
    if output_fourccs.contains(&V4L2_PIX_FMT_H264) {
        profiles.extend_from_slice(&SUPPORTED_PROFILES);
    }
    if output_fourccs.contains(&V4L2_PIX_FMT_HEVC) {
        profiles.push(VAProfile::VAProfileHEVCMain);
        if capture_fourccs.contains(&V4L2_PIX_FMT_P010) {
            profiles.push(VAProfile::VAProfileHEVCMain10);
        }
    }
    if output_fourccs.contains(&V4L2_PIX_FMT_VP9) {
        profiles.push(VAProfile::VAProfileVP9Profile0);
    }
    // AV1 keyframes match, but inter-frame parity remains unresolved.
    if experimental_av1 && output_fourccs.contains(&V4L2_PIX_FMT_AV1) {
        profiles.push(VAProfile::VAProfileAV1Profile0);
    }
    profiles
}

/// Discover capabilities for one driver initialization. No process-global
/// cache: independent displays and later device overrides cannot inherit a
/// table from another node. Enumeration failure advertises no decode support.
pub(crate) fn advertised_profiles() -> Vec<VAProfile> {
    advertised_profiles_from(
        &crate::v4l2::enumerate_output_fourccs(),
        &crate::v4l2::enumerate_capture_fourccs(),
        std::env::var("V4L2_VA_EXPERIMENTAL_AV1").is_ok_and(|value| value == "1"),
    )
}

fn validate_create_attributes(profile: VAProfile, attributes: &[VAConfigAttrib]) -> VAStatus {
    let required_format = DecodedFormat::from_profile(profile);
    for attribute in attributes {
        let status = match attribute.type_ {
            VAConfigAttribType::VAConfigAttribRTFormat => {
                if attribute.value == required_format.rt_format() {
                    ok()
                } else {
                    err(VA_STATUS_ERROR_UNSUPPORTED_RT_FORMAT)
                }
            }
            VAConfigAttribType::VAConfigAttribDecSliceMode => {
                if attribute.value == VA_DEC_SLICE_MODE_NORMAL {
                    ok()
                } else {
                    err(VA_STATUS_ERROR_ATTR_NOT_SUPPORTED)
                }
            }
            VAConfigAttribType::VAConfigAttribDecProcessing => {
                if attribute.value == VA_DEC_PROCESSING_NONE {
                    ok()
                } else {
                    err(VA_STATUS_ERROR_ATTR_NOT_SUPPORTED)
                }
            }
            _ => err(VA_STATUS_ERROR_ATTR_NOT_SUPPORTED),
        };
        if status != ok() {
            return status;
        }
    }
    ok()
}

pub(crate) unsafe extern "C" fn query_config_profiles(
    ctx: VADriverContextP,
    profile_list: *mut VAProfile,
    num_profiles: *mut c_int,
) -> VAStatus {
    if profile_list.is_null() || num_profiles.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let profiles = &state.profiles;
    for (i, profile) in profiles.iter().enumerate() {
        unsafe { *profile_list.add(i) = *profile };
    }
    unsafe { *num_profiles = profiles.len() as c_int };
    ok()
}

pub(crate) unsafe extern "C" fn query_config_entrypoints(
    ctx: VADriverContextP,
    profile: VAProfile,
    entrypoint_list: *mut VAEntrypoint,
    num_entrypoints: *mut c_int,
) -> VAStatus {
    if entrypoint_list.is_null() || num_entrypoints.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if !state.profiles.contains(&profile) {
        unsafe { *num_entrypoints = 0 };
        return err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE);
    }
    unsafe {
        *entrypoint_list = VAEntrypoint::VAEntrypointVLD;
        *num_entrypoints = 1;
    }
    ok()
}

pub(crate) unsafe extern "C" fn get_config_attributes(
    ctx: VADriverContextP,
    profile: VAProfile,
    entrypoint: VAEntrypoint,
    attrib_list: *mut VAConfigAttrib,
    num_attribs: c_int,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if !state.profiles.contains(&profile) {
        return err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE);
    }
    if entrypoint != VAEntrypoint::VAEntrypointVLD {
        return err(VA_STATUS_ERROR_UNSUPPORTED_ENTRYPOINT);
    }
    if num_attribs < 0 || (num_attribs > 0 && attrib_list.is_null()) {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_attribs as usize > DRV_MAX_ATTRIBUTE_LIST {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    for i in 0..num_attribs as isize {
        let attr = unsafe { &mut *attrib_list.offset(i) };
        attr.value = match attr.type_ {
            VAConfigAttribType::VAConfigAttribRTFormat => {
                DecodedFormat::from_profile(profile).rt_format()
            }
            VAConfigAttribType::VAConfigAttribMaxPictureWidth => DRV_MAX_DIM as u32,
            VAConfigAttribType::VAConfigAttribMaxPictureHeight => DRV_MAX_DIM as u32,
            _ => VA_ATTRIB_NOT_SUPPORTED,
        };
    }
    ok()
}

pub(crate) unsafe extern "C" fn create_config(
    ctx: VADriverContextP,
    profile: VAProfile,
    entrypoint: VAEntrypoint,
    attrib_list: *mut VAConfigAttrib,
    num_attribs: c_int,
    config_id: *mut VAConfigID,
) -> VAStatus {
    if config_id.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_attribs < 0 || (num_attribs > 0 && attrib_list.is_null()) {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if num_attribs as usize > DRV_MAX_ATTRIBUTE_LIST {
        return err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED);
    }
    let check = unsafe { get_config_attributes(ctx, profile, entrypoint, ptr::null_mut(), 0) };
    if check != ok() {
        return check;
    }
    let attributes = if num_attribs > 0 {
        unsafe { slice::from_raw_parts(attrib_list, num_attribs as usize) }
    } else {
        &[]
    };
    let check = validate_create_attributes(profile, attributes);
    if check != ok() {
        return check;
    }
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    let attribs = attributes.iter().take(16).copied().collect();
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if let Some((idx, slot)) = guard
        .configs
        .iter_mut()
        .enumerate()
        .find(|(_, v)| v.is_none())
    {
        *slot = Some(Config {
            profile,
            entrypoint,
            attribs,
            format: DecodedFormat::from_profile(profile),
        });
        unsafe { *config_id = DRV_ID_BASE_CONFIG + idx as u32 };
        ok()
    } else {
        err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED)
    }
}

pub(crate) unsafe extern "C" fn destroy_config(
    ctx: VADriverContextP,
    config_id: VAConfigID,
) -> VAStatus {
    let Some(state) = (unsafe { state_from_ctx(ctx) }) else {
        return err(VA_STATUS_ERROR_INVALID_DISPLAY);
    };
    if config_id < DRV_ID_BASE_CONFIG {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    }
    let idx = (config_id - DRV_ID_BASE_CONFIG) as usize;
    let mut guard = match state.lock.lock() {
        Ok(g) => g,
        Err(_) => return err(VA_STATUS_ERROR_OPERATION_FAILED),
    };
    if idx >= guard.configs.len() || guard.configs[idx].is_none() {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    }
    if guard
        .contexts
        .iter()
        .flatten()
        .any(|context| context.config_id == config_id)
    {
        return err(VA_STATUS_ERROR_OPERATION_FAILED);
    }
    guard.configs[idx] = None;
    ok()
}

pub(crate) unsafe extern "C" fn query_config_attributes(
    ctx: VADriverContextP,
    config_id: VAConfigID,
    profile: *mut VAProfile,
    entrypoint: *mut VAEntrypoint,
    attrib_list: *mut VAConfigAttrib,
    num_attribs: *mut c_int,
) -> VAStatus {
    if profile.is_null() || entrypoint.is_null() || attrib_list.is_null() || num_attribs.is_null() {
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
    let Some(Some(cfg)) = guard.configs.get(idx) else {
        return err(VA_STATUS_ERROR_INVALID_CONFIG);
    };
    unsafe {
        *profile = cfg.profile;
        *entrypoint = cfg.entrypoint;
        (*attrib_list).type_ = VAConfigAttribType::VAConfigAttribRTFormat;
        (*attrib_list).value = cfg.format.rt_format();
        *num_attribs = 1;
    }
    ok()
}

pub(crate) unsafe extern "C" fn query_display_attributes(
    _ctx: VADriverContextP,
    _attr_list: *mut VADisplayAttribute,
    num_attributes: *mut c_int,
) -> VAStatus {
    if !num_attributes.is_null() {
        unsafe { *num_attributes = 0 };
    }
    ok()
}

pub(crate) unsafe extern "C" fn get_display_attributes(
    _ctx: VADriverContextP,
    _attr_list: *mut VADisplayAttribute,
    _num_attributes: c_int,
) -> VAStatus {
    ok()
}

pub(crate) unsafe extern "C" fn query_subpicture_formats(
    _ctx: VADriverContextP,
    _format_list: *mut VAImageFormat,
    flags: *mut u32,
    num_formats: *mut u32,
) -> VAStatus {
    if !flags.is_null() {
        unsafe { *flags = 0 };
    }
    if !num_formats.is_null() {
        unsafe { *num_formats = 0 };
    }
    ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attribute(type_: VAConfigAttribType, value: u32) -> VAConfigAttrib {
        VAConfigAttrib { type_, value }
    }

    #[test]
    fn capabilities_are_isolated_between_driver_instances() {
        use crate::state::DriverBox;
        use std::ffi::c_void;
        let mut first = Box::new(DriverBox::new());
        first.profiles = vec![VAProfile::VAProfileH264Main];
        let mut second = Box::new(DriverBox::new());
        second.profiles = vec![VAProfile::VAProfileVP9Profile0];
        for driver in [&mut first, &mut second] {
            let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
            ctx.pDriverData = (&mut **driver as *mut DriverBox).cast::<c_void>();
            let mut output = [VAProfile::VAProfileNone; 8];
            let mut count = -1;
            assert_eq!(
                unsafe { query_config_profiles(&mut ctx, output.as_mut_ptr(), &mut count) },
                ok()
            );
            assert_eq!(&output[..count as usize], driver.profiles.as_slice());
            let mut entrypoint = VAEntrypoint::VAEntrypointVLD;
            assert_eq!(
                unsafe {
                    query_config_entrypoints(
                        &mut ctx,
                        VAProfile::VAProfileHEVCMain,
                        &mut entrypoint,
                        &mut count,
                    )
                },
                err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE)
            );
            assert_eq!(count, 0);
            assert_eq!(
                unsafe {
                    get_config_attributes(
                        &mut ctx,
                        driver.profiles[0],
                        VAEntrypoint::VAEntrypointVLD,
                        std::ptr::null_mut(),
                        1,
                    )
                },
                err(VA_STATUS_ERROR_INVALID_PARAMETER)
            );
        }
        first.profiles.clear();
        let mut ctx: VADriverContext = unsafe { std::mem::zeroed() };
        ctx.pDriverData = (&mut *first as *mut DriverBox).cast::<c_void>();
        let mut output = VAProfile::VAProfileNone;
        let mut count = -1;
        assert_eq!(
            unsafe { query_config_profiles(&mut ctx, &mut output, &mut count) },
            ok()
        );
        assert_eq!(count, 0);
    }

    #[test]
    fn accepts_only_supported_decode_configuration_attributes() {
        assert_eq!(
            validate_create_attributes(VAProfile::VAProfileH264Main, &[]),
            ok()
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribRTFormat,
                    VA_RT_FORMAT_YUV420,
                )],
            ),
            ok()
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileHEVCMain10,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribRTFormat,
                    VA_RT_FORMAT_YUV420_10,
                )],
            ),
            ok()
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribDecSliceMode,
                    VA_DEC_SLICE_MODE_NORMAL,
                )],
            ),
            ok()
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribDecProcessing,
                    VA_DEC_PROCESSING_NONE,
                )],
            ),
            ok()
        );
    }

    #[test]
    fn rejects_unsupported_decode_configuration_attributes() {
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribRTFormat,
                    VA_RT_FORMAT_YUV420_10,
                )],
            ),
            VA_STATUS_ERROR_UNSUPPORTED_RT_FORMAT as VAStatus
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileHEVCMain10,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribRTFormat,
                    VA_RT_FORMAT_YUV420,
                )],
            ),
            VA_STATUS_ERROR_UNSUPPORTED_RT_FORMAT as VAStatus
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(
                    VAConfigAttribType::VAConfigAttribDecSliceMode,
                    VA_DEC_SLICE_MODE_BASE,
                )],
            ),
            VA_STATUS_ERROR_ATTR_NOT_SUPPORTED as VAStatus
        );
        assert_eq!(
            validate_create_attributes(
                VAProfile::VAProfileH264Main,
                &[attribute(VAConfigAttribType::VAConfigAttribRateControl, 0)],
            ),
            VA_STATUS_ERROR_ATTR_NOT_SUPPORTED as VAStatus
        );
    }

    #[test]
    fn codec_table_reports_every_profile_for_enumerated_formats() {
        let all = [
            V4L2_PIX_FMT_H264,
            V4L2_PIX_FMT_HEVC,
            V4L2_PIX_FMT_VP9,
            crate::v4l2::V4L2_PIX_FMT_AV1,
        ];
        let profiles = advertised_profiles_from(&all, &[V4L2_PIX_FMT_P010], false);
        assert_eq!(profiles.len(), 6);
        // Validated HEVC and VP9 follow H.264; AV1 needs explicit opt-in.
        assert_eq!(&profiles[..3], &SUPPORTED_PROFILES);
        assert_eq!(profiles[3], VAProfile::VAProfileHEVCMain);
        assert_eq!(profiles[4], VAProfile::VAProfileHEVCMain10);
        assert_eq!(profiles[5], VAProfile::VAProfileVP9Profile0);
        assert!(!profiles.contains(&VAProfile::VAProfileAV1Profile0));

        let no_p010 = advertised_profiles_from(&all, &[], false);
        assert!(!no_p010.contains(&VAProfile::VAProfileHEVCMain10));
    }

    #[test]
    fn experimental_av1_requires_opt_in_and_kernel_support() {
        let formats = [V4L2_PIX_FMT_H264, V4L2_PIX_FMT_AV1];
        assert!(
            !advertised_profiles_from(&formats, &[], false)
                .contains(&VAProfile::VAProfileAV1Profile0)
        );
        assert!(
            advertised_profiles_from(&formats, &[], true)
                .contains(&VAProfile::VAProfileAV1Profile0)
        );
        assert!(
            !advertised_profiles_from(&[V4L2_PIX_FMT_H264], &[], true)
                .contains(&VAProfile::VAProfileAV1Profile0)
        );
    }

    #[test]
    fn codec_table_advertises_only_v4l2_enumerated_codecs() {
        // Only VP9 exposed: no H.264, no HEVC, no AV1, and 10-bit VP9 stays
        // out of scope.
        let vp9_only = advertised_profiles_from(&[V4L2_PIX_FMT_VP9], &[V4L2_PIX_FMT_P010], false);
        assert_eq!(vp9_only, &[VAProfile::VAProfileVP9Profile0]);
        assert!(!vp9_only.contains(&VAProfile::VAProfileH264Main));
        assert!(!vp9_only.contains(&VAProfile::VAProfileHEVCMain));
        assert!(!vp9_only.contains(&VAProfile::VAProfileAV1Profile0));
        assert!(!vp9_only.contains(&VAProfile::VAProfileVP9Profile2));

        // Failed or unrecognized enumeration must never invent support.
        assert_eq!(
            advertised_profiles_from(&[0x1234_5678], &[V4L2_PIX_FMT_P010], false),
            Vec::<VAProfile>::new()
        );
        assert_eq!(
            advertised_profiles_from(&[], &[], false),
            Vec::<VAProfile>::new()
        );
    }

    #[test]
    fn supported_profile_rejects_codecs_outside_the_advertised_table() {
        // Codecs no V4L2 enumeration can ever advertise are rejected for the
        // full table and the device-gated table alike; they must keep failing
        // with VA_STATUS_ERROR_UNSUPPORTED_PROFILE through the entrypoint,
        // config-attribute, and create-config paths.
        let never = [
            VAProfile::VAProfileMPEG2Main,
            VAProfile::VAProfileVP8Version0_3,
            VAProfile::VAProfileVC1Advanced,
            VAProfile::VAProfileH264Baseline,
        ];
        let all = [
            V4L2_PIX_FMT_H264,
            V4L2_PIX_FMT_HEVC,
            V4L2_PIX_FMT_VP9,
            crate::v4l2::V4L2_PIX_FMT_AV1,
        ];
        let table = advertised_profiles_from(&all, &[V4L2_PIX_FMT_P010], false);
        for profile in never {
            assert!(!table.contains(&profile));
        }
        // Every advertised profile is a member of the codec table, so the
        // UNSUPPORTED_PROFILE rejection only ever fires for unadvertised
        // values.
        for profile in &table {
            assert!(matches!(
                profile,
                VAProfile::VAProfileH264ConstrainedBaseline
                    | VAProfile::VAProfileH264Main
                    | VAProfile::VAProfileH264High
                    | VAProfile::VAProfileHEVCMain
                    | VAProfile::VAProfileHEVCMain10
                    | VAProfile::VAProfileVP9Profile0
                    | VAProfile::VAProfileAV1Profile0
            ));
            assert!(table.contains(profile));
        }
    }
}
