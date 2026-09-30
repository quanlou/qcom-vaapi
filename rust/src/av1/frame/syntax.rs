//! Frame-header syntax sections and reference-order calculations.

use super::{
    Av1SynthError, BitWriter, FrameHeaderInput, FrameType, MAX_SEGMENTS, MAX_TILE_AREA,
    MAX_TILE_COLS, MAX_TILE_ROWS, MAX_TILE_WIDTH, SEG_LVL_ALT_Q, SEG_LVL_MAX,
    SEGMENTATION_FEATURE_BITS, SEGMENTATION_FEATURE_SIGNED, SequenceHeaderInput,
};

pub(super) fn write_superres(
    w: &mut BitWriter,
    frame: &FrameHeaderInput,
) -> Result<(), Av1SynthError> {
    w.write_flag(frame.use_superres);
    if frame.use_superres {
        // coded_denom = denominator − 9 fits 3 bits (denominator 9..16).
        debug_assert!(frame.superres_coded_denom <= 7);
        w.write_bits(u32::from(frame.superres_coded_denom), 3);
    }
    Ok(())
}

/// spec 5.9.3 tile_info. Only the uniform-spacing derivation is
/// supported: the increment bits are reconstructed from the caller's
/// tile_cols / tile_rows counts by walking the spec's min/max log2
/// bounds, which reproduces the encoder's bits exactly.
pub(super) fn write_tile_info(
    w: &mut BitWriter,
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
) -> Result<(), Av1SynthError> {
    if !frame.uniform_tile_spacing {
        return Err(Av1SynthError::UnsupportedNonUniformTiles);
    }
    let (width, height) = frame_size(seq, frame);
    let mi_cols = 2 * ((width + 7) >> 3);
    let mi_rows = 2 * ((height + 7) >> 3);
    let sb_shift: u32 = if seq.use_128x128_superblock { 5 } else { 4 };
    let sb_size = sb_shift + 2;
    let sb_cols = (mi_cols + (1 << sb_shift) - 1) >> sb_shift;
    let sb_rows = (mi_rows + (1 << sb_shift) - 1) >> sb_shift;
    let max_tile_width_sb = MAX_TILE_WIDTH >> sb_size;
    let max_tile_area_sb = MAX_TILE_AREA >> (2 * sb_size);
    let min_log2_tile_cols = tile_log2(max_tile_width_sb, sb_cols);
    let max_log2_tile_cols = tile_log2(1, sb_cols.min(MAX_TILE_COLS));
    let max_log2_tile_rows = tile_log2(1, sb_rows.min(MAX_TILE_ROWS));
    let min_log2_tiles = std::cmp::max(
        min_log2_tile_cols,
        tile_log2(max_tile_area_sb, sb_rows * sb_cols),
    );

    let tile_cols = frame.tile_cols;
    let tile_rows = frame.tile_rows;
    if tile_cols == 0
        || tile_rows == 0
        || (tile_cols & (tile_cols - 1)) != 0
        || (tile_rows & (tile_rows - 1)) != 0
    {
        return Err(Av1SynthError::InvalidTileCount);
    }
    let target_cols_log2 = 31 - u32::from(tile_cols).leading_zeros();
    let target_rows_log2 = 31 - u32::from(tile_rows).leading_zeros();
    if target_cols_log2 < min_log2_tile_cols
        || target_cols_log2 > max_log2_tile_cols
        || target_rows_log2 > max_log2_tile_rows
    {
        return Err(Av1SynthError::InvalidTileCount);
    }

    w.write_flag(true); // uniform_tile_spacing_flag
    let mut tile_cols_log2 = min_log2_tile_cols;
    while tile_cols_log2 < max_log2_tile_cols {
        let increment = tile_cols_log2 < target_cols_log2;
        w.write_flag(increment);
        if !increment {
            break;
        }
        tile_cols_log2 += 1;
    }
    let min_log2_tile_rows = min_log2_tiles.saturating_sub(tile_cols_log2);
    let mut tile_rows_log2 = min_log2_tile_rows;
    while tile_rows_log2 < max_log2_tile_rows {
        let increment = tile_rows_log2 < target_rows_log2;
        w.write_flag(increment);
        if !increment {
            break;
        }
        tile_rows_log2 += 1;
    }
    if tile_cols_log2 > 0 || tile_rows_log2 > 0 {
        w.write_bits(
            u32::from(frame.context_update_tile_id),
            (tile_rows_log2 + tile_cols_log2) as u8,
        );
        w.write_bits(u32::from(frame.tile_size_bytes_minus_1 & 0x03), 2);
    }
    Ok(())
}

