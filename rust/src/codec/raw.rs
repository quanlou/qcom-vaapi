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
    tile_index: Option<usize>,
}

pub(crate) struct RawDecoder {
    codec: Codec,
    picture_seen: bool,
    keyframe: bool,
    hevc_picture: Option<VAPictureParameterBufferHEVC>,
    av1_picture: Option<VADecPictureParameterBufferAV1>,
    av1_ref_order_hint: [u16; 8],
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
            av1_picture: None,
            av1_ref_order_hint: [0; 8],
            ranges: Vec::new(),
            chunks: Vec::new(),
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        self.picture_seen = false;
        self.keyframe = false;
        self.hevc_picture = None;
        self.av1_picture = None;
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
        let headers = if self.codec == Codec::Av1 {
            let picture = *self
                .av1_picture
                .as_ref()
                .ok_or_else(|| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let sequence_header = av1_sequence_header_input(&picture)
                .map(|seq| crate::av1::synthesize_sequence_header(&seq))
                .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
            let seq = av1_sequence_header_input(&picture)
                .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
            let tile_size_bytes_minus_1 = av1_tile_size_bytes_minus_1(&self.chunks)
                .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
            let refresh_frame_flags = refresh_frame_flags(&picture);
            let frame = av1_frame_header_input(
                &picture,
                tile_size_bytes_minus_1,
                refresh_frame_flags,
                self.av1_ref_order_hint,
            )
            .map_err(|_| err(VA_STATUS_ERROR_UNSUPPORTED_PROFILE))?;
            let tile_data = av1_tile_group_data(
                &picture,
                &self.ranges,
                &self.chunks,
                tile_size_bytes_minus_1,
            )
            .map_err(|_| err(VA_STATUS_ERROR_INVALID_PARAMETER))?;
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
            self.update_av1_ref_order_hints(&picture, refresh_frame_flags);
            sequence_header
        } else {
            headers
        };
        Ok(EncodedFrame {
            bytes,
            headers,
            keyframe: self.keyframe,
            expects_output: self
                .av1_picture
                .as_ref()
                .is_none_or(|pp| unsafe { pp.pic_info_fields.bits }.show_frame() != 0),
            timestamp_usec: sequence.saturating_mul(33_333),
        })
    }

