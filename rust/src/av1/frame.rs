//! AV1 uncompressed_header (Frame OBU) syntax writer.
//!
//! Reference: AV1 Bitstream & Decoding Process Specification 5.9.1
//! (`uncompressed_header`) and the helpers it calls: 5.9.3 `tile_info`,
//! 5.9.7 `quantization_params`, 5.9.8 `segmentation_params`,
//! 5.9.9/5.9.10 delta-q/lf params, 5.9.11 `loop_filter_params`,
//! 5.9.12 `cdef_params`, 5.9.13 `lr_params`, 5.9.14 `read_tx_mode`,
//! 5.9.15 `frame_reference_mode`, 5.9.16 `skip_mode_params`,
//! 5.9.17 `global_motion_params`, 5.9.18 `film_grain_params`.
//!
//! Every syntax element comes from `FrameHeaderInput` (caller maps
//! `VADecPictureParameterBufferAV1`) plus the sequence-level
//! `SequenceHeaderInput` that governs conditional fields, so the writer
//! never invents values. Byte-exactness is pinned by round-trip tests
//! against two frames of the real libsvtav1 sample
//! (/home/mq/tmp/vaatest/codec5/av1-720p.mp4): the keyframe's 22-byte
//! header and the first inter frame's 28-byte header.
//!
//! Deliberately unsupported paths return `Av1SynthError` instead of
//! guessing: show-existing frames, SWITCH frames, frame_size_with_refs
//! (per-ref size inheritance), non-uniform tile spacing, non-identity
//! global motion, and applied film grain.

use super::bitstream::{BitWriter, ObuType, ObuWriter};
use super::synth::SequenceHeaderInput;

/// AV1 frame_type values (spec 6.8.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum FrameType {
    Key = 0,
    Inter = 1,
    IntraOnly = 2,
}

/// Failure modes of the frame header writer. Returned instead of
/// silently emitting a stream the hardware would reject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Av1SynthError {
    /// SWITCH frames (frame_type 3) are never synthesised.
    UnsupportedSwitchFrame,
    /// `frame_size_with_refs()` needs per-ref `found_ref` bits that VA
    /// does not carry; a size-overridden inter frame must instead use
    /// error_resilient_mode.
    UnsupportedFrameSizeWithRefs,
    /// Only uniform_tile_spacing_flag = 1 is derived (VA's tile_cols /
    /// tile_rows are power-of-two counts under uniform spacing).
    UnsupportedNonUniformTiles,
    /// Only identity global motion (is_global = 0 for all refs) is
    /// written; the subexp parameter syntax is not implemented.
    UnsupportedGlobalMotion,
    /// apply_grain = 1 streams need the full film grain payload.
    UnsupportedAppliedFilmGrain,
    /// tile_cols / tile_rows must be non-zero powers of two reachable
    /// from the spec's min/max tile log2 range.
    InvalidTileCount,
    /// cdef_bits must be 0..=3 (it indexes 1 << cdef_bits strength sets).
    InvalidCdefBits,
}

/// All uncompressed_header fields the writer needs, in raw bitstream
/// units. The caller populates this from `VADecPictureParameterBufferAV1`:
///
/// - pic_info_fields → frame_type / show_frame / showable_frame /
///   error_resilient_mode / disable_cdf_update /
///   allow_screen_content_tools / force_integer_mv /
///   allow_high_precision_mv / is_motion_mode_switchable /
///   use_ref_frame_mvs / disable_frame_end_update_cdf / allow_intrabc
/// - pp.frame_width_minus1 / frame_height_minus1 → the `*_minus_1` dims
///   (the caller derives frame_size_override_flag by comparing with the
///   sequence max dims)
/// - pp.order_hint / refresh_frame_flags / primary_ref_frame /
///   ref_frame_idx / ref_order_hint → same-named fields
/// - pp.interp_filter: 0..3 = explicit filter, 4 = SWITCHABLE
/// - pp.superres_scale_denominator minus 9 is the coded_denom value
/// - pp.tile_cols / tile_rows are counts (powers of two); VA carries no
///   tile_size_bytes_minus1, so the caller derives it from the largest
///   tile payload it will wrap
/// - quantizer deltas are raw su(7) values (None → delta_coded = 0);
///   note VA's *_delta_q fields are "value * 2" and need converting
/// - CDEF strengths unpack as `(packed >> 2, packed & 3)` from VA's
///   cdef_y_strengths[] / cdef_uv_strengths[]
#[derive(Clone, Debug)]
pub(crate) struct FrameHeaderInput {
    pub frame_type: FrameType,
    pub show_frame: bool,
    /// Only coded when `show_frame` is false.
    pub showable_frame: bool,
    /// Only coded when this is not a KEY frame with show_frame = 1 (and
    /// not a SWITCH frame) — those imply error resilience.
    pub error_resilient_mode: bool,
    pub disable_cdf_update: bool,
    /// Only coded when the sequence chose SELECT for
    /// screen-content-tools (always true for this driver's sequence
    /// headers).
    pub allow_screen_content_tools: bool,
    /// Only coded when screen-content-tools are allowed and the sequence
    /// chose SELECT for integer MV.
    pub force_integer_mv: bool,
    pub frame_size_override_flag: bool,
    pub frame_width_minus_1: u16,
    pub frame_height_minus_1: u16,
    /// f(OrderHintBits); not emitted when the sequence disabled order
    /// hints.
    pub order_hint: u16,
    /// f(3), only for non-intra, non-error-resilient frames.
    pub primary_ref_frame: u8,
    /// f(8), only when this is not an shown KEY/SWITCH frame (those
    /// refresh all slots implicitly).
    pub refresh_frame_flags: u8,
    /// Only emitted when (!intra || refresh != allFrames) with error
    /// resilience and order hints enabled; 8 × f(OrderHintBits).
    pub ref_order_hint: [u16; 8],
    pub use_superres: bool,
    /// `coded_denom` (denominator − 9), only when `use_superres`.
    pub superres_coded_denom: u8,
    pub render_and_frame_size_different: bool,
    pub render_width_minus_1: u16,
    pub render_height_minus_1: u16,
    /// Intra frames only, when screen-content-tools are allowed and no
    /// superres is applied.
    pub allow_intrabc: bool,
    /// Inter frames: 7 × f(3) into ref_frame_map.
    pub ref_frame_idx: [u8; 7],
    /// Inter frames only; skipped when force_integer_mv ends up set.
    pub allow_high_precision_mv: bool,
    pub is_filter_switchable: bool,
    /// f(2), only when `is_filter_switchable` is false.
    pub interpolation_filter: u8,
    pub is_motion_mode_switchable: bool,
    /// Inter frames only, when the sequence enabled ref frame MVs.
    pub use_ref_frame_mvs: bool,
    /// Only coded when `disable_cdf_update` is false (otherwise implied 1).
    pub disable_frame_end_update_cdf: bool,

