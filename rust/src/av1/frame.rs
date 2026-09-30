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

mod syntax;

use syntax::{
    coded_lossless, height_bits, skip_mode_allowed, width_bits, write_cdef_params,
    write_loop_filter_params, write_lr_params, write_quantization_params,
    write_segmentation_params, write_superres, write_tile_info,
};

#[cfg(test)]
mod tests;

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
///   FFmpeg supplies these values directly in VA *_delta_q fields
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
    if frame.allow_screen_content_tools && seq.seq_choose_integer_mv {
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
    let primary_ref_none =
        frame_is_intra || frame.error_resilient_mode || frame.primary_ref_frame == 7;

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
