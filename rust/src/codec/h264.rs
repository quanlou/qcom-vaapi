//! H.264 VA buffer parsing and Annex-B assembly.

use super::EncodedFrame;
use crate::bindings::*;
use crate::err;
use crate::h264::{H264Slice, H264Synth};
use crate::state::{Buffer, DRV_MAX_SLICES_PER_FRAME};
use std::ptr;

pub(crate) struct H264Decoder {
    slices: Vec<H264Slice>,
    synth: H264Synth,
    first_poc: Option<i32>,
    epoch_usec: u64,
    max_timestamp_usec: u64,
}

impl H264Decoder {
    pub(crate) fn new(profile: VAProfile) -> Self {
        Self {
            slices: Vec::new(),
            synth: H264Synth::new(profile),
            first_poc: None,
            epoch_usec: 0,
            max_timestamp_usec: 0,
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        self.slices.clear();
        self.synth.begin_picture();
    }

    pub(crate) fn render_buffer(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        match buffer.type_ {
            VABufferType::VAPictureParameterBufferType => {
                let pp = read_one::<VAPictureParameterBufferH264>(buffer)?;
                self.synth.set_picture_params(pp);
            }
            VABufferType::VAIQMatrixBufferType => {
                if buffer.data.len() >= std::mem::size_of::<VAIQMatrixBufferH264>() {
                    let iq = unsafe {
                        ptr::read_unaligned(buffer.data.as_ptr() as *const VAIQMatrixBufferH264)
                    };
                    self.synth.set_iq_matrix(iq);
                }
            }
            VABufferType::VASliceParameterBufferType => self.read_slice_parameters(buffer)?,
            VABufferType::VASliceDataBufferType => self.read_slice_data(buffer)?,
            _ => return Err(err(VA_STATUS_ERROR_UNSUPPORTED_BUFFERTYPE)),
        }
        Ok(())
    }

    pub(crate) fn finish_picture(&mut self) -> Result<EncodedFrame, VAStatus> {
        if !self.synth.have_pp
            || self.slices.is_empty()
            || self.slices.iter().any(|slice| slice.data.is_empty())
        {
            return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        let frame = self
            .synth
            .assemble_frame(&self.slices)
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        let keyframe = frame
            .bytes
            .windows(5)
            .any(|bytes| bytes == [0, 0, 0, 1, 0x65]);
        let poc = self.synth.pp.CurrPic.TopFieldOrderCnt;
        let timestamp_usec = normalized_timestamp(
            poc,
            keyframe,
            &mut self.first_poc,
            &mut self.epoch_usec,
            &mut self.max_timestamp_usec,
        );
        self.slices.clear();
        Ok(EncodedFrame {
            bytes: frame.bytes,
            headers: self.synth.header_bytes(),
            keyframe,
            timestamp_usec,
        })
    }

    fn read_slice_parameters(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        let elem = std::mem::size_of::<VASliceParameterBufferH264>();
        if (buffer.elem_size as usize) < elem {
            return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
        }
        let count = self
            .slices
            .len()
            .checked_add(buffer.num_elements as usize)
            .filter(|count| *count <= DRV_MAX_SLICES_PER_FRAME)
            .ok_or_else(|| err(VA_STATUS_ERROR_MAX_NUM_EXCEEDED))?;
        self.slices.reserve(count - self.slices.len());
        for index in 0..buffer.num_elements as usize {
            let offset = index
                .checked_mul(buffer.elem_size as usize)
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let end = offset
                .checked_add(elem)
                .filter(|end| *end <= buffer.data.len())
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let sp = unsafe {
                ptr::read_unaligned(
                    buffer.data[offset..end].as_ptr() as *const VASliceParameterBufferH264
                )
            };
            self.slices.push(H264Slice {
                sp,
                data: Vec::new(),
            });
        }
        Ok(())
    }

    fn read_slice_data(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        let first = self
            .slices
            .iter()
            .position(|slice| slice.data.is_empty())
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        let total = (buffer.elem_size as usize)
            .checked_mul(buffer.num_elements as usize)
            .filter(|total| *total <= buffer.data.len())
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        for slice in self.slices.iter_mut().skip(first) {
            if !slice.data.is_empty() {
                continue;
            }
            let offset = slice.sp.slice_data_offset as usize;
            let end = offset
                .checked_add(slice.sp.slice_data_size as usize)
                .filter(|end| *end <= total)
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            if end == offset {
                return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
            }
            slice.data.extend_from_slice(&buffer.data[offset..end]);
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

fn normalized_timestamp(
    poc: i32,
    keyframe: bool,
    first_poc: &mut Option<i32>,
    epoch_usec: &mut u64,
    max_timestamp_usec: &mut u64,
) -> u64 {
    if poc < 0 {
        return *epoch_usec;
    }
    if keyframe && first_poc.is_some() {
        *epoch_usec = max_timestamp_usec.saturating_add(33_333);
        *first_poc = Some(poc);
    }
    let base = *first_poc.get_or_insert(poc);
    let relative_poc = i64::from(poc).saturating_sub(i64::from(base)).max(0) as u64;
    let timestamp = epoch_usec.saturating_add((relative_poc * 100_000 + 3) / 6);
    *max_timestamp_usec = (*max_timestamp_usec).max(timestamp);
    timestamp
}

#[cfg(test)]
mod tests {
    use super::normalized_timestamp;

    #[test]
    fn timestamps_are_relative_to_first_poc() {
        let mut first = None;
        let mut epoch = 0;
        let mut max = 0;
        assert_eq!(
            normalized_timestamp(4, false, &mut first, &mut epoch, &mut max),
            0
        );
        assert_eq!(
            normalized_timestamp(6, false, &mut first, &mut epoch, &mut max),
            33_333
        );
    }
}
