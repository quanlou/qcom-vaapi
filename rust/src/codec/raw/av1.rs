//! Translation from AV1 VA picture parameters to compressed header inputs.
//!
//! Some sequence syntax and reference refresh flags are absent from VA's
//! stateless contract. The current defaults are experimental and cannot yet
//! promise stateful inter-frame parity; capability reporting gates this path.

use super::DataRange;
use crate::bindings::*;

/// Shadow of the firmware DPB. VA does not carry refresh_frame_flags, so
/// experimental refresh inference must be checked against the next VA map
/// before we submit another frame that might use the wrong reference pixels.
pub(super) struct ReferenceState {
    surfaces: [VASurfaceID; 8],
    pub(super) order_hints: [u16; 8],
}

impl ReferenceState {
    pub(super) fn new() -> Self {
        Self {
            surfaces: [VA_INVALID_ID; 8],
            order_hints: [0; 8],
        }
    }

    pub(super) fn matches(&self, pp: &VADecPictureParameterBufferAV1) -> bool {
        let fields = unsafe { pp.pic_info_fields.bits };
        (fields.frame_type() == 0 && fields.show_frame() != 0) || pp.ref_frame_map == self.surfaces
    }

    pub(super) fn refresh(&mut self, pp: &VADecPictureParameterBufferAV1, flags: u8) {
        for slot in 0..8 {
            if flags & (1 << slot) != 0 {
                self.surfaces[slot] = pp.current_frame;
                self.order_hints[slot] = u16::from(pp.order_hint);
            }
        }
    }
}

pub(super) fn av1_sequence_header_input(
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

pub(super) fn av1_frame_header_input(
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
            restoration_syntax(lr.yframe_restoration_type())?,
            restoration_syntax(lr.cbframe_restoration_type())?,
            restoration_syntax(lr.crframe_restoration_type())?,
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

// FFmpeg passes delta_q directly; the VA ABI prose's historical "value * 2"
// wording does not describe the producer's actual buffer values.
fn av1_delta_q(value: i8) -> Option<i8> {
    (value != 0).then_some(value)
}

// Invert FFmpeg vaapi_av1.c's raw-syntax-to-VA table {0, 3, 1, 2}.
// VA values must not be written directly as compressed lr_type bits.
fn restoration_syntax(value: u16) -> Result<u8, ()> {
    match value {
        0 => Ok(0),
        1 => Ok(2),
        2 => Ok(3),
        3 => Ok(1),
        _ => Err(()),
    }
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

pub(super) fn refresh_frame_flags(pp: &VADecPictureParameterBufferAV1) -> u8 {
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

pub(super) fn av1_tile_size_bytes_minus_1(chunks: &[Vec<u8>]) -> Result<u8, ()> {
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

pub(super) fn av1_tile_group_data(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_map_divergence_is_detected_before_next_submission() {
        let mut state = ReferenceState::new();
        let mut pp: VADecPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pp.ref_frame_map = [VA_INVALID_ID; 8];
        pp.current_frame = 42;
        state.refresh(&pp, 255);
        pp.ref_frame_map = [42; 8];
        assert!(state.matches(&pp));
        pp.current_frame = 43;
        pp.order_hint = 64;
        state.refresh(&pp, 1); // inferred slot 0
        pp.ref_frame_map[1] = 43; // producer actually refreshed slot 1
        assert!(!state.matches(&pp));
    }

    #[test]
    fn hidden_reference_header_matches_original_sample() {
        // Third coded frame, order_hint=16. VA values captured from FFmpeg;
        // expected bytes extracted from the original OBU, independently of
        // our writer. This catches the restoration enum translation bug.
        let mut pp: VADecPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pp.frame_width_minus1 = 1279;
        pp.frame_height_minus1 = 719;
        pp.order_hint_bits_minus_1 = 6;
        pp.order_hint = 16;
        pp.seq_info_fields.value = 0x7298;
        pp.pic_info_fields.value = 0x5809;
        pp.tile_cols = 1;
        pp.tile_rows = 1;
        pp.base_qindex = 53;
        pp.interp_filter = 4;
        pp.ref_frame_idx = [2, 2, 2, 2, 0, 0, 0];
        pp.cdef_y_strengths[0] = 2;
        pp.cdef_uv_strengths[0] = 30;
        let mut mode = unsafe { pp.mode_control_fields.bits };
        mode.set_tx_mode(1);
        mode.set_reference_select(1);
        mode.set_skip_mode_present(1);
        pp.mode_control_fields.bits = mode;
        let mut restoration = unsafe { pp.loop_restoration_fields.bits };
        restoration.set_yframe_restoration_type(1);
        restoration.set_lr_unit_shift(2);
        pp.loop_restoration_fields.bits = restoration;
        let seq = av1_sequence_header_input(&pp).unwrap();
        let frame = av1_frame_header_input(&pp, 0, 8, [32, 0, 0, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(
            crate::av1::synthesize_uncompressed_header(&seq, &frame).unwrap(),
            [
                0x28, 0x10, 0x01, 0x04, 0x92, 0x00, 0x1d, 0x0d, 0x40, 0x00, 0x00, 0x00, 0x9e, 0x83,
                0x60, 0x00
            ]
        );
    }

    #[test]
    fn va_quantizer_and_restoration_mapping_preserves_producer_values() {
        // FFmpeg vaapi_av1.c copies signed deltas without scaling and maps
        // bitstream restoration types through {0, 3, 1, 2} for VA.
        let mut pp: VADecPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pp.y_dc_delta_q = -7;
        pp.u_dc_delta_q = 3;
        pp.u_ac_delta_q = -4;
        pp.v_dc_delta_q = 5;
        pp.v_ac_delta_q = -9;
        let mut restoration = unsafe { pp.loop_restoration_fields.bits };
        restoration.set_yframe_restoration_type(3);
        restoration.set_cbframe_restoration_type(1);
        restoration.set_crframe_restoration_type(2);
        pp.loop_restoration_fields.bits = restoration;
        let header = av1_frame_header_input(&pp, 0, 255, [0; 8]).unwrap();
        assert_eq!(header.delta_q_y_dc, Some(-7));
        assert_eq!(header.delta_q_u_dc, Some(3));
        assert_eq!(header.delta_q_u_ac, Some(-4));
        assert_eq!(header.delta_q_v_dc, Some(5));
        assert_eq!(header.delta_q_v_ac, Some(-9));
        assert_eq!(header.lr_type, [1, 2, 3]);
    }

    #[test]
    fn tile_group_orders_payloads_and_rejects_duplicates() {
        let mut pp: VADecPictureParameterBufferAV1 = unsafe { std::mem::zeroed() };
        pp.tile_cols = 2;
        pp.tile_rows = 1;
        let ranges = [
            DataRange {
                offset: 0,
                size: 1,
                tile_index: Some(1),
            },
            DataRange {
                offset: 1,
                size: 2,
                tile_index: Some(0),
            },
        ];
        let chunks = [vec![0xcc], vec![0xaa, 0xbb]];
        assert_eq!(
            av1_tile_group_data(&pp, &ranges, &chunks, 0).unwrap(),
            [0, 1, 0xaa, 0xbb, 0xcc]
        );
        let duplicates = [ranges[0], ranges[0]];
        assert!(av1_tile_group_data(&pp, &duplicates, &chunks, 0).is_err());
    }
}
