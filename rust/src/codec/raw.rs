//! VA slice-data collection for stateful HEVC, VP9, and AV1 decoders.
//!
//! VP9 VA slice data is a complete compressed frame. HEVC slice data begins
//! at each NAL header, so an Annex-B start code is restored per slice. AV1 VA
//! data is tile-granular; collecting it here establishes the correct buffer
//! contract while sequence/frame OBU synthesis remains gated from profile
//! advertisement.

use super::{Codec, EncodedFrame};
use crate::bindings::*;
use crate::err;
use crate::state::{Buffer, DRV_MAX_SLICES_PER_FRAME};
use std::ptr;

#[derive(Clone, Copy)]
struct DataRange {
    offset: usize,
    size: usize,
}

pub(crate) struct RawDecoder {
    codec: Codec,
    picture_seen: bool,
    keyframe: bool,
    hevc_picture: Option<VAPictureParameterBufferHEVC>,
    ranges: Vec<DataRange>,
    chunks: Vec<Vec<u8>>,
}

impl RawDecoder {
    pub(crate) fn new(codec: Codec) -> Self {
        Self {
            codec,
            picture_seen: false,
            keyframe: false,
            hevc_picture: None,
            ranges: Vec::new(),
            chunks: Vec::new(),
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        self.picture_seen = false;
        self.keyframe = false;
        self.hevc_picture = None;
        self.ranges.clear();
        self.chunks.clear();
    }

    pub(crate) fn render_buffer(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        match buffer.type_ {
            VABufferType::VAPictureParameterBufferType => self.read_picture(buffer)?,
            VABufferType::VASliceParameterBufferType => self.read_ranges(buffer)?,
            VABufferType::VASliceDataBufferType => self.read_data(buffer)?,
            // Quantization matrices are consumed by stateless accelerators.
            // Iris receives the original compressed access unit and parses
            // these values itself.
            VABufferType::VAIQMatrixBufferType => {}
            _ => return Err(err(VA_STATUS_ERROR_UNSUPPORTED_BUFFERTYPE)),
        }
        Ok(())
    }

    pub(crate) fn finish_picture(&mut self, sequence: u64) -> Result<EncodedFrame, VAStatus> {
        if !self.picture_seen || self.ranges.is_empty() || self.chunks.len() != self.ranges.len() {
            return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        let size = self
            .chunks
            .iter()
            .try_fold(0usize, |total, chunk| {
                total
                    .checked_add(chunk.len())
                    .filter(|size| *size <= 64 * 1024 * 1024)
            })
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        let mut bytes = Vec::with_capacity(size + 256);
        for chunk in &self.chunks {
            if self.codec == Codec::Hevc {
                bytes.extend_from_slice(&[0, 0, 0, 1]);
            }
            bytes.extend_from_slice(chunk);
        }
        let headers = if self.codec == Codec::Hevc {
            let picture = self
                .hevc_picture
                .as_ref()
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let first = self
                .chunks
                .first()
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let pps_id = crate::h265::slice_pps_id(first)
                .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            crate::h265::synthesize_parameter_sets(picture, pps_id)
                .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?
        } else {
            Vec::new()
        };
        if self.codec == Codec::Hevc && (sequence == 0 || self.keyframe) {
            let mut with_headers = Vec::with_capacity(headers.len() + bytes.len());
            with_headers.extend_from_slice(&headers);
            with_headers.extend_from_slice(&bytes);
            bytes = with_headers;
        }
        Ok(EncodedFrame {
            bytes,
            headers,
            keyframe: self.keyframe,
            timestamp_usec: sequence.saturating_mul(33_333),
        })
    }

    fn read_picture(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        match self.codec {
            Codec::Hevc => {
                let picture: VAPictureParameterBufferHEVC = read_one(buffer)?;
                self.hevc_picture = Some(picture);
                // HEVC random-access NAL type is read from the compressed NAL
                // header once slice data arrives.
            }
            Codec::Vp9 => {
                let pp: VADecPictureParameterBufferVP9 = read_one(buffer)?;
                let fields = unsafe { pp.pic_fields.bits };
                self.keyframe = fields.frame_type() == 0;
            }
            Codec::Av1 => {
                let pp: VADecPictureParameterBufferAV1 = read_one(buffer)?;
                let fields = unsafe { pp.pic_info_fields.bits };
                self.keyframe = fields.frame_type() == 0;
            }
            Codec::H264 => unreachable!(),
        }
        self.picture_seen = true;
        Ok(())
    }

    fn read_ranges(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        if self
            .ranges
            .len()
            .checked_add(buffer.num_elements as usize)
            .is_none_or(|count| count > DRV_MAX_SLICES_PER_FRAME)
        {
            return Err(err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED));
        }
        match self.codec {
            Codec::Hevc => self.read_typed_ranges::<VASliceParameterBufferHEVC>(buffer, |sp| {
                (sp.slice_data_offset, sp.slice_data_size)
            }),
            Codec::Vp9 => self.read_typed_ranges::<VASliceParameterBufferVP9>(buffer, |sp| {
                (sp.slice_data_offset, sp.slice_data_size)
            }),
            Codec::Av1 => self.read_typed_ranges::<VASliceParameterBufferAV1>(buffer, |sp| {
                (sp.slice_data_offset, sp.slice_data_size)
            }),
            Codec::H264 => unreachable!(),
        }
    }

    fn read_typed_ranges<T: Copy>(
        &mut self,
        buffer: &Buffer,
        range: impl Fn(T) -> (u32, u32),
    ) -> Result<(), VAStatus> {
        let size = std::mem::size_of::<T>();
        if (buffer.elem_size as usize) < size {
            return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        for index in 0..buffer.num_elements as usize {
            let offset = index
                .checked_mul(buffer.elem_size as usize)
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let end = offset
                .checked_add(size)
                .filter(|end| *end <= buffer.data.len())
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let params =
                unsafe { ptr::read_unaligned(buffer.data[offset..end].as_ptr() as *const T) };
            let (offset, size) = range(params);
            if size == 0 {
                return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
            }
            self.ranges.push(DataRange {
                offset: offset as usize,
                size: size as usize,
            });
        }
        Ok(())
    }

    fn read_data(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        let total = (buffer.elem_size as usize)
            .checked_mul(buffer.num_elements as usize)
            .filter(|total| *total <= buffer.data.len())
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        let first = self.chunks.len();
        if first >= self.ranges.len() {
            return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        for range in self.ranges.iter().skip(first) {
            let end = range
                .offset
                .checked_add(range.size)
                .filter(|end| *end <= total)
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let chunk = buffer.data[range.offset..end].to_vec();
            if self.codec == Codec::Hevc && chunk.len() >= 2 {
                let nal_type = (chunk[0] >> 1) & 0x3f;
                self.keyframe |= matches!(nal_type, 19..=21);
            }
            self.chunks.push(chunk);
        }
        Ok(())
    }
}

fn read_one<T: Copy>(buffer: &Buffer) -> Result<T, VAStatus> {
    let size = std::mem::size_of::<T>();
    if (buffer.elem_size as usize) < size || buffer.data.len() < size {
        return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
    }
    Ok(unsafe { ptr::read_unaligned(buffer.data.as_ptr() as *const T) })
}