    fn update_av1_ref_order_hints(
        &mut self,
        pp: &VADecPictureParameterBufferAV1,
        refresh_frame_flags: u8,
    ) {
        let order_hint = u16::from(pp.order_hint);
        for slot in 0..8 {
            if (refresh_frame_flags & (1 << slot)) != 0 {
                self.av1_ref_order_hint[slot] = order_hint;
            }
        }
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
                (sp.slice_data_offset, sp.slice_data_size, None)
            }),
            Codec::Vp9 => self.read_typed_ranges::<VASliceParameterBufferVP9>(buffer, |sp| {
                (sp.slice_data_offset, sp.slice_data_size, None)
            }),
            Codec::Av1 => {
                let tile_cols = self
                    .av1_picture
                    .as_ref()
                    .map(|pp| usize::from(pp.tile_cols))
                    .unwrap_or(0);
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
                    let tile_index = (tile_cols != 0).then_some(
                        usize::from(sp.tile_row) * tile_cols + usize::from(sp.tile_column),
                    );
                    (sp.slice_data_offset, sp.slice_data_size, tile_index)
                })
            }
            Codec::H264 => unreachable!(),
        }
    }

    fn read_typed_ranges<T: Copy>(
        &mut self,
        buffer: &Buffer,
        range: impl Fn(T) -> (u32, u32, Option<usize>),
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
            let (offset, size, tile_index) = range(params);
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

fn av1_sequence_header_input(
    pp: &VADecPictureParameterBufferAV1,
) -> Result<crate::av1::SequenceHeaderInput, ()> {
    let seq = unsafe { pp.seq_info_fields.fields };
    let pic = unsafe { pp.pic_info_fields.bits };
    let seq_profile = match pp.profile {
        0 => crate::av1::SeqProfile::Main,
        1 => crate::av1::SeqProfile::High,
        2 => crate::av1::SeqProfile::Professional,
        _ => return Err(()),
    };
    let bit_depth = match pp.bit_depth_idx {
        0 => 8,
        1 => 10,
        2 => 12,
        _ => return Err(()),
    };
    let separate_uv_deltas =
        pp.v_dc_delta_q != pp.u_dc_delta_q || pp.v_ac_delta_q != pp.u_ac_delta_q;
    Ok(crate::av1::SequenceHeaderInput {
        seq_profile,
        seq_level_idx_0: 5,
        seq_tier_0: false,
        max_frame_width: u32::from(pp.frame_width_minus1) + 1,
        max_frame_height: u32::from(pp.frame_height_minus1) + 1,
        use_128x128_superblock: seq.use_128x128_superblock() != 0,
        enable_filter_intra: seq.enable_filter_intra() != 0,
        enable_intra_edge_filter: seq.enable_intra_edge_filter() != 0,
        enable_interintra_compound: seq.enable_interintra_compound() != 0,
        enable_masked_compound: seq.enable_masked_compound() != 0,
        enable_warped_motion: true,
        enable_dual_filter: seq.enable_dual_filter() != 0,
        enable_order_hint: seq.enable_order_hint() != 0,
        enable_jnt_comp: seq.enable_jnt_comp() != 0,
        enable_ref_frame_mvs: true,
        order_hint_bits_minus_1: pp.order_hint_bits_minus_1,
        enable_superres: pic.use_superres() != 0,
        enable_cdef: seq.enable_cdef() != 0,
        enable_restoration: true,
        seq_choose_integer_mv: true,
        seq_force_integer_mv: false,
        bit_depth,
        monochrome: seq.mono_chrome() != 0,
        color_description: None,
        color_range: seq.color_range() != 0,
        subsampling_x: seq.subsampling_x() != 0,
        subsampling_y: seq.subsampling_y() != 0,
        chroma_sample_position: seq.chroma_sample_position() as u8,
        separate_uv_deltas,
        film_grain_params_present: seq.film_grain_params_present() != 0,
    })
}

fn av1_frame_header_input(
    pp: &VADecPictureParameterBufferAV1,
    tile_size_bytes_minus_1: u8,
    refresh_frame_flags: u8,
    ref_order_hint: [u16; 8],
) -> Result<crate::av1::FrameHeaderInput, ()> {
    let pic = unsafe { pp.pic_info_fields.bits };
    let frame_type = match pic.frame_type() {
        0 => crate::av1::FrameType::Key,
        1 => crate::av1::FrameType::Inter,
        2 => crate::av1::FrameType::IntraOnly,
        _ => return Err(()),
    };
    let qmatrix = unsafe { pp.qmatrix_fields.bits };
    let mode = unsafe { pp.mode_control_fields.bits };
    let lf = unsafe { pp.loop_filter_info_fields.bits };
    let lr = unsafe { pp.loop_restoration_fields.bits };
    let seg = unsafe { pp.seg_info.segment_info_fields.bits };
    let film_grain = unsafe { pp.film_grain_info.film_grain_info_fields.bits };
    let mut seg_feature_enabled = [[false; 8]; 8];
    let mut seg_feature_data = [[0i32; 8]; 8];
    for segment in 0..8 {
        for feature in 0..8 {
            seg_feature_enabled[segment][feature] =
                (pp.seg_info.feature_mask[segment] & (1 << feature)) != 0;
            seg_feature_data[segment][feature] =
                i32::from(pp.seg_info.feature_data[segment][feature]);
        }
    }
    let loop_filter_ref_deltas = lf_delta_updates(lf.mode_ref_delta_update() != 0, pp.ref_deltas);
    let loop_filter_mode_deltas = lf_delta_updates(lf.mode_ref_delta_update() != 0, pp.mode_deltas);
    let cdef_y_pri = cdef_primary(pp.cdef_y_strengths);
    let cdef_y_sec = cdef_secondary(pp.cdef_y_strengths);
    let cdef_uv_pri = cdef_primary(pp.cdef_uv_strengths);
    let cdef_uv_sec = cdef_secondary(pp.cdef_uv_strengths);
    let global_motion_is_global = pp
        .wm
        .map(|wm| wm.wmtype != VAAV1TransformationType::VAAV1TransformationIdentity);
    let separate_uv_deltas =
        pp.v_dc_delta_q != pp.u_dc_delta_q || pp.v_ac_delta_q != pp.u_ac_delta_q;
    Ok(crate::av1::FrameHeaderInput {
        frame_type,
        show_frame: pic.show_frame() != 0,
        showable_frame: pic.showable_frame() != 0,
        error_resilient_mode: pic.error_resilient_mode() != 0,
        disable_cdf_update: pic.disable_cdf_update() != 0,
        allow_screen_content_tools: pic.allow_screen_content_tools() != 0,
        force_integer_mv: pic.force_integer_mv() != 0,
        frame_size_override_flag: false,
        frame_width_minus_1: pp.frame_width_minus1,
        frame_height_minus_1: pp.frame_height_minus1,
        order_hint: u16::from(pp.order_hint),
        primary_ref_frame: pp.primary_ref_frame,
        refresh_frame_flags,
        ref_order_hint,
        use_superres: pic.use_superres() != 0,
        superres_coded_denom: pp.superres_scale_denominator.saturating_sub(9),
        render_and_frame_size_different: false,
        render_width_minus_1: pp.frame_width_minus1,
        render_height_minus_1: pp.frame_height_minus1,
        allow_intrabc: pic.allow_intrabc() != 0,
        ref_frame_idx: pp.ref_frame_idx,
        allow_high_precision_mv: pic.allow_high_precision_mv() != 0,
        is_filter_switchable: pp.interp_filter == 4,
        interpolation_filter: pp.interp_filter.min(3),
        is_motion_mode_switchable: pic.is_motion_mode_switchable() != 0,
        use_ref_frame_mvs: pic.use_ref_frame_mvs() != 0,
        disable_frame_end_update_cdf: pic.disable_frame_end_update_cdf() != 0,
        uniform_tile_spacing: pic.uniform_tile_spacing_flag() != 0,
        tile_cols: pp.tile_cols,
        tile_rows: pp.tile_rows,
        context_update_tile_id: pp.context_update_tile_id,
        tile_size_bytes_minus_1,
        base_q_idx: pp.base_qindex,
        delta_q_y_dc: av1_delta_q(pp.y_dc_delta_q),
        delta_q_u_dc: av1_delta_q(pp.u_dc_delta_q),
        delta_q_u_ac: av1_delta_q(pp.u_ac_delta_q),
        delta_q_v_dc: separate_uv_deltas
            .then_some(pp.v_dc_delta_q)
            .and_then(av1_delta_q),
        delta_q_v_ac: separate_uv_deltas
            .then_some(pp.v_ac_delta_q)
            .and_then(av1_delta_q),
        diff_uv_delta: separate_uv_deltas,
        using_qmatrix: qmatrix.using_qmatrix() != 0,
        qm_y: qmatrix.qm_y() as u8,
        qm_u: qmatrix.qm_u() as u8,
        qm_v: qmatrix.qm_v() as u8,
        segmentation_enabled: seg.enabled() != 0,
        segmentation_update_map: seg.update_map() != 0,
        segmentation_temporal_update: seg.temporal_update() != 0,
        segmentation_update_data: seg.update_data() != 0,
        seg_feature_enabled,
        seg_feature_data,
        delta_q_present: mode.delta_q_present_flag() != 0,
        delta_q_res: mode.log2_delta_q_res() as u8,
        delta_lf_present: mode.delta_lf_present_flag() != 0,
        delta_lf_res: mode.log2_delta_lf_res() as u8,
        delta_lf_multi: mode.delta_lf_multi() != 0,
        loop_filter_level: [
            pp.filter_level[0],
            pp.filter_level[1],
            pp.filter_level_u,
            pp.filter_level_v,
        ],
        loop_filter_sharpness: lf.sharpness_level(),
        loop_filter_delta_enabled: lf.mode_ref_delta_enabled() != 0,
        loop_filter_delta_update: lf.mode_ref_delta_update() != 0,
        loop_filter_ref_deltas,
        loop_filter_mode_deltas,
        cdef_damping_minus_3: pp.cdef_damping_minus_3,
        cdef_bits: pp.cdef_bits,
        cdef_y_pri,
        cdef_y_sec,
        cdef_uv_pri,
        cdef_uv_sec,
        lr_type: [
            lr.yframe_restoration_type() as u8,
            lr.cbframe_restoration_type() as u8,
            lr.crframe_restoration_type() as u8,
        ],
        lr_unit_shift: lr.lr_unit_shift() != 0,
        lr_unit_extra_shift: lr.lr_unit_shift() > 1,
        lr_uv_shift: lr.lr_uv_shift() != 0,
        tx_mode_select: mode.tx_mode() == 2,
        reference_select: mode.reference_select() != 0,
        skip_mode_present: mode.skip_mode_present() != 0,
        allow_warped_motion: pic.allow_warped_motion() != 0,
        reduced_tx_set: mode.reduced_tx_set_used() != 0,
        global_motion_is_global,
        film_grain_apply: film_grain.apply_grain() != 0,
    })
}

fn av1_delta_q(value: i8) -> Option<i8> {
    (value != 0).then_some(value / 2)
}

fn lf_delta_updates<const N: usize>(update: bool, values: [i8; N]) -> [Option<i8>; N] {
    if update { values.map(Some) } else { [None; N] }
}

fn cdef_primary(packed: [u8; 8]) -> [u8; 8] {
    packed.map(|value| value >> 2)
}

fn cdef_secondary(packed: [u8; 8]) -> [u8; 8] {
    packed.map(|value| value & 0x03)
}

fn refresh_frame_flags(pp: &VADecPictureParameterBufferAV1) -> u8 {
    let pic = unsafe { pp.pic_info_fields.bits };
    if pic.frame_type() == 0 && pic.show_frame() != 0 {
        return 0xff;
    }
    let current = pp.current_frame;
    let inferred = pp
        .ref_frame_map
        .iter()
        .enumerate()
        .fold(0u8, |flags, (idx, surface)| {
            flags | (u8::from(*surface == current) << idx)
        });
    if inferred != 0 || pic.show_frame() != 0 {
        return inferred;
    }
    hierarchical_refresh_frame_flags(pp.order_hint)
}

fn hierarchical_refresh_frame_flags(order_hint: u8) -> u8 {
    let slot = if order_hint & 31 == 0 {
        0
    } else if order_hint & 15 == 0 {
        3
    } else if order_hint & 7 == 0 {
        5
    } else if order_hint & 3 == 0 {
        6
    } else {
        7
    };
    1 << slot
}

fn av1_tile_size_bytes_minus_1(chunks: &[Vec<u8>]) -> Result<u8, ()> {
    let max_size = chunks
        .iter()
        .map(Vec::len)
        .max()
        .ok_or(())?
        .checked_sub(1)
        .ok_or(())?;
    let bytes = if max_size <= 0xff {
        1
    } else if max_size <= 0xffff {
        2
    } else if max_size <= 0x00ff_ffff {
        3
    } else {
        4
    };
    Ok(bytes - 1)
}

fn av1_tile_group_data(
    pp: &VADecPictureParameterBufferAV1,
    ranges: &[DataRange],
    chunks: &[Vec<u8>],
    tile_size_bytes_minus_1: u8,
) -> Result<Vec<u8>, ()> {
    let tile_count = usize::from(pp.tile_cols)
        .checked_mul(usize::from(pp.tile_rows))
        .ok_or(())?;
    if tile_count == 0 || ranges.len() != chunks.len() || chunks.len() != tile_count {
        return Err(());
    }
    let mut by_tile = vec![None; tile_count];
    for (range, chunk) in ranges.iter().zip(chunks) {
        let tile_index = range.tile_index.ok_or(())?;
        if tile_index >= tile_count || by_tile[tile_index].is_some() {
            return Err(());
        }
        by_tile[tile_index] = Some(chunk.as_slice());
    }
    let tile_size_bytes = usize::from(tile_size_bytes_minus_1) + 1;
    let total_payload = chunks.iter().map(Vec::len).sum::<usize>();
    let mut out = Vec::with_capacity(total_payload + tile_count * tile_size_bytes + 1);
    if tile_count > 1 {
        let mut bits = crate::av1::BitWriter::new();
        bits.write_flag(false);
        out.extend_from_slice(&bits.finish());
    }
    for (tile_index, tile) in by_tile.iter().enumerate() {
        let tile = tile.ok_or(())?;
        if tile_index + 1 != tile_count {
            let size_minus_1 = tile.len().checked_sub(1).ok_or(())?;
            for byte in 0..tile_size_bytes {
                out.push(((size_minus_1 >> (8 * byte)) & 0xff) as u8);
            }
        }
        out.extend_from_slice(tile);
    }
    Ok(out)
}
