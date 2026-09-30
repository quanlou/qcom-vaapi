//! HEVC VPS/SPS/PPS synthesis from VA long-format picture parameters.

use super::bitstream::{BitReader, BitWriter};
use super::{Error, Nal};
use crate::bindings::VAPictureParameterBufferHEVC;

/// Recover the PPS id referenced by a complete HEVC slice NAL (without an
/// Annex-B start code). VA long-format slice buffers retain this field in the
/// original header but do not expose it separately.
pub(crate) fn slice_pps_id(nal: &[u8]) -> Result<u32, Error> {
    let parsed = Nal::parse(nal)?;
    if !parsed.kind().is_slice() {
        return Err(Error::UnexpectedNalType);
    }
    let rbsp = parsed.rbsp()?;
    let mut reader = BitReader::new(&rbsp);
    let first_slice_segment_in_pic_flag = reader.get(1)?;
    if first_slice_segment_in_pic_flag == 0 {
        return Err(Error::OutOfRange("first slice must lead access unit"));
    }
    if (16..=23).contains(&parsed.header.nal_unit_type) {
        reader.skip(1)?; // no_output_of_prior_pics_flag
    }
    reader.ue()
}

/// Synthesize the parameter sets a stateful V4L2 decoder needs before VA
/// long-format HEVC slices. The VA picture buffer exposes the SPS/PPS fields
/// needed by Main-profile streams, but not SPS-resident reference-picture-set
/// syntax. We therefore reject that shape rather than generating headers that
/// would make the original slice header ambiguous.
pub(crate) fn synthesize_parameter_sets(
    pp: &VAPictureParameterBufferHEVC,
    pps_id: u32,
) -> Result<Vec<u8>, Error> {
    if pp.num_short_term_ref_pic_sets != 0 {
        return Err(Error::OutOfRange("SPS short-term reference picture sets"));
    }
    let pic = unsafe { pp.pic_fields.bits };
    let slice = unsafe { pp.slice_parsing_fields.bits };
    if pic.scaling_list_enabled_flag() != 0 || slice.long_term_ref_pics_present_flag() != 0 {
        return Err(Error::OutOfRange(
            "HEVC scaling lists or long-term references",
        ));
    }
    validate_tiles(pp)?;
    let profile_idc = if pp.bit_depth_luma_minus8 > 0 { 2 } else { 1 };
    let reorder = if pic.NoPicReorderingFlag() != 0 {
        0
    } else {
        pp.sps_max_dec_pic_buffering_minus1.min(2)
    };
    let level_idc = level_idc(pp.pic_width_in_luma_samples, pp.pic_height_in_luma_samples);

    let vps = synthesize_vps(
        pp.sps_max_dec_pic_buffering_minus1,
        reorder,
        profile_idc,
        level_idc,
    );
    let sps = synthesize_sps(pp, reorder, profile_idc, level_idc);
    let pps = synthesize_pps(pp, pps_id);
    let mut headers = Vec::with_capacity(vps.len() + sps.len() + pps.len());
    headers.extend_from_slice(&vps);
    headers.extend_from_slice(&sps);
    headers.extend_from_slice(&pps);
    Ok(headers)
}

// The last tile dimension is inferred. Explicit dimensions must fit both
// the fixed VA arrays and the coded picture, leaving a nonempty final tile.
fn validate_tiles(pp: &VAPictureParameterBufferHEVC) -> Result<(), Error> {
    if unsafe { pp.pic_fields.bits }.tiles_enabled_flag() == 0 {
        return Ok(());
    }
    let columns = usize::from(pp.num_tile_columns_minus1);
    let rows = usize::from(pp.num_tile_rows_minus1);
    if columns > pp.column_width_minus1.len() || rows > pp.row_height_minus1.len() {
        return Err(Error::OutOfRange("HEVC tile counts exceed VA arrays"));
    }
    let log2_ctb = u32::from(pp.log2_min_luma_coding_block_size_minus3)
        + 3
        + u32::from(pp.log2_diff_max_min_luma_coding_block_size);
    if !(4..=6).contains(&log2_ctb) {
        return Err(Error::OutOfRange("HEVC coding tree block size"));
    }
    let ctb_size = 1u32 << log2_ctb;
    let picture_columns = u32::from(pp.pic_width_in_luma_samples).div_ceil(ctb_size);
    let picture_rows = u32::from(pp.pic_height_in_luma_samples).div_ceil(ctb_size);
    let explicit_columns: u32 = pp.column_width_minus1[..columns]
        .iter()
        .map(|width| u32::from(*width) + 1)
        .sum();
    let explicit_rows: u32 = pp.row_height_minus1[..rows]
        .iter()
        .map(|height| u32::from(*height) + 1)
        .sum();
    if explicit_columns >= picture_columns || explicit_rows >= picture_rows {
        return Err(Error::OutOfRange("HEVC tiles exceed coded picture"));
    }
    Ok(())
}

