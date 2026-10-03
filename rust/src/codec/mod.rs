//! Codec selection and codec-specific VA buffer translation.
//!
//! The V4L2 session only consumes complete coded access units.  VA clients,
//! however, submit picture parameters, slice metadata, and compressed bytes as
//! separate buffers.  Each decoder below owns that assembly state so the
//! picture lifecycle in `decode.rs` remains independent of codec syntax.

mod h264;
mod raw;

use crate::bindings::*;
use crate::state::Buffer;
use crate::v4l2::{V4L2_PIX_FMT_AV1, V4L2_PIX_FMT_H264, V4L2_PIX_FMT_HEVC, V4L2_PIX_FMT_VP9};

pub(crate) use h264::H264Decoder;
pub(crate) use raw::RawDecoder;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Codec {
    H264,
    Hevc,
    Vp9,
    Av1,
}

impl Codec {
    pub(crate) fn from_profile(profile: VAProfile) -> Option<Self> {
        match profile {
            VAProfile::VAProfileH264ConstrainedBaseline
            | VAProfile::VAProfileH264Main
            | VAProfile::VAProfileH264High => Some(Self::H264),
            VAProfile::VAProfileHEVCMain | VAProfile::VAProfileHEVCMain10 => Some(Self::Hevc),
            VAProfile::VAProfileVP9Profile0 => Some(Self::Vp9),
            VAProfile::VAProfileAV1Profile0 => Some(Self::Av1),
            _ => None,
        }
    }

    pub(crate) const fn fourcc(self) -> u32 {
        match self {
            Self::H264 => V4L2_PIX_FMT_H264,
            Self::Hevc => V4L2_PIX_FMT_HEVC,
            Self::Vp9 => V4L2_PIX_FMT_VP9,
            Self::Av1 => V4L2_PIX_FMT_AV1,
        }
    }
}

pub(crate) struct EncodedFrame {
    pub(crate) bytes: Vec<u8>,
    pub(crate) headers: Vec<u8>,
    pub(crate) keyframe: bool,
    pub(crate) expects_output: bool,
    /// Expose a hidden VP9 reference without changing its encoded show_frame
    /// bit (which would change subsequent motion-vector decoding semantics).
    pub(crate) vp9_show_existing: Option<u8>,
    pub(crate) timestamp_usec: u64,
}

pub(crate) enum Decoder {
    H264(Box<H264Decoder>),
    Raw(Box<RawDecoder>),
}

impl Decoder {
    pub(crate) fn transport_picture(&self) -> Option<&VADecPictureParameterBufferAV1> {
        match self {
            Self::Raw(decoder) => decoder.transport_picture(),
            Self::H264(_) => None,
        }
    }
    pub(crate) fn new(profile: VAProfile) -> Option<Self> {
        match Codec::from_profile(profile)? {
            Codec::H264 => Some(Self::H264(Box::new(H264Decoder::new(profile)))),
            codec => Some(Self::Raw(Box::new(RawDecoder::new(codec)))),
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        match self {
            Self::H264(decoder) => decoder.begin_picture(),
            Self::Raw(decoder) => decoder.begin_picture(),
        }
    }

    pub(crate) fn render_buffer(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        let active_bytes = (buffer.elem_size as usize).checked_mul(buffer.num_elements as usize);
        if buffer.elem_size == 0
            || buffer.num_elements == 0
            || active_bytes.is_none_or(|length| length > buffer.data.len())
        {
            return Err(crate::err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        match self {
            Self::H264(decoder) => decoder.render_buffer(buffer),
            Self::Raw(decoder) => decoder.render_buffer(buffer),
        }
    }

    pub(crate) fn finish_picture(&mut self, sequence: u64) -> Result<EncodedFrame, VAStatus> {
        match self {
            Self::H264(decoder) => decoder.finish_picture(),
            Self::Raw(decoder) => decoder.finish_picture(sequence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_rejects_inactive_and_truncated_buffers_before_reading_parameters() {
        let size = std::mem::size_of::<VAPictureParameterBufferH264>();
        let mut decoder = Decoder::new(VAProfile::VAProfileH264Main).unwrap();
        let mut buffer = Buffer {
            owner: VA_INVALID_ID,
            type_: VABufferType::VAPictureParameterBufferType,
            elem_size: size as u32,
            num_elements: 0,
            data: vec![0; size],
            mapped: false,
        };
        assert_eq!(
            decoder.render_buffer(&buffer),
            Err(crate::err(VA_STATUS_ERROR_INVALID_PARAMETER))
        );
        buffer.num_elements = 1;
        assert_eq!(decoder.render_buffer(&buffer), Ok(()));
        buffer.num_elements = 2;
        assert_eq!(
            decoder.render_buffer(&buffer),
            Err(crate::err(VA_STATUS_ERROR_INVALID_PARAMETER))
        );
    }
}
