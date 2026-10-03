//! VA slice-data collection for stateful HEVC, VP9, and AV1 decoders.
//!
//! VP9 VA slice data is a complete compressed frame. HEVC slice data begins
//! at each NAL header, so an Annex-B start code is restored per slice. AV1 VA
//! data is tile-granular; collecting it here establishes the correct buffer
//! contract while sequence/frame OBU synthesis remains gated from profile
//! advertisement.

mod av1;
mod complete;
mod transport;
#[cfg(test)]
mod transport_tests;

use super::{Codec, EncodedFrame};
use crate::bindings::*;
use crate::err;
use crate::state::{Buffer, DRV_MAX_SLICES_PER_FRAME};
use av1::{
    ReferenceState, av1_frame_header_input, av1_sequence_header_input, av1_tile_group_data,
    av1_tile_size_bytes_minus_1, refresh_frame_flags,
};
use std::ptr;

#[derive(Clone, Copy)]
struct DataRange {
    offset: usize,
    size: usize,
    tile_index: Option<usize>,
}

pub(crate) struct RawDecoder {
    codec: Codec,
    picture_seen: bool,
    keyframe: bool,
    hevc_picture: Option<VAPictureParameterBufferHEVC>,
    av1_picture: Option<VADecPictureParameterBufferAV1>,
    av1_refs: ReferenceState,
    ranges: Vec<DataRange>,
    chunks: Vec<Vec<u8>>,
    av1_transport: Option<transport::Transport>,
}