fn write_profile_tier_level(writer: &mut BitWriter, profile_idc: u8, level_idc: u8) {
    writer.put(0, 2); // general_profile_space
    writer.put(0, 1); // general_tier_flag
    writer.put(u64::from(profile_idc), 5);
    let compatibility = if profile_idc == 1 {
        0x6000_0000
    } else {
        0x4000_0000
    };
    writer.put(compatibility, 32);
    writer.put(1, 1); // progressive_source_flag
    writer.put(0, 1); // interlaced_source_flag
    writer.put(0, 1); // non_packed_constraint_flag
    writer.put(1, 1); // frame_only_constraint_flag
    writer.put(0, 32);
    writer.put(0, 12);
    writer.put(u64::from(level_idc), 8);
}

fn level_idc(width: u16, height: u16) -> u8 {
    match u32::from(width).saturating_mul(u32::from(height)) {
        0..=552_960 => 90,            // level 3.0
        552_961..=983_040 => 93,      // level 3.1
        983_041..=2_228_224 => 120,   // level 4.0
        2_228_225..=8_912_896 => 153, // level 5.1
        _ => 180,                     // level 6.0
    }
}

fn nal(nal_type: u8, mut writer: BitWriter) -> Vec<u8> {
    writer.rbsp_trailing();
    Nal::build(nal_type, 0, 1, &writer.into_bytes()).annex_b()
}

fn synthesize_vps(max_buffering: u8, reorder: u8, profile: u8, level: u8) -> Vec<u8> {
    let mut writer = BitWriter::new();
    writer.put(0, 4); // vps_video_parameter_set_id
    writer.put(3, 2); // base-layer internal/available
    writer.put(0, 6); // max_layers_minus1
    writer.put(0, 3); // max_sub_layers_minus1
    writer.put(1, 1); // temporal_id_nesting_flag
    writer.put(0xffff, 16);
    write_profile_tier_level(&mut writer, profile, level);
    writer.put(1, 1); // sub_layer_ordering_info_present_flag
    writer.put_ue(u32::from(max_buffering));
    writer.put_ue(u32::from(reorder));
    writer.put_ue(0); // max_latency_increase_plus1
    writer.put(0, 6); // max_layer_id
    writer.put_ue(0); // num_layer_sets_minus1
    writer.put(0, 1); // timing_info_present_flag
    writer.put(0, 1); // extension_flag
    nal(32, writer)
}