    // tile_info (spec 5.9.3)
    pub uniform_tile_spacing: bool,
    /// Count of tile columns; must be a power of two.
    pub tile_cols: u8,
    /// Count of tile rows; must be a power of two.
    pub tile_rows: u8,
    /// Only coded when more than one tile exists.
    pub context_update_tile_id: u16,
    /// Only coded when more than one tile exists; VA does not carry it,
    /// so the caller derives it from the largest wrapped tile payload.
    pub tile_size_bytes_minus_1: u8,

    // quantization_params (spec 5.9.7)
    pub base_q_idx: u8,
    pub delta_q_y_dc: Option<i8>,
    pub delta_q_u_dc: Option<i8>,
    pub delta_q_u_ac: Option<i8>,
    /// Only coded when `diff_uv_delta` is set (which itself only exists
    /// with separate UV deltas).
    pub delta_q_v_dc: Option<i8>,
    pub delta_q_v_ac: Option<i8>,
    /// Only emitted for non-monochrome sequences with separate UV deltas.
    pub diff_uv_delta: bool,
    pub using_qmatrix: bool,
    pub qm_y: u8,
    pub qm_u: u8,
    /// Only coded with separate UV deltas; otherwise qm_v = qm_u.
    pub qm_v: u8,

    // segmentation_params (spec 5.9.8) — [segment][feature]
    pub segmentation_enabled: bool,
    /// Only coded when primary_ref_frame != PRIMARY_REF_NONE.
    pub segmentation_update_map: bool,
    /// Only coded when update_map is set (and primary ref is loaded).
    pub segmentation_temporal_update: bool,
    /// Only coded when primary_ref_frame != PRIMARY_REF_NONE.
    pub segmentation_update_data: bool,
    pub seg_feature_enabled: [[bool; SEG_LVL_MAX]; MAX_SEGMENTS],
    pub seg_feature_data: [[i32; SEG_LVL_MAX]; MAX_SEGMENTS],

    // delta_q_params / delta_lf_params (spec 5.9.9, 5.9.10)
    /// Only coded when base_q_idx > 0.
    pub delta_q_present: bool,
    pub delta_q_res: u8,
    /// Only coded when delta_q is present and intrabc is off.
    pub delta_lf_present: bool,
    pub delta_lf_res: u8,
    pub delta_lf_multi: bool,

    // loop_filter_params (spec 5.9.11) — levels 0..3 = Y0, Y1, U, V
    pub loop_filter_level: [u8; 4],
    pub loop_filter_sharpness: u8,
    pub loop_filter_delta_enabled: bool,
    /// Only coded when delta is enabled.
    pub loop_filter_delta_update: bool,
    /// su(1+6) values; None → no update bit for that slot.
    pub loop_filter_ref_deltas: [Option<i8>; TOTAL_REFS_PER_FRAME],
    pub loop_filter_mode_deltas: [Option<i8>; 2],

    // cdef_params (spec 5.9.12)
    pub cdef_damping_minus_3: u8,
    /// 0..=3; indexes how many strength sets follow.
    pub cdef_bits: u8,
    pub cdef_y_pri: [u8; 8],
    pub cdef_y_sec: [u8; 8],
    pub cdef_uv_pri: [u8; 8],
    pub cdef_uv_sec: [u8; 8],

