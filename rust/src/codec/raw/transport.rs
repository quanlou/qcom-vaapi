//! Opt-in complete CBS frame transport; ordinary tile-only clients stay gated.
use super::{DataRange, av1::ReferenceState};
use crate::av1::transport_prefix::{self as prefix, Sequence};
use crate::bindings::*;
use crate::codec::EncodedFrame;
use crate::err;

pub(super) struct Transport {
    sequence: Option<Sequence>,
    headers: Vec<u8>,
    pub(super) data: Option<Vec<u8>>,
    companion: Option<Result<super::complete::Companion, VAStatus>>,
}

impl Transport {
    #[cfg(test)]
    pub(super) fn with_library(path: &std::ffi::OsStr) -> Self {
        let mut transport = Self::new();
        transport.companion = Some(super::complete::Companion::load(path));
        transport
    }
    pub(super) fn new() -> Self {
        Self {
            sequence: None,
            headers: Vec::new(),
            data: None,
            companion: std::env::var_os("V4L2_VA_AV1_COMPLETE_LIBRARY")
                .or_else(|| {
                    cfg!(feature = "system-av1")
                        .then(|| std::ffi::OsString::from("libiris_av1_complete.so"))
                })
                .map(|path| super::complete::Companion::load(&path)),
        }
    }

    pub(super) fn finish(
        &mut self,
        pp: &VADecPictureParameterBufferAV1,
        ranges: &[DataRange],
        refs: &mut ReferenceState,
        sequence: u64,
    ) -> Result<EncodedFrame, VAStatus> {
        let invalid = || err(VA_STATUS_ERROR_INVALID_PARAMETER);
        let original = self.data.take().ok_or_else(invalid)?;
        let prepared = match self.companion.as_mut() {
            Some(Ok(companion)) => Some(companion.prepare(
                &original,
                ranges,
                unsafe { pp.pic_info_fields.bits }.show_frame(),
            )?),
            Some(Err(status)) => return Err(*status),
            None => None,
        };
        let data = prepared
            .as_ref()
            .map_or(original.as_slice(), |p| p.data.as_slice());
        let ranges = prepared.as_ref().map_or(ranges, |p| p.ranges.as_slice());
        let units = prefix::obus(data).map_err(|_| invalid())?;
        let mut parsed_seq = self.sequence;
        let mut new_headers = None;
        let mut coded = None;
        for unit in &units {
            match unit.kind {
                1 if coded.is_none() && new_headers.is_none() => {
                    parsed_seq = Some(
                        prefix::sequence(unit.body)
                            .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?,
                    );
                    new_headers = Some(data[..unit.end].to_vec());
                }
                2 if coded.is_none() && unit.body.is_empty() && new_headers.is_none() => {}
                6 if coded.is_none() => {
                    coded = Some(unit);
                }
                _ => return Err(invalid()),
            }
        }
        let seq = parsed_seq.ok_or_else(invalid)?;
        let coded = coded.ok_or_else(invalid)?;
        let frame = prefix::frame(coded.body, &seq).map_err(|_| invalid())?;
        let pic = unsafe { pp.pic_info_fields.bits };
        let seq_fields = unsafe { pp.seq_info_fields.fields };
        let key = frame.frame_type == 0;
        if seq.bit_depth != 8
            || pp.bit_depth_idx != 0
            || pp.profile != 0
            || u32::from(pp.frame_width_minus1) + 1 > seq.width
            || u32::from(pp.frame_height_minus1) + 1 > seq.height
            || seq_fields.enable_order_hint() != u32::from(seq.order_bits != 0)
            || (seq.order_bits != 0 && seq.order_bits != pp.order_hint_bits_minus_1 + 1)
            || frame.frame_type != pic.frame_type() as u8
            || frame.order_hint != pp.order_hint
            || frame.primary_ref != pp.primary_ref_frame
            || frame.error_resilient != (pic.error_resilient_mode() != 0)
            || (pic.show_frame() == 0 && frame.frame_type != 1)
            || seq_fields.film_grain_params_present() != 0
            || unsafe { pp.film_grain_info.film_grain_info_fields.bits }.apply_grain() != 0
            || pp.current_frame == VA_INVALID_ID
            || (pp.current_display_picture != VA_INVALID_ID
                && pp.current_display_picture != pp.current_frame)
            || (!key && pp.ref_frame_map.contains(&pp.current_frame))
            || (key && pp.ref_frame_map.iter().any(|id| *id != VA_INVALID_ID))
            || (!key && !refs.matches(pp))
            || (self.sequence.is_some_and(|old| old != seq) && !key)
            || prepared
                .as_ref()
                .is_some_and(|p| p.refresh != frame.refresh)
        {
            return Err(invalid());
        }
        if frame.frame_type == 1
            && pp
                .ref_frame_idx
                .iter()
                .any(|idx| *idx >= 8 || pp.ref_frame_map[usize::from(*idx)] == VA_INVALID_ID)
        {
            return Err(invalid());
        }
        let tile_count = usize::from(pp.tile_cols)
            .checked_mul(usize::from(pp.tile_rows))
            .filter(|count| *count > 0 && *count == ranges.len())
            .ok_or_else(invalid)?;
        let prefix_end = coded
            .body_offset
            .checked_add(frame.prefix_bits.div_ceil(8))
            .ok_or_else(invalid)?;
        let mut last_end = None;
        for (index, range) in ranges.iter().enumerate() {
            let end = range
                .offset
                .checked_add(range.size)
                .filter(|end| *end <= coded.end)
                .ok_or_else(invalid)?;
            if range.tile_index != Some(index)
                || range.offset <= prefix_end
                || range.size == 0
                || last_end.is_some_and(|last| {
                    // AV1 tile_group omits the size prefix for its final tile.
                    // Interior tiles have a one-to-four-byte size prefix.
                    if index + 1 == tile_count {
                        range.offset != last
                    } else {
                        range.offset <= last || range.offset - last > 4
                    }
                })
            {
                return Err(invalid());
            }
            last_end = Some(end);
        }
        if tile_count == 0 || last_end != Some(coded.end) || coded.end != data.len() {
            return Err(invalid());
        }
        let headers = new_headers.unwrap_or_else(|| self.headers.clone());
        if headers.is_empty() {
            return Err(invalid());
        }
        // Commit only after metadata, complete payload and map validation.
        if let Some(Ok(companion)) = &mut self.companion {
            companion.commit()?;
        }
        self.sequence = Some(seq);
        self.headers = headers.clone();
        refs.refresh(pp, frame.refresh);
        Ok(EncodedFrame {
            bytes: prepared.map_or(original, |p| p.data),
            headers,
            keyframe: key,
            expects_output: true,
            timestamp_usec: sequence.saturating_mul(33_333),
        })
    }
}