impl RawDecoder {
    #[cfg(test)]
    pub(crate) fn new_cbs_transport_for_test() -> Self {
        let mut decoder = Self::new(Codec::Av1);
        decoder.av1_transport = Some(transport::Transport::new());
        decoder
    }
    pub(crate) fn transport_picture(&self) -> Option<&VADecPictureParameterBufferAV1> {
        self.av1_transport.as_ref()?;
        self.av1_picture.as_ref()
    }
    pub(crate) fn new(codec: Codec) -> Self {
        Self {
            codec,
            picture_seen: false,
            keyframe: false,
            hevc_picture: None,
            av1_picture: None,
            av1_refs: ReferenceState::new(),
            ranges: Vec::new(),
            chunks: Vec::new(),
            av1_transport: (codec == Codec::Av1
                && std::env::var("V4L2_VA_AV1_CBS_TRANSPORT")
                    .map_or(cfg!(feature = "system-av1"), |value| value == "1"))
            .then(transport::Transport::new),
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        self.picture_seen = false;
        self.keyframe = false;
        self.hevc_picture = None;
        self.av1_picture = None;
        self.ranges.clear();
        self.chunks.clear();
        if let Some(transport) = &mut self.av1_transport {
            transport.data = None;
        }
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
        if let Some(transport) = &mut self.av1_transport {
            let picture = self
                .av1_picture
                .as_ref()
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            return transport.finish(picture, &self.ranges, &mut self.av1_refs, sequence);
        }
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
        let mut bytes = if self.codec == Codec::Av1 {
            Vec::new()
        } else {
            Vec::with_capacity(size + 256)
        };
        // AV1 assembles tile payloads below; avoid copying them into an
        // intermediate packet that would immediately be discarded.
        if self.codec != Codec::Av1 {
            for chunk in &self.chunks {
                if self.codec == Codec::Hevc {
                    bytes.extend_from_slice(&[0, 0, 0, 1]);
                }
                bytes.extend_from_slice(chunk);
            }
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
        let headers = if self.codec == Codec::Av1 {
            let picture = *self
                .av1_picture
                .as_ref()
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            if !self.av1_refs.matches(&picture) {
                return Err(err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE));
            }
            let seq = av1_sequence_header_input(&picture)
                .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
            let sequence_header = crate::av1::synthesize_sequence_header(&seq);
            let tile_size_bytes_minus_1 = av1_tile_size_bytes_minus_1(&self.chunks)
                .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let refresh_frame_flags = refresh_frame_flags(&picture);
            let mut frame = av1_frame_header_input(
                &picture,
                tile_size_bytes_minus_1,
                refresh_frame_flags,
                self.av1_refs.order_hints,
            )
            .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
            // VA retains a decoded surface even for hidden reference frames;
            // show_existing_frame later reuses that surface without submitting
            // another picture. Stateful V4L2 must therefore emit its pixels now.
            if frame.frame_type == crate::av1::FrameType::Inter {
                frame.show_frame = true;
            }
            let tile_data = av1_tile_group_data(
                &picture,
                &self.ranges,
                &self.chunks,
                tile_size_bytes_minus_1,
            )
            .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            if std::env::var_os("V4L2_VA_AV1_DUMP").is_some() {
                let header = crate::av1::synthesize_uncompressed_header(&seq, &frame)
                    .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
                eprintln!(
                    "av1_header order_hint={} refresh={:#04x} ref_hints={:?} bytes={:02x?}",
                    picture.order_hint, refresh_frame_flags, self.av1_refs.order_hints, header
                );
            }
            let frame_obu = crate::av1::synthesize_frame_obu(&seq, &frame, &tile_data)
                .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;

            let mut access_unit = Vec::with_capacity(
                crate::av1::ObuWriter::temporal_delimiter().len()
                    + sequence_header.len()
                    + frame_obu.len(),
            );
            access_unit.extend_from_slice(&crate::av1::ObuWriter::temporal_delimiter());
            if sequence == 0 || self.keyframe {
                access_unit.extend_from_slice(&sequence_header);
            }
            access_unit.extend_from_slice(&frame_obu);
            bytes = access_unit;
            self.av1_refs.refresh(&picture, refresh_frame_flags);
            sequence_header
        } else {
            headers
        };
        Ok(EncodedFrame {
            bytes,
            headers,
            keyframe: self.keyframe,
            expects_output: true,
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
                self.av1_picture = Some(pp);
                let seq = unsafe { pp.seq_info_fields.fields };
                let fields = unsafe { pp.pic_info_fields.bits };
                self.keyframe = fields.frame_type() == 0;
                if std::env::var_os("V4L2_VA_AV1_DUMP").is_some() {
                    eprintln!(
                        "av1_pp w={} h={} profile={} bit_depth_idx={} order_hint_bits={} frame_type={} show={} key={} tiles={}x{} order_hint={} primary_ref={} base_q={} seq_value={:#x} pic_value={:#x}",
                        pp.frame_width_minus1 as u32 + 1,
                        pp.frame_height_minus1 as u32 + 1,
                        pp.profile,
                        pp.bit_depth_idx,
                        pp.order_hint_bits_minus_1 as u32 + 1,
                        fields.frame_type(),
                        fields.show_frame(),
                        self.keyframe,
                        pp.tile_cols,
                        pp.tile_rows,
                        pp.order_hint,
                        pp.primary_ref_frame,
                        pp.base_qindex,
                        unsafe { pp.seq_info_fields.value },
                        unsafe { pp.pic_info_fields.value },
                    );
                    eprintln!(
                        "av1_seq still={} sb128={} filter_intra={} intra_edge={} interintra={} masked={} dual={} order_hint={} jnt={} cdef={} mono={} subx={} suby={} film={} color_range={} chroma_pos={}",
                        seq.still_picture(),
                        seq.use_128x128_superblock(),
                        seq.enable_filter_intra(),
                        seq.enable_intra_edge_filter(),
                        seq.enable_interintra_compound(),
                        seq.enable_masked_compound(),
                        seq.enable_dual_filter(),
                        seq.enable_order_hint(),
                        seq.enable_jnt_comp(),
                        seq.enable_cdef(),
                        seq.mono_chrome(),
                        seq.subsampling_x(),
                        seq.subsampling_y(),
                        seq.film_grain_params_present(),
                        seq.color_range(),
                        seq.chroma_sample_position()
                    );
                    let lf = unsafe { pp.loop_filter_info_fields.bits };
                    let qm = unsafe { pp.qmatrix_fields.bits };
                    let mc = unsafe { pp.mode_control_fields.bits };
                    let lr = unsafe { pp.loop_restoration_fields.bits };
                    let seg = unsafe { pp.seg_info.segment_info_fields.bits };
                    eprintln!(
                        "av1_pic showable={} err={} cdf={} screen={} intmv={} intrabc={} superres={} hpmv={} motion_switch={} ref_mvs={} end_cdf={} uniform={} warped={} large={}",
                        fields.showable_frame(),
                        fields.error_resilient_mode(),
                        fields.disable_cdf_update(),
                        fields.allow_screen_content_tools(),
                        fields.force_integer_mv(),
                        fields.allow_intrabc(),
                        fields.use_superres(),
                        fields.allow_high_precision_mv(),
                        fields.is_motion_mode_switchable(),
                        fields.use_ref_frame_mvs(),
                        fields.disable_frame_end_update_cdf(),
                        fields.uniform_tile_spacing_flag(),
                        fields.allow_warped_motion(),
                        fields.large_scale_tile()
                    );
                    eprintln!(
                        "av1_q base={} ydc={} udc={} uac={} vdc={} vac={} qm={} qmy={} qmu={} qmv={} deltaq={} dqres={} deltalf={} dlfres={} dlfmulti={} tx={} refsel={} reducedtx={} skip={}",
                        pp.base_qindex,
                        pp.y_dc_delta_q,
                        pp.u_dc_delta_q,
                        pp.u_ac_delta_q,
                        pp.v_dc_delta_q,
                        pp.v_ac_delta_q,
                        qm.using_qmatrix(),
                        qm.qm_y(),
                        qm.qm_u(),
                        qm.qm_v(),
                        mc.delta_q_present_flag(),
                        mc.log2_delta_q_res(),
                        mc.delta_lf_present_flag(),
                        mc.log2_delta_lf_res(),
                        mc.delta_lf_multi(),
                        mc.tx_mode(),
                        mc.reference_select(),
                        mc.reduced_tx_set_used(),
                        mc.skip_mode_present()
                    );
                    eprintln!(
                        "av1_lf levels={:?} u={} v={} sharp={} delta_enabled={} delta_update={} ref={:?} mode={:?}",
                        pp.filter_level,
                        pp.filter_level_u,
                        pp.filter_level_v,
                        lf.sharpness_level(),
                        lf.mode_ref_delta_enabled(),
                        lf.mode_ref_delta_update(),
                        pp.ref_deltas,
                        pp.mode_deltas
                    );
                    eprintln!(
                        "av1_refs current={} map={:?} idx={:?}",
                        pp.current_frame, pp.ref_frame_map, pp.ref_frame_idx
                    );
                    eprintln!(
                        "av1_cdef damping={} bits={} y={:?} uv={:?} lr=({},{},{}) lr_shift={} lr_uv={} seg enabled={} update_map={} temporal={} update_data={}",
                        pp.cdef_damping_minus_3,
                        pp.cdef_bits,
                        pp.cdef_y_strengths,
                        pp.cdef_uv_strengths,
                        lr.yframe_restoration_type(),
                        lr.cbframe_restoration_type(),
                        lr.crframe_restoration_type(),
                        lr.lr_unit_shift(),
                        lr.lr_uv_shift(),
                        seg.enabled(),
                        seg.update_map(),
                        seg.temporal_update(),
                        seg.update_data()
                    );
                }
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
                Ok((sp.slice_data_offset, sp.slice_data_size, None))
            }),
            Codec::Vp9 => self.read_typed_ranges::<VASliceParameterBufferVP9>(buffer, |sp| {
                Ok((sp.slice_data_offset, sp.slice_data_size, None))
            }),
            Codec::Av1 => {
                let picture = self
                    .av1_picture
                    .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
                self.read_typed_ranges::<VASliceParameterBufferAV1>(buffer, |sp| {
                    if std::env::var_os("V4L2_VA_AV1_DUMP").is_some() {
                        eprintln!(
                            "av1_tile row={} col={} tg_start={} tg_end={} off={} size={}",
                            sp.tile_row,
                            sp.tile_column,
                            sp.tg_start,
                            sp.tg_end,
                            sp.slice_data_offset,
                            sp.slice_data_size
                        );
                    }
                    let tile_index = av1::tile_index(&picture, &sp)
                        .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
                    Ok((sp.slice_data_offset, sp.slice_data_size, Some(tile_index)))
                })
            }
            Codec::H264 => unreachable!(),
        }
    }