    // lr_params (spec 5.9.13) — per-plane lr_type (0 = none)
    pub lr_type: [u8; 3],
    pub lr_unit_shift: bool,
    /// Only coded for 64x64 superblocks when unit shift is set.
    pub lr_unit_extra_shift: bool,
    /// Only coded for 4:2:0 when chroma restoration is used.
    pub lr_uv_shift: bool,

    // read_tx_mode / mode control
    pub tx_mode_select: bool,
    /// Inter frames only.
    pub reference_select: bool,
    /// Inter frames only; coded only when the spec's skipModeAllowed
    /// holds (derived from ref_frame_idx / ref_order_hint, since the
    /// bitstream omits it otherwise and the decoder implies 0).
    pub skip_mode_present: bool,
    /// Inter frames only, when error resilience is off and the sequence
    /// enabled warped motion.
    pub allow_warped_motion: bool,
    pub reduced_tx_set: bool,

    // global_motion_params (spec 5.9.17) — all entries must be false
    /// (identity) or the writer returns UnsupportedGlobalMotion.
    pub global_motion_is_global: [bool; 7],

    /// apply_grain from `VAFilmGrainStructAV1`; only read when the
    /// sequence signalled film_grain_params_present. True is rejected.
    pub film_grain_apply: bool,
}

const MAX_SEGMENTS: usize = 8;
const SEG_LVL_MAX: usize = 8;
const TOTAL_REFS_PER_FRAME: usize = 8;
const REF_FRAMES: usize = 8;
const REFS_PER_FRAME: usize = 7;
/// SEG_LVL_ALT_Q — the segmentation feature that overrides base_q_idx.
const SEG_LVL_ALT_Q: usize = 0;
/// Per-feature bit widths (spec 5.9.8 table).
const SEGMENTATION_FEATURE_BITS: [u8; SEG_LVL_MAX] = [8, 6, 6, 6, 6, 3, 0, 0];
/// Per-feature signedness (spec 5.9.8 table).
const SEGMENTATION_FEATURE_SIGNED: [bool; SEG_LVL_MAX] =
    [true, true, true, true, true, false, false, false];

const MAX_TILE_WIDTH: u32 = 4096;
const MAX_TILE_AREA: u32 = 4096 * 2304;
const MAX_TILE_COLS: u32 = 64;
const MAX_TILE_ROWS: u32 = 64;
/// allFrames = (1 << NUM_REF_FRAMES) − 1: refresh_frame_flags value that
/// marks every reference slot for refresh.
const ALL_FRAMES: u8 = 0xff;

/// Emit the uncompressed_header() payload, zero-padded to a byte
/// boundary per the spec's `byte_alignment()`. Wrap it with
/// `ObuWriter::wrap(ObuType::Frame, …)` after appending the tile group
/// data to form the Frame OBU, or use [`synthesize_frame_obu`] with the
/// tile data at hand.
pub(crate) fn synthesize_uncompressed_header(
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
) -> Result<Vec<u8>, Av1SynthError> {
    if frame.frame_type as u8 == 3 {
        return Err(Av1SynthError::UnsupportedSwitchFrame);
    }
    let mut w = BitWriter::new();
    write_uncompressed_header(&mut w, seq, frame)?;
    // uncompressed_header() does NOT end with trailing_bits(): frame_obu
    // (spec 5.9.2) follows it with byte_alignment(), whose padding bits
    // are zeros only. The trailing one-bit marker belongs exclusively to
    // OBUs that end at payload granularity (e.g. sequence_header_obu).
    w.align_to_byte();
    Ok(w.finish())
}

/// Emit a complete Frame OBU: uncompressed_header followed by
/// `tile_data` (the tile_group_obu body: per-tile sizes + payloads).
/// The OBU size prefix covers header + tiles.
pub(crate) fn synthesize_frame_obu(
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
    tile_data: &[u8],
) -> Result<Vec<u8>, Av1SynthError> {
    let payload = synthesize_uncompressed_header(seq, frame)?;
    let mut full = payload;
    full.extend_from_slice(tile_data);
    Ok(ObuWriter::wrap(ObuType::Frame, &full))
}