fn synthesize_sps(
    pp: &VAPictureParameterBufferHEVC,
    reorder: u8,
    profile: u8,
    level: u8,
) -> Vec<u8> {
    let pic = unsafe { pp.pic_fields.bits };
    let slice = unsafe { pp.slice_parsing_fields.bits };
    let mut writer = BitWriter::new();
    writer.put(0, 4); // sps_video_parameter_set_id
    writer.put(0, 3); // max_sub_layers_minus1
    writer.put(1, 1); // temporal_id_nesting_flag
    write_profile_tier_level(&mut writer, profile, level);
    writer.put_ue(0); // sps_seq_parameter_set_id
    writer.put_ue(pic.chroma_format_idc());
    if pic.chroma_format_idc() == 3 {
        writer.put(u64::from(pic.separate_colour_plane_flag()), 1);
    }
    writer.put_ue(u32::from(pp.pic_width_in_luma_samples));
    writer.put_ue(u32::from(pp.pic_height_in_luma_samples));
    writer.put(0, 1); // conformance_window_flag
    writer.put_ue(u32::from(pp.bit_depth_luma_minus8));
    writer.put_ue(u32::from(pp.bit_depth_chroma_minus8));
    writer.put_ue(u32::from(pp.log2_max_pic_order_cnt_lsb_minus4));
    writer.put(1, 1); // sub_layer_ordering_info_present_flag
    writer.put_ue(u32::from(pp.sps_max_dec_pic_buffering_minus1));
    writer.put_ue(u32::from(reorder));
    writer.put_ue(0); // max_latency_increase_plus1
    writer.put_ue(u32::from(pp.log2_min_luma_coding_block_size_minus3));
    writer.put_ue(u32::from(pp.log2_diff_max_min_luma_coding_block_size));
    writer.put_ue(u32::from(pp.log2_min_transform_block_size_minus2));
    writer.put_ue(u32::from(pp.log2_diff_max_min_transform_block_size));
    writer.put_ue(u32::from(pp.max_transform_hierarchy_depth_inter));
    writer.put_ue(u32::from(pp.max_transform_hierarchy_depth_intra));
    writer.put(0, 1); // scaling_list_enabled_flag: rejected above when set
    writer.put(u64::from(pic.amp_enabled_flag()), 1);
    writer.put(u64::from(slice.sample_adaptive_offset_enabled_flag()), 1);
    writer.put(u64::from(pic.pcm_enabled_flag()), 1);
    if pic.pcm_enabled_flag() != 0 {
        writer.put(u64::from(pp.pcm_sample_bit_depth_luma_minus1), 4);
        writer.put(u64::from(pp.pcm_sample_bit_depth_chroma_minus1), 4);
        writer.put_ue(u32::from(pp.log2_min_pcm_luma_coding_block_size_minus3));
        writer.put_ue(u32::from(pp.log2_diff_max_min_pcm_luma_coding_block_size));
        writer.put(u64::from(pic.pcm_loop_filter_disabled_flag()), 1);
    }
    writer.put_ue(0); // num_short_term_ref_pic_sets
    writer.put(0, 1); // long_term_ref_pics_present_flag
    writer.put(u64::from(slice.sps_temporal_mvp_enabled_flag()), 1);
    writer.put(u64::from(pic.strong_intra_smoothing_enabled_flag()), 1);
    writer.put(0, 1); // vui_parameters_present_flag
    writer.put(0, 1); // sps_extension_present_flag
    nal(33, writer)
}

