//! Byte fixtures from independently encoded streams and syntax boundaries.

use super::syntax::{get_relative_dist, tile_log2};
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
fn chosen_integer_motion_codes_the_per_frame_flag() {
    let mut seq = sample_seq();
    let mut frame = sample_keyframe();
    frame.allow_screen_content_tools = true;
    frame.force_integer_mv = false;
    let zero = synthesize_uncompressed_header(&seq, &frame).unwrap();
    frame.force_integer_mv = true;
    let one = synthesize_uncompressed_header(&seq, &frame).unwrap();
    assert_ne!(zero, one);
    seq.seq_choose_integer_mv = false;
    let implied_one = synthesize_uncompressed_header(&seq, &frame).unwrap();
    frame.force_integer_mv = false;
    let implied_zero = synthesize_uncompressed_header(&seq, &frame).unwrap();
    assert_eq!(implied_one, implied_zero);
}

#[test]
fn restoration_unit_shift_uses_superblock_specific_minimum() {
    let mut seq = sample_seq();
    let mut frame = sample_keyframe();
    frame.lr_type = [1, 0, 0];
    for (sb128, shift, extra, expected) in [
        (false, false, false, 0x40),
        (false, true, false, 0x42),
        (false, true, true, 0x43),
        (true, true, false, 0x40),
        (true, true, true, 0x42),
    ] {
        seq.use_128x128_superblock = sb128;
        frame.lr_unit_shift = shift;
        frame.lr_unit_extra_shift = extra;
        let mut writer = BitWriter::new();
        write_lr_params(&mut writer, &seq, &frame, false).unwrap();
        assert_eq!(writer.finish(), [expected], "sb128={sb128} extra={extra}");
    }
}

#[test]
fn keyframe_header_matches_real_sample_bytes() {
    const REAL_HEADER: [u8; 22] = [
        0x10, 0x00, 0x83, 0x40, 0x80, 0x41, 0x00, 0x00, 0x30, 0x80, 0xf8, 0x0f, 0xbc, 0xf0, 0x00,
        0x00, 0x03, 0xc0, 0x00, 0x00, 0x00, 0x20,
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
        0x28, 0x20, 0x00, 0x24, 0x92, 0x49, 0x1d, 0x09, 0xc1, 0x02, 0x8a, 0x00, 0x00, 0x61, 0x79,
        0xf7, 0x81, 0x01, 0xf0, 0x00, 0x01, 0xe7, 0x80, 0x79, 0xe0, 0x00, 0x70, 0x00,
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
        0x12, 0x00, 0x0a, 0x0b, 0x00, 0x00, 0x00, 0x2d, 0x4c, 0xff, 0xb3, 0xc6, 0xaf, 0x98, 0x24,
        0x32, 0xc4, 0xf7, 0x01, 0x10, 0x00, 0x83, 0x40, 0x80, 0x41, 0x00, 0x00, 0x30, 0x80, 0xf8,
        0x0f, 0xbc, 0xf0, 0x00, 0x00, 0x03, 0xc0, 0x00, 0x00, 0x00, 0x20,
    ];
    let seq = sample_seq();
    let header =
        synthesize_uncompressed_header(&seq, &sample_keyframe()).expect("keyframe must synthesize");
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
    let lossless = synthesize_uncompressed_header(&sample_seq(), &frame).expect("lossless frame");
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