    fn read_typed_ranges<T: Copy>(
        &mut self,
        buffer: &Buffer,
        range: impl Fn(T) -> Result<(u32, u32, Option<usize>), VAStatus>,
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
            let (offset, size, tile_index) = range(params)?;
            if size == 0 {
                return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
            }
            self.ranges.push(DataRange {
                offset: offset as usize,
                size: size as usize,
                tile_index,
            });
        }
        Ok(())
    }

    fn read_data(&mut self, buffer: &Buffer) -> Result<(), VAStatus> {
        let total = (buffer.elem_size as usize)
            .checked_mul(buffer.num_elements as usize)
            .filter(|total| *total <= buffer.data.len())
            .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
        if let Some(transport) = &mut self.av1_transport {
            // VA clients may submit the complete data before tile parameters
            // (Chromium does). Collect once; finish validates every tile bound
            // and ownership before committing or submitting to V4L2.
            if transport.data.is_some() || total == 0 || total > 64 * 1024 * 1024 {
                return Err(err(VA_STATUS_ERROR_INVALID_PARAMETER));
            }
            transport.data = Some(buffer.data[..total].to_vec());
            return Ok(());
        }
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
            if self.codec == Codec::Av1 && std::env::var_os("V4L2_VA_AV1_DUMP").is_some() {
                let preview_len = chunk.len().min(64);
                eprintln!(
                    "av1_chunk len={} first={:02x?}",
                    chunk.len(),
                    &chunk[..preview_len]
                );
            }
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