fn synthesize_pps(pp: &VAPictureParameterBufferHEVC, pps_id: u32) -> Vec<u8> {
    let pic = unsafe { pp.pic_fields.bits };
    let slice = unsafe { pp.slice_parsing_fields.bits };
    let mut writer = BitWriter::new();
    writer.put_ue(pps_id);
    writer.put_ue(0); // pps_seq_parameter_set_id
    writer.put(u64::from(slice.dependent_slice_segments_enabled_flag()), 1);
    writer.put(u64::from(slice.output_flag_present_flag()), 1);
    writer.put(u64::from(pp.num_extra_slice_header_bits), 3);
    writer.put(u64::from(pic.sign_data_hiding_enabled_flag()), 1);
    writer.put(u64::from(slice.cabac_init_present_flag()), 1);
    writer.put_ue(u32::from(pp.num_ref_idx_l0_default_active_minus1));
    writer.put_ue(u32::from(pp.num_ref_idx_l1_default_active_minus1));
    writer.put_se(i32::from(pp.init_qp_minus26));
    writer.put(u64::from(pic.constrained_intra_pred_flag()), 1);
    writer.put(u64::from(pic.transform_skip_enabled_flag()), 1);
    writer.put(u64::from(pic.cu_qp_delta_enabled_flag()), 1);
    if pic.cu_qp_delta_enabled_flag() != 0 {
        writer.put_ue(u32::from(pp.diff_cu_qp_delta_depth));
    }
    writer.put_se(i32::from(pp.pps_cb_qp_offset));
    writer.put_se(i32::from(pp.pps_cr_qp_offset));
    writer.put(
        u64::from(slice.pps_slice_chroma_qp_offsets_present_flag()),
        1,
    );
    writer.put(u64::from(pic.weighted_pred_flag()), 1);
    writer.put(u64::from(pic.weighted_bipred_flag()), 1);
    writer.put(u64::from(pic.transquant_bypass_enabled_flag()), 1);
    writer.put(u64::from(pic.tiles_enabled_flag()), 1);
    writer.put(u64::from(pic.entropy_coding_sync_enabled_flag()), 1);
    if pic.tiles_enabled_flag() != 0 {
        writer.put_ue(u32::from(pp.num_tile_columns_minus1));
        writer.put_ue(u32::from(pp.num_tile_rows_minus1));
        writer.put(0, 1); // uniform_spacing_flag
        for width in pp
            .column_width_minus1
            .iter()
            .take(pp.num_tile_columns_minus1 as usize)
        {
            writer.put_ue(u32::from(*width));
        }
        for height in pp
            .row_height_minus1
            .iter()
            .take(pp.num_tile_rows_minus1 as usize)
        {
            writer.put_ue(u32::from(*height));
        }
        writer.put(u64::from(pic.loop_filter_across_tiles_enabled_flag()), 1);
    }
    writer.put(
        u64::from(pic.pps_loop_filter_across_slices_enabled_flag()),
        1,
    );
    let deblocking_present = slice.deblocking_filter_override_enabled_flag() != 0
        || slice.pps_disable_deblocking_filter_flag() != 0
        || pp.pps_beta_offset_div2 != 0
        || pp.pps_tc_offset_div2 != 0;
    writer.put(u64::from(deblocking_present), 1);
    if deblocking_present {
        writer.put(
            u64::from(slice.deblocking_filter_override_enabled_flag()),
            1,
        );
        writer.put(u64::from(slice.pps_disable_deblocking_filter_flag()), 1);
        if slice.pps_disable_deblocking_filter_flag() == 0 {
            writer.put_se(i32::from(pp.pps_beta_offset_div2));
            writer.put_se(i32::from(pp.pps_tc_offset_div2));
        }
    }
    writer.put(0, 1); // pps_scaling_list_data_present_flag
    writer.put(u64::from(slice.lists_modification_present_flag()), 1);
    writer.put_ue(u32::from(pp.log2_parallel_merge_level_minus2));
    writer.put(
        u64::from(slice.slice_segment_header_extension_present_flag()),
        1,
    );
    writer.put(0, 1); // pps_extension_present_flag
    nal(34, writer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiled_picture() -> VAPictureParameterBufferHEVC {
        let mut pp: VAPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
        pp.pic_width_in_luma_samples = 3840;
        pp.pic_height_in_luma_samples = 2160;
        pp.log2_diff_max_min_luma_coding_block_size = 3;
        let mut fields = unsafe { pp.pic_fields.bits };
        fields.set_tiles_enabled_flag(1);
        pp.pic_fields.bits = fields;
        pp
    }

    #[test]
    fn tile_counts_are_bounded_by_va_storage() {
        let mut pp = tiled_picture();
        pp.num_tile_columns_minus1 = pp.column_width_minus1.len() as u8;
        pp.num_tile_rows_minus1 = pp.row_height_minus1.len() as u8;
        assert!(synthesize_parameter_sets(&pp, 0).is_ok());
        pp.num_tile_columns_minus1 += 1;
        assert!(synthesize_parameter_sets(&pp, 0).is_err());
        pp.num_tile_columns_minus1 = 0;
        pp.num_tile_rows_minus1 += 1;
        assert!(synthesize_parameter_sets(&pp, 0).is_err());
    }

    #[test]
    fn explicit_tiles_leave_room_for_the_final_tile() {
        let mut pp = tiled_picture();
        pp.num_tile_columns_minus1 = 1;
        pp.column_width_minus1[0] = 59; // consumes all 60 CTU columns
        assert!(synthesize_parameter_sets(&pp, 0).is_err());
        pp.column_width_minus1[0] = 58;
        assert!(synthesize_parameter_sets(&pp, 0).is_ok());
        pp.num_tile_rows_minus1 = 1;
        pp.row_height_minus1[0] = 33; // consumes all 34 CTU rows
        assert!(synthesize_parameter_sets(&pp, 0).is_err());
    }
}