fn write_uncompressed_header(
    w: &mut BitWriter,
    seq: &SequenceHeaderInput,
    frame: &FrameHeaderInput,
) -> Result<(), Av1SynthError> {
    let frame_is_intra =
        frame.frame_type == FrameType::Key || frame.frame_type == FrameType::IntraOnly;
    // frame_id_numbers_present_flag = 0, decoder_model_info_present_flag
    // = 0, reduced_still_picture_header = 0 for this driver's streams, so
    // none of those branches emit bits.
    let key_showing = frame.frame_type == FrameType::Key && frame.show_frame;
    // show_existing_frame is always 0: the driver synthesises decode
    // pictures, never references back into the reference buffer.
    w.write_flag(false);
    w.write_bits(frame.frame_type as u32, 2);
    w.write_flag(frame.show_frame);
    if !frame.show_frame {
        w.write_flag(frame.showable_frame);
    }
    // SWITCH and shown KEY frames imply error_resilient_mode = 1.
    if !key_showing {
        w.write_flag(frame.error_resilient_mode);
    }
    w.write_flag(frame.disable_cdf_update);
    // seq_choose_screen_content_tools is pinned to 1 by our sequence
    // header writer, so seq_force_screen_content_tools == SELECT and the
    // per-frame flag is always coded.
    w.write_flag(frame.allow_screen_content_tools);
    if frame.allow_screen_content_tools && !seq.seq_choose_integer_mv {
        w.write_flag(frame.force_integer_mv);
    }
    // frame_id_numbers_present_flag = 0 → no current_frame_id.

    w.write_flag(frame.frame_size_override_flag);
    let order_hint_bits = if seq.enable_order_hint {
        seq.order_hint_bits_minus_1 + 1
    } else {
        0
    };
    w.write_bits(u32::from(frame.order_hint), order_hint_bits);
    if !frame_is_intra && !frame.error_resilient_mode {
        debug_assert!(frame.primary_ref_frame <= 7);
        w.write_bits(u32::from(frame.primary_ref_frame), 3);
    }
    // decoder_model_info_present_flag = 0 → no buffer_removal_time.

    if key_showing {
        // refresh_frame_flags = allFrames implied.
    } else {
        w.write_bits(u32::from(frame.refresh_frame_flags), 8);
    }
    if (!frame_is_intra || frame.refresh_frame_flags != ALL_FRAMES)
        && frame.error_resilient_mode
        && seq.enable_order_hint
    {
        for hint in frame.ref_order_hint {
            w.write_bits(u32::from(hint), order_hint_bits);
        }
    }

    if frame_is_intra {
        if frame.frame_size_override_flag {
            let wb = width_bits(seq);
            let hb = height_bits(seq);
            w.write_bits(u32::from(frame.frame_width_minus_1), wb);
            w.write_bits(u32::from(frame.frame_height_minus_1), hb);
        }
        if seq.enable_superres {
            write_superres(w, frame)?;
        }
        w.write_flag(frame.render_and_frame_size_different);
        if frame.render_and_frame_size_different {
            w.write_bits(u32::from(frame.render_width_minus_1), 16);
            w.write_bits(u32::from(frame.render_height_minus_1), 16);
        }
        if frame.allow_screen_content_tools && !frame.use_superres {
            w.write_flag(frame.allow_intrabc);
        }
    } else {
        // frame_refs_short_signaling is a coded shortcut VA never
        // requests; always emit 0 and then the explicit ref indices.
        if seq.enable_order_hint {
            w.write_flag(false);
        }
        for idx in frame.ref_frame_idx {
            debug_assert!(idx < REF_FRAMES as u8);
            w.write_bits(u32::from(idx), 3);
        }
        if frame.frame_size_override_flag && !frame.error_resilient_mode {
            // frame_size_with_refs(): no VA source for found_ref bits.
            return Err(Av1SynthError::UnsupportedFrameSizeWithRefs);
        }
        if frame.frame_size_override_flag {
            let wb = width_bits(seq);
            let hb = height_bits(seq);
            w.write_bits(u32::from(frame.frame_width_minus_1), wb);
            w.write_bits(u32::from(frame.frame_height_minus_1), hb);
        }
        if seq.enable_superres {
            write_superres(w, frame)?;
        }
        w.write_flag(frame.render_and_frame_size_different);
        if frame.render_and_frame_size_different {
            w.write_bits(u32::from(frame.render_width_minus_1), 16);
            w.write_bits(u32::from(frame.render_height_minus_1), 16);
        }
        let force_integer_mv = frame.force_integer_mv || frame_is_intra;
        if !force_integer_mv {
            w.write_flag(frame.allow_high_precision_mv);
        }
        w.write_flag(frame.is_filter_switchable);
        if !frame.is_filter_switchable {
            w.write_bits(u32::from(frame.interpolation_filter & 0x03), 2);
        }
        w.write_flag(frame.is_motion_mode_switchable);
        if !frame.error_resilient_mode && seq.enable_ref_frame_mvs {
            w.write_flag(frame.use_ref_frame_mvs);
        }
    }
    if !frame.disable_cdf_update {
        w.write_flag(frame.disable_frame_end_update_cdf);
    }

    write_tile_info(w, seq, frame)?;
    write_quantization_params(w, seq, frame)?;

    let lossless = coded_lossless(frame);
    let primary_ref_none = frame_is_intra || frame.error_resilient_mode;

    write_segmentation_params(w, frame, primary_ref_none)?;
    if frame.base_q_idx > 0 {
        w.write_flag(frame.delta_q_present);
    } else {
        debug_assert!(
            !frame.delta_q_present,
            "delta_q_present requires base_q_idx > 0"
        );
    }
    if frame.delta_q_present {
        w.write_bits(u32::from(frame.delta_q_res & 0x03), 2);
    }
    if frame.delta_q_present && !frame.allow_intrabc {
        w.write_flag(frame.delta_lf_present);
        if frame.delta_lf_present {
            w.write_bits(u32::from(frame.delta_lf_res & 0x03), 2);
            w.write_flag(frame.delta_lf_multi);
        }
    }

    write_loop_filter_params(w, frame, lossless)?;
    write_cdef_params(w, seq, frame, lossless)?;
    write_lr_params(w, seq, frame, lossless)?;

    if !lossless {
        w.write_flag(frame.tx_mode_select);
    }
    if !frame_is_intra {
        w.write_flag(frame.reference_select);
        let skip_allowed = skip_mode_allowed(frame, seq);
        if skip_allowed || frame.skip_mode_present {
            w.write_flag(frame.skip_mode_present);
        }
        if !frame.error_resilient_mode && seq.enable_warped_motion {
            w.write_flag(frame.allow_warped_motion);
        }
    }
    w.write_flag(frame.reduced_tx_set);
    if !frame_is_intra {
        for is_global in frame.global_motion_is_global {
            if is_global {
                return Err(Av1SynthError::UnsupportedGlobalMotion);
            }
            w.write_flag(false);
        }
    }
    if seq.film_grain_params_present && (frame.show_frame || frame.showable_frame) {
        // apply_grain must be 0; applied grain is not supported.
        if frame.film_grain_apply {
            return Err(Av1SynthError::UnsupportedAppliedFilmGrain);
        }
        w.write_flag(false);
    }
    Ok(())
}