/// spec 5.9.7 quantization_params. `read_delta_q` is a presence bit plus
/// an su(1+6) value; None → 0 delta.
pub(super) fn write_quantization_params(
    w: &mut BitWriter,
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
) -> Result<(), Av1SynthError> {
    w.write_bits(u32::from(frame.base_q_idx), 8);
    write_delta_q(w, frame.delta_q_y_dc);
    if !seq.monochrome {
        if seq.separate_uv_deltas {
            w.write_flag(frame.diff_uv_delta);
        }
        write_delta_q(w, frame.delta_q_u_dc);
        write_delta_q(w, frame.delta_q_u_ac);
        if seq.separate_uv_deltas && frame.diff_uv_delta {
            write_delta_q(w, frame.delta_q_v_dc);
            write_delta_q(w, frame.delta_q_v_ac);
        }
    }
    w.write_flag(frame.using_qmatrix);
    if frame.using_qmatrix {
        w.write_bits(u32::from(frame.qm_y & 0x0f), 4);
        w.write_bits(u32::from(frame.qm_u & 0x0f), 4);
        if seq.separate_uv_deltas {
            w.write_bits(u32::from(frame.qm_v & 0x0f), 4);
        }
    }
    Ok(())
}

fn write_delta_q(w: &mut BitWriter, delta: Option<i8>) {
    w.write_flag(delta.is_some());
    if let Some(value) = delta {
        // su(1+6): 7-bit two's complement.
        w.write_bits((value as u8 & 0x7f) as u32, 7);
    }
}

/// spec 5.9.8 segmentation_params.
pub(super) fn write_segmentation_params(
    w: &mut BitWriter,
    frame: &FrameHeaderInput,
    primary_ref_none: bool,
) -> Result<(), Av1SynthError> {
    w.write_flag(frame.segmentation_enabled);
    if !frame.segmentation_enabled {
        return Ok(());
    }
    if primary_ref_none {
        // update_map = 1, temporal_update = 0, update_data = 1 implied.
    } else {
        w.write_flag(frame.segmentation_update_map);
        if frame.segmentation_update_map {
            w.write_flag(frame.segmentation_temporal_update);
        }
        w.write_flag(frame.segmentation_update_data);
    }
    let update_data = primary_ref_none || frame.segmentation_update_data;
    if update_data {
        for segment in 0..MAX_SEGMENTS {
            for feature in 0..SEG_LVL_MAX {
                let enabled = frame.seg_feature_enabled[segment][feature];
                w.write_flag(enabled);
                if enabled {
                    let bits = SEGMENTATION_FEATURE_BITS[feature];
                    if SEGMENTATION_FEATURE_SIGNED[feature] {
                        // su(1 + bits): a bits+1-wide two's complement.
                        write_su(w, frame.seg_feature_data[segment][feature], bits + 1);
                    } else if bits > 0 {
                        w.write_bits(frame.seg_feature_data[segment][feature] as u32, bits);
                    }
                }
            }
        }
    }
    Ok(())
}

/// spec 5.9.11 loop_filter_params. Skipped entirely (defaults) when the
/// frame is coded lossless or intrabc is allowed.
pub(super) fn write_loop_filter_params(
    w: &mut BitWriter,
    frame: &FrameHeaderInput,
    lossless: bool,
) -> Result<(), Av1SynthError> {
    if lossless || frame.allow_intrabc {
        return Ok(());
    }
    w.write_bits(u32::from(frame.loop_filter_level[0] & 0x3f), 6);
    w.write_bits(u32::from(frame.loop_filter_level[1] & 0x3f), 6);
    let chroma_levels = frame.loop_filter_level[0] != 0 || frame.loop_filter_level[1] != 0;
    if chroma_levels {
        w.write_bits(u32::from(frame.loop_filter_level[2] & 0x3f), 6);
        w.write_bits(u32::from(frame.loop_filter_level[3] & 0x3f), 6);
    }
    w.write_bits(u32::from(frame.loop_filter_sharpness & 0x07), 3);
    w.write_flag(frame.loop_filter_delta_enabled);
    if frame.loop_filter_delta_enabled {
        w.write_flag(frame.loop_filter_delta_update);
        if frame.loop_filter_delta_update {
            for delta in frame.loop_filter_ref_deltas {
                w.write_flag(delta.is_some());
                if let Some(value) = delta {
                    write_su(w, i32::from(value), 7);
                }
            }
            for delta in frame.loop_filter_mode_deltas {
                w.write_flag(delta.is_some());
                if let Some(value) = delta {
                    write_su(w, i32::from(value), 7);
                }
            }
        }
    }
    Ok(())
}

