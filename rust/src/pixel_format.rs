//! Decoded surface format metadata.
//!
//! The V4L2 OUTPUT format selects the compressed codec. The CAPTURE format
//! selects how decoded frames are stored and exposed through VAImage and DRM
//! PRIME. Keeping that decoded format in one enum avoids codec-profile checks
//! leaking through image, export, and queue code.

use crate::bindings::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DecodedFormat {
    Nv12,
    P010,
}

impl DecodedFormat {
    pub(crate) fn from_profile(profile: VAProfile) -> Self {
        match profile {
            VAProfile::VAProfileHEVCMain10 => Self::P010,
            _ => Self::Nv12,
        }
    }

    pub(crate) fn from_rt_format(format: u32) -> Option<Self> {
        match format {
            VA_RT_FORMAT_YUV420 => Some(Self::Nv12),
            VA_RT_FORMAT_YUV420_10 => Some(Self::P010),
            _ => None,
        }
    }

    /// Inverse of `v4l2_fourcc`: recover the format enum from a raw V4L2
    /// CAPTURE pixel-format fourcc. Callers that only kept the fourcc (e.g.
    /// the V4L2 session) use this to hand a typed format back to state
    /// layer objects that carry `DecodedFormat`.
    pub(crate) fn from_v4l2_fourcc(fourcc: u32) -> Option<Self> {
        match fourcc {
            f if f == Self::Nv12.v4l2_fourcc() => Some(Self::Nv12),
            f if f == Self::P010.v4l2_fourcc() => Some(Self::P010),
            _ => None,
        }
    }

    pub(crate) const fn rt_format(self) -> u32 {
        match self {
            Self::Nv12 => VA_RT_FORMAT_YUV420,
            Self::P010 => VA_RT_FORMAT_YUV420_10,
        }
    }

    pub(crate) const fn va_fourcc(self) -> u32 {
        match self {
            Self::Nv12 => VA_FOURCC_NV12,
            Self::P010 => VA_FOURCC_P010,
        }
    }

    pub(crate) const fn v4l2_fourcc(self) -> u32 {
        match self {
            Self::Nv12 => fourcc(b'N', b'V', b'1', b'2'),
            Self::P010 => fourcc(b'P', b'0', b'1', b'0'),
        }
    }

    pub(crate) const fn bytes_per_sample(self) -> u32 {
        match self {
            Self::Nv12 => 1,
            Self::P010 => 2,
        }
    }

    pub(crate) const fn bits_per_pixel(self) -> u32 {
        match self {
            Self::Nv12 => 12,
            Self::P010 => 24,
        }
    }

    pub(crate) const fn depth(self) -> u32 {
        match self {
            Self::Nv12 => 8,
            Self::P010 => 10,
        }
    }
}

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}