fn write_superres(w: &mut BitWriter, frame: &FrameHeaderInput) -> Result<(), Av1SynthError> {
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
fn write_tile_info(
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
fn write_quantization_params(
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
fn write_segmentation_params(
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
fn write_loop_filter_params(
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
fn write_cdef_params(
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
fn write_lr_params(
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
        w.write_flag(frame.lr_unit_shift);
        if seq.use_128x128_superblock {
            // lr_unit_shift is bumped to 1 unconditionally; no extra bit.
        } else if frame.lr_unit_shift {
            w.write_flag(frame.lr_unit_extra_shift);
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

fn width_bits(seq: &SequenceHeaderInput) -> u8 {
    bits_needed(seq.max_frame_width.saturating_sub(1))
}

fn height_bits(seq: &SequenceHeaderInput) -> u8 {
    bits_needed(seq.max_frame_height.saturating_sub(1))
}

fn bits_needed(value: u32) -> u8 {
    let bits = 32 - value.leading_zeros();
    bits.max(1) as u8
}

/// spec 5.9.3 tile_log2: smallest k with `blk_size << k >= target`.
fn tile_log2(blk_size: u32, target: u32) -> u32 {
    let mut k = 0;
    while (blk_size << k) < target {
        k += 1;
    }
    k
}

/// spec get_relative_dist: signed modular distance between two order
/// hints under OrderHintBits wrap-around.
fn get_relative_dist(a: u16, b: u16, order_hint_bits: u8) -> i32 {
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
fn skip_mode_allowed(frame: &FrameHeaderInput, seq: &SequenceHeaderInput) -> bool {
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
fn coded_lossless(frame: &FrameHeaderInput) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The sequence header of the reference sample, matching the
    /// byte-exact fixture in `synth.rs`.
    fn sample_seq() -> SequenceHeaderInput {
        SequenceHeaderInput {
            seq_profile: super::super::synth::SeqProfile::Main,
            seq_level_idx_0: 5,
            seq_tier_0: false,
            max_frame_width: 1280,
            max_frame_height: 720,
            use_128x128_superblock: false,
            enable_filter_intra: false,
            enable_intra_edge_filter: true,
            enable_interintra_compound: true,
            enable_masked_compound: false,
            enable_warped_motion: true,
            enable_dual_filter: false,
            enable_order_hint: true,
            enable_jnt_comp: false,
            enable_ref_frame_mvs: true,
            order_hint_bits_minus_1: 6,
            enable_superres: false,
            enable_cdef: true,
            enable_restoration: true,
            seq_choose_integer_mv: true,
            seq_force_integer_mv: false,
            bit_depth: 8,
            monochrome: false,
            color_description: None,
            color_range: false,
            subsampling_x: true,
            subsampling_y: true,
            chroma_sample_position: 1,
            separate_uv_deltas: false,
            film_grain_params_present: false,
        }
    }

    /// Keyframe #1 of the real sample. Field values decoded from the
    /// actual 22 header bytes by the spec reference parser.
    fn sample_keyframe() -> FrameHeaderInput {
        FrameHeaderInput {
            frame_type: FrameType::Key,
            show_frame: true,
            showable_frame: false,
            error_resilient_mode: false, // implied 1, not coded
            disable_cdf_update: false,
            allow_screen_content_tools: false,
            force_integer_mv: false,
            frame_size_override_flag: false,
            frame_width_minus_1: 1279,
            frame_height_minus_1: 719,
            order_hint: 0,
            primary_ref_frame: 7,
            refresh_frame_flags: 255,
            ref_order_hint: [0; 8],
            use_superres: false,
            superres_coded_denom: 0,
            render_and_frame_size_different: false,
            render_width_minus_1: 1279,
            render_height_minus_1: 719,
            allow_intrabc: false,
            ref_frame_idx: [0; 7],
            allow_high_precision_mv: false,
            is_filter_switchable: true,
            interpolation_filter: 0,
            is_motion_mode_switchable: true,
            use_ref_frame_mvs: false,
            disable_frame_end_update_cdf: false,
            uniform_tile_spacing: true,
            tile_cols: 1,
            tile_rows: 1,
            context_update_tile_id: 0,
            tile_size_bytes_minus_1: 0,
            base_q_idx: 26,
            delta_q_y_dc: None,
            delta_q_u_dc: None,
            delta_q_u_ac: None,
            delta_q_v_dc: None,
            delta_q_v_ac: None,
            diff_uv_delta: false,
            using_qmatrix: false,
            qm_y: 0,
            qm_u: 0,
            qm_v: 0,
            segmentation_enabled: false,
            segmentation_update_map: false,
            segmentation_temporal_update: false,
            segmentation_update_data: false,
            seg_feature_enabled: [[false; SEG_LVL_MAX]; MAX_SEGMENTS],
            seg_feature_data: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
            delta_q_present: true,
            delta_q_res: 0,
            delta_lf_present: false,
            delta_lf_res: 0,
            delta_lf_multi: false,
            loop_filter_level: [1, 1, 0, 0],
            loop_filter_sharpness: 0,
            loop_filter_delta_enabled: false,
            loop_filter_delta_update: false,
            loop_filter_ref_deltas: [None; TOTAL_REFS_PER_FRAME],
            loop_filter_mode_deltas: [None; 2],
            cdef_damping_minus_3: 0,
            cdef_bits: 3,
            cdef_y_pri: [0, 15, 15, 15, 0, 0, 0, 0],
            cdef_y_sec: [2, 2, 2, 0, 0, 0, 0, 0],
            cdef_uv_pri: [0, 0, 15, 0, 0, 15, 0, 0],
            cdef_uv_sec: [0, 0, 0, 0, 0, 0, 0, 0],
            lr_type: [0, 0, 0],
            lr_unit_shift: false,
            lr_unit_extra_shift: false,
            lr_uv_shift: false,
            tx_mode_select: true,
            reference_select: false,
            skip_mode_present: false,
            allow_warped_motion: false,
            reduced_tx_set: false,
            global_motion_is_global: [false; 7],
            film_grain_apply: false,
        }
    }

    /// Inter frame #2 of the real sample (show_frame = 0, order_hint 32,
    /// refresh slot 0). Field values decoded from the actual 28 header
    /// bytes by the spec reference parser.
    fn sample_inter() -> FrameHeaderInput {
        FrameHeaderInput {
            frame_type: FrameType::Inter,
            show_frame: false,
            showable_frame: true,
            error_resilient_mode: false,
            disable_cdf_update: false,
            allow_screen_content_tools: false,
            force_integer_mv: false,
            frame_size_override_flag: false,
            frame_width_minus_1: 1279,
            frame_height_minus_1: 719,
            order_hint: 32,
            primary_ref_frame: 0,
            refresh_frame_flags: 1,
            ref_order_hint: [0; 8],
            use_superres: false,
            superres_coded_denom: 0,
            render_and_frame_size_different: false,
            render_width_minus_1: 1279,
            render_height_minus_1: 719,
            allow_intrabc: false,
            ref_frame_idx: [2, 2, 2, 2, 2, 2, 2],
            allow_high_precision_mv: false,
            is_filter_switchable: true,
            interpolation_filter: 0,
            is_motion_mode_switchable: true,
            use_ref_frame_mvs: true,
            disable_frame_end_update_cdf: false,
            uniform_tile_spacing: true,
            tile_cols: 1,
            tile_rows: 1,
            context_update_tile_id: 0,
            tile_size_bytes_minus_1: 0,
            base_q_idx: 39,
            delta_q_y_dc: None,
            delta_q_u_dc: None,
            delta_q_u_ac: None,
            delta_q_v_dc: None,
            delta_q_v_ac: None,
            diff_uv_delta: false,
            using_qmatrix: false,
            qm_y: 0,
            qm_u: 0,
            qm_v: 0,
            segmentation_enabled: false,
            segmentation_update_map: false,
            segmentation_temporal_update: false,
            segmentation_update_data: false,
            seg_feature_enabled: [[false; SEG_LVL_MAX]; MAX_SEGMENTS],
            seg_feature_data: [[0; SEG_LVL_MAX]; MAX_SEGMENTS],
            delta_q_present: true,
            delta_q_res: 0,
            delta_lf_present: false,
            delta_lf_res: 0,
            delta_lf_multi: false,
            loop_filter_level: [5, 5, 0, 0],
            loop_filter_sharpness: 0,
            loop_filter_delta_enabled: false,
            loop_filter_delta_update: false,
            loop_filter_ref_deltas: [None; TOTAL_REFS_PER_FRAME],
            loop_filter_mode_deltas: [None; 2],
            cdef_damping_minus_3: 0,
            cdef_bits: 3,
            cdef_y_pri: [0, 15, 0, 15, 0, 15, 0, 15],
            cdef_y_sec: [2, 2, 2, 2, 0, 0, 0, 0],
            cdef_uv_pri: [15, 15, 0, 0, 0, 15, 15, 0],
            cdef_uv_sec: [0, 0, 0, 0, 0, 0, 0, 0],
            lr_type: [0, 0, 0],
            lr_unit_shift: false,
            lr_unit_extra_shift: false,
            lr_uv_shift: false,
            tx_mode_select: true,
            reference_select: true,
            // skip_mode_present is not coded in the real frame: every
            // ref points forward (allow_warped_motion = 1 is coded after
            // the omitted skip_mode_present bit).
            skip_mode_present: false,
            allow_warped_motion: true,
            reduced_tx_set: false,
            global_motion_is_global: [false; 7],
            film_grain_apply: false,
        }
    }

    /// Byte-exact: the keyframe header must equal the first 22 payload
    /// bytes of the real sample's Frame OBU (file offset 19..41, after
    /// TD + Sequence Header + Frame OBU header + LEB128 size).
    #[test]
    fn keyframe_header_matches_real_sample_bytes() {
        const REAL_HEADER: [u8; 22] = [
            0x10, 0x00, 0x83, 0x40, 0x80, 0x41, 0x00, 0x00, 0x30, 0x80, 0xf8, 0x0f, 0xbc, 0xf0,
            0x00, 0x00, 0x03, 0xc0, 0x00, 0x00, 0x00, 0x20,
        ];
        let out = synthesize_uncompressed_header(&sample_seq(), &sample_keyframe())
            .expect("keyframe must synthesize");
        assert_eq!(out, REAL_HEADER, "keyframe uncompressed_header mismatch");
    }

    /// Byte-exact: the first inter frame header must equal the real
    /// sample's 28 payload bytes (frame OBU #2).
    #[test]
    fn inter_header_matches_real_sample_bytes() {
        const REAL_HEADER: [u8; 28] = [
            0x28, 0x20, 0x00, 0x24, 0x92, 0x49, 0x1d, 0x09, 0xc1, 0x02, 0x8a, 0x00, 0x00, 0x61,
            0x79, 0xf7, 0x81, 0x01, 0xf0, 0x00, 0x01, 0xe7, 0x80, 0x79, 0xe0, 0x00, 0x70, 0x00,
        ];
        let out = synthesize_uncompressed_header(&sample_seq(), &sample_inter())
            .expect("inter frame must synthesize");
        assert_eq!(out, REAL_HEADER, "inter uncompressed_header mismatch");
    }

    /// The full access-unit prefix the driver must prepend to a
    /// keyframe's tile data: TD OBU || Sequence Header OBU || Frame OBU
    /// header + LEB128 size + uncompressed_header. The reference file's
    /// first 41 bytes end exactly where the first VA tile payload
    /// begins.
    #[test]
    fn access_unit_prefix_matches_real_sample_first_41_bytes() {
        const REAL_PREFIX: [u8; 41] = [
            0x12, 0x00, 0x0a, 0x0b, 0x00, 0x00, 0x00, 0x2d, 0x4c, 0xff, 0xb3, 0xc6, 0xaf, 0x98,
            0x24, 0x32, 0xc4, 0xf7, 0x01, 0x10, 0x00, 0x83, 0x40, 0x80, 0x41, 0x00, 0x00, 0x30,
            0x80, 0xf8, 0x0f, 0xbc, 0xf0, 0x00, 0x00, 0x03, 0xc0, 0x00, 0x00, 0x00, 0x20,
        ];
        let seq = sample_seq();
        let header = synthesize_uncompressed_header(&seq, &sample_keyframe())
            .expect("keyframe must synthesize");
        // The real Frame OBU payload is 31684 bytes: header + tile data.
        let mut payload = Vec::with_capacity(31684);
        payload.extend_from_slice(&header);
        payload.resize(31684, 0);
        let frame_obu = ObuWriter::wrap(ObuType::Frame, &payload);
        let mut access_unit = Vec::new();
        access_unit.extend_from_slice(&ObuWriter::temporal_delimiter());
        access_unit.extend_from_slice(&super::super::synth::synthesize_sequence_header(&seq));
        access_unit.extend_from_slice(&frame_obu[..]);
        assert_eq!(&access_unit[..41], &REAL_PREFIX[..]);
    }

    /// Tile increment derivation: switching the keyframe to a 2×1 tile
    /// layout must emit increment_tile_cols = 1 followed by the stop bit,
    /// then context_update_tile_id f(1) and tile_size_bytes_minus_1 f(2)
    /// — 4 more content bits than the single-tile header (the stop bit
    /// replaces the single-tile 0 increment), keeping the header at 22
    /// bytes.
    #[test]
    fn multi_tile_layout_adds_context_update_fields() {
        let mut frame = sample_keyframe();
        let single =
            synthesize_uncompressed_header(&sample_seq(), &frame).expect("single tile keyframe");
        assert_eq!(single.len(), 22);

        frame.tile_cols = 2;
        frame.context_update_tile_id = 0;
        frame.tile_size_bytes_minus_1 = 1;
        let multi = synthesize_uncompressed_header(&sample_seq(), &frame).expect("2-tile keyframe");
        // 172 content bits + 4 = 176 → exactly 22 bytes.
        assert_eq!(multi.len(), 22);
    }

    #[test]
    fn error_paths_cover_unsupported_features() {
        let seq = sample_seq();
        // Non-uniform tiles are rejected.
        let mut frame = sample_keyframe();
        frame.uniform_tile_spacing = false;
        assert_eq!(
            synthesize_uncompressed_header(&seq, &frame),
            Err(Av1SynthError::UnsupportedNonUniformTiles)
        );
        // Invalid tile counts are rejected.
        let mut frame = sample_keyframe();
        frame.tile_cols = 3;
        assert_eq!(
            synthesize_uncompressed_header(&seq, &frame),
            Err(Av1SynthError::InvalidTileCount)
        );
        // Non-identity global motion is rejected.
        let mut frame = sample_inter();
        frame.global_motion_is_global[0] = true;
        assert_eq!(
            synthesize_uncompressed_header(&seq, &frame),
            Err(Av1SynthError::UnsupportedGlobalMotion)
        );
        // Size-overridden inter frames without error resilience would
        // need frame_size_with_refs.
        let mut frame = sample_inter();
        frame.frame_size_override_flag = true;
        assert_eq!(
            synthesize_uncompressed_header(&seq, &frame),
            Err(Av1SynthError::UnsupportedFrameSizeWithRefs)
        );
        // Applied film grain is rejected when the sequence signals it.
        let mut seq_fg = sample_seq();
        seq_fg.film_grain_params_present = true;
        let mut frame = sample_keyframe();
        frame.film_grain_apply = true;
        assert_eq!(
            synthesize_uncompressed_header(&seq_fg, &frame),
            Err(Av1SynthError::UnsupportedAppliedFilmGrain)
        );
    }

    /// Lossless frames (qindex 0 everywhere, no deltas) must skip the
    /// loop filter, CDEF, LR and tx_mode bits entirely.
    #[test]
    fn lossless_frame_skips_filter_syntax() {
        let mut frame = sample_keyframe();
        frame.base_q_idx = 0;
        frame.delta_q_present = false;
        let lossless =
            synthesize_uncompressed_header(&sample_seq(), &frame).expect("lossless frame");
        let lossy =
            synthesize_uncompressed_header(&sample_seq(), &sample_keyframe()).expect("lossy frame");
        assert!(
            lossless.len() < lossy.len(),
            "lossless header ({}) must be shorter than lossy ({})",
            lossless.len(),
            lossy.len()
        );
    }

    /// Delta-q with a signed value must emit the 7-bit two's complement.
    #[test]
    fn quantizer_deltas_emit_su7_values() {
        let mut frame = sample_keyframe();
        frame.delta_q_y_dc = Some(-64);
        let out = synthesize_uncompressed_header(&sample_seq(), &frame).expect("delta frame");
        // Baseline (no deltas) for comparison: the presence bit plus the
        // su(7) value add 8 bits, spilling the 172-bit header into one
        // extra byte.
        let base =
            synthesize_uncompressed_header(&sample_seq(), &sample_keyframe()).expect("base frame");
        assert_eq!(out.len(), base.len() + 1);
    }

    #[test]
    fn tile_log2_matches_spec_examples() {
        assert_eq!(tile_log2(1, 1), 0);
        assert_eq!(tile_log2(1, 2), 1);
        assert_eq!(tile_log2(1, 20), 5);
        assert_eq!(tile_log2(1, 64), 6);
        assert_eq!(tile_log2(64, 20), 0);
        assert_eq!(tile_log2(2304, 240), 0);
    }

    #[test]
    fn get_relative_dist_wraps_across_order_hint_boundary() {
        let bits = 7;
        assert_eq!(get_relative_dist(0, 32, bits), -32);
        assert_eq!(get_relative_dist(40, 32, bits), 8);
        assert_eq!(get_relative_dist(32, 40, bits), -8);
        // 120 wraps past the 7-bit maximum of 127.
        assert_eq!(get_relative_dist(120, 32, bits), -40);
        assert_eq!(get_relative_dist(5, 5, bits), 0);
    }

    /// The real inter fixture pins the not-allowed path (all refs
    /// forward → no skip_mode_present bit). This pins the allowed path:
    /// adding a backward reference (order hint ahead of the current
    /// frame) inserts exactly one bit before allow_warped_motion.
    #[test]
    fn skip_mode_present_coded_only_when_a_backward_ref_exists() {
        let seq = sample_seq();
        let mut frame = sample_inter();
        let not_allowed =
            synthesize_uncompressed_header(&seq, &frame).expect("forward-only inter frame");

        frame.skip_mode_present = true;
        frame.ref_frame_idx[1] = 3;
        frame.ref_order_hint[3] = 40; // ahead of order_hint 32 → backward
        let allowed =
            synthesize_uncompressed_header(&seq, &frame).expect("forward+backward inter frame");

        assert_eq!(allowed.len(), not_allowed.len());
        // tx=1, reference_select=1, skip_mode_present=1, allow_warped=1.
        assert_eq!(allowed[26], 0x78);
        assert_eq!(not_allowed[26], 0x70);
        assert_eq!(allowed[27], 0x00);
    }
}