/// spec 5.9.12 cdef_params. Skipped (defaults) when lossless, intrabc,
/// or the sequence disabled CDEF.
pub(super) fn write_cdef_params(
    w: &mut BitWriter,
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
    lossless: bool,
) -> Result<(), Av1SynthError> {
    if lossless || frame.allow_intrabc || !seq.enable_cdef {
        return Ok(());
    }
    debug_assert!(frame.cdef_bits <= 3);
    if frame.cdef_bits > 3 {
        return Err(Av1SynthError::InvalidCdefBits);
    }
    w.write_bits(u32::from(frame.cdef_damping_minus_3 & 0x03), 2);
    w.write_bits(u32::from(frame.cdef_bits), 2);
    for i in 0..(1 << frame.cdef_bits) {
        w.write_bits(u32::from(frame.cdef_y_pri[i] & 0x0f), 4);
        w.write_bits(u32::from(frame.cdef_y_sec[i] & 0x03), 2);
        if !seq.monochrome {
            w.write_bits(u32::from(frame.cdef_uv_pri[i] & 0x0f), 4);
            w.write_bits(u32::from(frame.cdef_uv_sec[i] & 0x03), 2);
        }
    }
    Ok(())
}

/// spec 5.9.13 lr_params. Skipped (defaults) when all-lossless, intrabc,
/// or the sequence disabled restoration.
pub(super) fn write_lr_params(
    w: &mut BitWriter,
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
    lossless: bool,
) -> Result<(), Av1SynthError> {
    let all_lossless = lossless && !frame.use_superres;
    if all_lossless || frame.allow_intrabc || !seq.enable_restoration {
        return Ok(());
    }
    let planes: usize = if seq.monochrome { 1 } else { 3 };
    let mut uses_lr = false;
    let mut uses_chroma_lr = false;
    for plane in 0..planes {
        let lr_type = frame.lr_type[plane] & 0x03;
        w.write_bits(u32::from(lr_type), 2);
        if lr_type != 0 {
            uses_lr = true;
            if plane > 0 {
                uses_chroma_lr = true;
            }
        }
    }
    if uses_lr {
        if seq.use_128x128_superblock {
            // increment(1, 2): shift 1 emits 0, shift 2 emits 1.
            w.write_flag(frame.lr_unit_extra_shift);
        } else {
            w.write_flag(frame.lr_unit_shift);
            if frame.lr_unit_shift {
                w.write_flag(frame.lr_unit_extra_shift);
            }
        }
        if seq.subsampling_x && seq.subsampling_y && uses_chroma_lr {
            w.write_flag(frame.lr_uv_shift);
        }
    }
    Ok(())
}

/// Effective frame dimensions: overridden values when frame_size_override
/// is set, otherwise the sequence maximums.
fn frame_size(seq: &SequenceHeaderInput, frame: &FrameHeaderInput) -> (u32, u32) {
    if frame.frame_size_override_flag {
        (
            u32::from(frame.frame_width_minus_1) + 1,
            u32::from(frame.frame_height_minus_1) + 1,
        )
    } else {
        (seq.max_frame_width, seq.max_frame_height)
    }
}

pub(super) fn width_bits(seq: &SequenceHeaderInput) -> u8 {
    bits_needed(seq.max_frame_width.saturating_sub(1))
}

pub(super) fn height_bits(seq: &SequenceHeaderInput) -> u8 {
    bits_needed(seq.max_frame_height.saturating_sub(1))
}

