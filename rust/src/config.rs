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
use crate::state::{
    Config, DRV_ID_BASE_CONFIG, DRV_MAX_ATTRIBUTE_LIST, DRV_MAX_DIM, SUPPORTED_PROFILES,
};
use crate::v4l2::{V4L2_PIX_FMT_H264, V4L2_PIX_FMT_HEVC, V4L2_PIX_FMT_VP9};
use crate::{err, ok, state_from_ctx};
use std::ffi::c_int;
use std::ptr;
use std::slice;
use std::sync::OnceLock;

/// Coded formats mapped to the VA profiles they allow this driver to report,
/// in fixed advertisement order. A codec's profiles are advertised only when
/// the V4L2 OUTPUT queue actually enumerated its coded format.
///
/// Only profiles with a complete userspace translation path belong here.
/// HEVC Main10 remains hidden until P010 surfaces are implemented, and AV1
/// remains hidden until VA tile buffers can be rebuilt into complete OBUs.
/// Kernel format enumeration is necessary capability evidence, but by itself
/// is not enough to promise a working VA profile.
const CODEC_PROFILES: &[(u32, &[VAProfile])] = &[
    (V4L2_PIX_FMT_H264, &SUPPORTED_PROFILES),
    (V4L2_PIX_FMT_HEVC, &[VAProfile::VAProfileHEVCMain]),
    (V4L2_PIX_FMT_VP9, &[VAProfile::VAProfileVP9Profile0]),
];

static ADVERTISED_PROFILES: OnceLock<&'static [VAProfile]> = OnceLock::new();

/// Pure mapping from an enumerated OUTPUT fourcc list to the profile table
/// this driver reports. Fixed codec order (H.264, HEVC, VP9, AV1); codecs
/// whose V4L2 format was not enumerated are absent from the result. An
/// enumeration with no recognized format falls back to the historical
/// H.264-only table so capability reporting never regresses below the
/// production decode path.
fn advertised_profiles_from(fourccs: &[u32]) -> &'static [VAProfile] {
    let mut profiles: Vec<VAProfile> = Vec::new();
    for (fourcc, codec_profiles) in CODEC_PROFILES {
        if fourccs.contains(fourcc) {
            profiles.extend_from_slice(codec_profiles);
        }
    }
    if profiles.is_empty() {
        return &SUPPORTED_PROFILES;
    }
    Box::leak(profiles.into_boxed_slice())
}

/// The profile table libva sees, gated once per process on what the V4L2
/// decoder node actually exposes (read-only enumeration; no decode session).
pub(crate) fn advertised_profiles() -> &'static [VAProfile] {
    ADVERTISED_PROFILES
        .get_or_init(|| advertised_profiles_from(&crate::v4l2::enumerate_output_fourccs()))
}

pub(crate) fn supported_profile(profile: VAProfile) -> bool {
    advertised_profiles().contains(&profile)
}