fn bits_needed(value: u32) -> u8 {
    let bits = 32 - value.leading_zeros();
    bits.max(1) as u8
}

/// spec 5.9.3 tile_log2: smallest k with `blk_size << k >= target`.
pub(super) fn tile_log2(blk_size: u32, target: u32) -> u32 {
    let mut k = 0;
    while (blk_size << k) < target {
        k += 1;
    }
    k
}

/// spec get_relative_dist: signed modular distance between two order
/// hints under OrderHintBits wrap-around.
pub(super) fn get_relative_dist(a: u16, b: u16, order_hint_bits: u8) -> i32 {
    let bits = u32::from(order_hint_bits);
    let half = 1u32 << (bits - 1);
    let diff = u32::from(a).wrapping_sub(u32::from(b)) & ((half << 1) - 1);
    ((diff & (half - 1)) as i32) - ((diff & half) as i32)
}

/// spec 5.9.16 skip_mode_params: the skip_mode_present bit is only coded
/// when skipModeAllowed holds — the explicit reference list must span a
/// forward and a backward picture in order-hint distance, or two
/// forwards. Derived here from ref_frame_idx / ref_order_hint because VA
/// carries no allowed-ness flag.
pub(super) fn skip_mode_allowed(frame: &FrameHeaderInput, seq: &SequenceHeaderInput) -> bool {
    if frame.frame_type == FrameType::Key || frame.frame_type == FrameType::IntraOnly {
        return false;
    }
    if !frame.reference_select || !seq.enable_order_hint {
        return false;
    }
    let bits = seq.order_hint_bits_minus_1 + 1;
    let mut forward_hint: Option<u16> = None;
    let mut backward_hint: Option<u16> = None;
    for idx in frame.ref_frame_idx {
        let ref_hint = frame.ref_order_hint[idx as usize];
        let dist = get_relative_dist(ref_hint, frame.order_hint, bits);
        if dist < 0 {
            // Track the latest forward hint (smallest backward distance).
            let better = match forward_hint {
                None => true,
                Some(prev) => get_relative_dist(ref_hint, prev, bits) > 0,
            };
            if better {
                forward_hint = Some(ref_hint);
            }
        } else if dist > 0 {
            // Track the earliest backward hint (smallest forward distance).
            let better = match backward_hint {
                None => true,
                Some(prev) => get_relative_dist(ref_hint, prev, bits) < 0,
            };
            if better {
                backward_hint = Some(ref_hint);
            }
        }
    }
    let Some(forward) = forward_hint else {
        return false;
    };
    if backward_hint.is_some() {
        return true;
    }
    // No backward reference: a second forward strictly behind the first
    // also allows skip mode.
    frame.ref_frame_idx.iter().any(|idx| {
        let ref_hint = frame.ref_order_hint[*idx as usize];
        get_relative_dist(ref_hint, forward, bits) < 0
    })
}

/// CodedLossless (spec 5.9.1): every segment's qindex is 0 and all
/// quantizer deltas are absent.
pub(super) fn coded_lossless(frame: &FrameHeaderInput) -> bool {
    if frame.base_q_idx != 0 {
        return false;
    }
    let deltas_absent = frame.delta_q_y_dc.is_none()
        && frame.delta_q_u_dc.is_none()
        && frame.delta_q_u_ac.is_none()
        && frame.delta_q_v_dc.is_none()
        && frame.delta_q_v_ac.is_none();
    if !deltas_absent {
        return false;
    }
    for segment in 0..MAX_SEGMENTS {
        let mut qindex = frame.base_q_idx;
        if frame.segmentation_enabled && frame.seg_feature_enabled[segment][SEG_LVL_ALT_Q] {
            qindex = (i32::from(frame.base_q_idx) + frame.seg_feature_data[segment][SEG_LVL_ALT_Q])
                .clamp(0, 255) as u8;
        }
        if qindex != 0 {
            return false;
        }
    }
    true
}

/// Two's-complement bit pattern for the low `bits` of a signed value,
/// written MSB-first (spec `su(n)`).
fn write_su(w: &mut BitWriter, value: i32, bits: u8) {
    debug_assert!(bits > 0 && bits <= 31);
    let mask = (1u32 << bits) - 1;
    w.write_bits((value as u32) & mask, bits);
}