fn validate_create_attributes(attributes: &[VAConfigAttrib]) -> VAStatus {
    for attribute in attributes {
        let status = match attribute.type_ {
            VAConfigAttribType::VAConfigAttribRTFormat => {
                if attribute.value == VA_RT_FORMAT_YUV420 {
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
    _ctx: VADriverContextP,
    profile_list: *mut VAProfile,
    num_profiles: *mut c_int,
) -> VAStatus {
    if profile_list.is_null() || num_profiles.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    let profiles = advertised_profiles();
    for (i, profile) in profiles.iter().enumerate() {
        unsafe { *profile_list.add(i) = *profile };
    }
    unsafe { *num_profiles = profiles.len() as c_int };
    ok()
}

pub(crate) unsafe extern "C" fn query_config_entrypoints(
    _ctx: VADriverContextP,
    profile: VAProfile,
    entrypoint_list: *mut VAEntrypoint,
    num_entrypoints: *mut c_int,
) -> VAStatus {
    if entrypoint_list.is_null() || num_entrypoints.is_null() {
        return err(VA_STATUS_ERROR_INVALID_PARAMETER);
    }
    if !supported_profile(profile) {
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
    _ctx: VADriverContextP,
    profile: VAProfile,
    entrypoint: VAEntrypoint,
    attrib_list: *mut VAConfigAttrib,
    num_attribs: c_int,
) -> VAStatus {
    if !supported_profile(profile) {
        return err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE);
    }
    if entrypoint != VAEntrypoint::VAEntrypointVLD {
        return err(VA_STATUS_ERROR_UNSUPPORTED_ENTRYPOINT);
    }
    if attrib_list.is_null() || num_attribs <= 0 {
        return ok();
    }
    for i in 0..num_attribs as isize {
        let attr = unsafe { &mut *attrib_list.offset(i) };
        attr.value = match attr.type_ {
            VAConfigAttribType::VAConfigAttribRTFormat => VA_RT_FORMAT_YUV420,
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
    let check = validate_create_attributes(attributes);
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
        (*attrib_list).value = VA_RT_FORMAT_YUV420;
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
    fn accepts_only_supported_decode_configuration_attributes() {
        assert_eq!(validate_create_attributes(&[]), ok());
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribRTFormat,
                VA_RT_FORMAT_YUV420,
            )]),
            ok()
        );
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribDecSliceMode,
                VA_DEC_SLICE_MODE_NORMAL,
            )]),
            ok()
        );
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribDecProcessing,
                VA_DEC_PROCESSING_NONE,
            )]),
            ok()
        );
    }

    #[test]
    fn rejects_unsupported_decode_configuration_attributes() {
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribRTFormat,
                VA_RT_FORMAT_YUV420_10,
            )]),
            VA_STATUS_ERROR_UNSUPPORTED_RT_FORMAT as VAStatus
        );
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribDecSliceMode,
                VA_DEC_SLICE_MODE_BASE,
            )]),
            VA_STATUS_ERROR_ATTR_NOT_SUPPORTED as VAStatus
        );
        assert_eq!(
            validate_create_attributes(&[attribute(
                VAConfigAttribType::VAConfigAttribRateControl,
                0,
            )]),
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
        let profiles = advertised_profiles_from(&all);
        assert_eq!(profiles.len(), 5);
        // H.264 stays first and unchanged; validated HEVC and VP9 follow.
        assert_eq!(&profiles[..3], &SUPPORTED_PROFILES);
        assert_eq!(profiles[3], VAProfile::VAProfileHEVCMain);
        assert_eq!(profiles[4], VAProfile::VAProfileVP9Profile0);
        assert!(!profiles.contains(&VAProfile::VAProfileHEVCMain10));
        assert!(!profiles.contains(&VAProfile::VAProfileAV1Profile0));
    }

    #[test]
    fn codec_table_advertises_only_v4l2_enumerated_codecs() {
        // Only VP9 exposed: no H.264, no HEVC, no AV1, and 10-bit VP9 stays
        // out of scope.
        let vp9_only = advertised_profiles_from(&[V4L2_PIX_FMT_VP9]);
        assert_eq!(vp9_only, &[VAProfile::VAProfileVP9Profile0]);
        assert!(!vp9_only.contains(&VAProfile::VAProfileH264Main));
        assert!(!vp9_only.contains(&VAProfile::VAProfileHEVCMain));
        assert!(!vp9_only.contains(&VAProfile::VAProfileAV1Profile0));
        assert!(!vp9_only.contains(&VAProfile::VAProfileVP9Profile2));

        // An enumeration with only unrecognized fourccs falls back to the
        // historical H.264-only table instead of advertising nothing.
        assert_eq!(
            advertised_profiles_from(&[0x1234_5678]),
            SUPPORTED_PROFILES.as_slice()
        );
        assert_eq!(advertised_profiles_from(&[]), SUPPORTED_PROFILES);
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
        let table = advertised_profiles_from(&all);
        for profile in never {
            assert!(!table.contains(&profile));
            assert!(!supported_profile(profile));
        }
        // Every advertised profile is a member of the codec table, so the
        // UNSUPPORTED_PROFILE rejection only ever fires for unadvertised
        // values.
        for profile in advertised_profiles() {
            assert!(
                CODEC_PROFILES
                    .iter()
                    .any(|(_, codec)| codec.contains(profile))
            );
            assert!(supported_profile(*profile));
        }
    }
}
