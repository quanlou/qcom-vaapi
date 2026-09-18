use crate::bindings::{
    VAIQMatrixBufferH264, VAPictureParameterBufferH264, VAProfile, VASliceParameterBufferH264,
};

mod bitstream;

use bitstream::{BitWriter, START_CODE, emit_nal};

const MAX_ASSEMBLED_FRAME_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub(crate) struct H264Slice {
    pub(crate) sp: VASliceParameterBufferH264,
    pub(crate) data: Vec<u8>,
}

#[derive(Clone)]
pub(crate) struct H264Synth {
    pub(crate) profile: VAProfile,
    pub(crate) have_pp: bool,
    pub(crate) pp: VAPictureParameterBufferH264,
    pub(crate) have_iq: bool,
    pub(crate) iq: VAIQMatrixBufferH264,
    last_sps: Vec<u8>,
    last_pps: Vec<u8>,
    emitted_any: bool,
    force_emit: bool,
    saw_frame_num: bool,
    last_frame_num: u16,
}

pub(crate) struct AssembledFrame {
    pub(crate) bytes: Vec<u8>,
    pub(crate) emitted_headers: bool,
}

impl H264Synth {
    pub(crate) fn new(profile: VAProfile) -> Self {
        Self {
            profile,
            have_pp: false,
            pp: zeroed(),
            have_iq: false,
            iq: zeroed(),
            last_sps: Vec::new(),
            last_pps: Vec::new(),
            emitted_any: false,
            force_emit: true,
            saw_frame_num: false,
            last_frame_num: 0,
        }
    }

    pub(crate) fn begin_picture(&mut self) {
        self.have_pp = false;
        self.have_iq = false;
    }

    pub(crate) fn set_picture_params(&mut self, pp: VAPictureParameterBufferH264) {
        self.pp = pp;
        self.have_pp = true;
    }

    pub(crate) fn set_iq_matrix(&mut self, iq: VAIQMatrixBufferH264) {
        self.iq = iq;
        self.have_iq = true;
    }

    pub(crate) fn assemble_frame(&mut self, slices: &[H264Slice]) -> Option<AssembledFrame> {
        if !self.have_pp || slices.is_empty() {
            return None;
        }

        let sps = synth_sps(&self.pp, self.profile)?;
        let pps = synth_pps(&self.pp, &slices[0].sp, self.profile)?;
        let emit = !self.emitted_any
            || self.force_emit
            || self.last_sps != sps
            || self.last_pps != pps
            || (self.saw_frame_num && self.pp.frame_num == 0);

        let header_bytes = if emit {
            sps.len().checked_add(pps.len())?
        } else {
            0
        };
        let payload_bytes = slices.iter().try_fold(0_usize, |total, slice| {
            if slice.data.is_empty() {
                Some(total)
            } else {
                total
                    .checked_add(START_CODE.len())?
                    .checked_add(slice.data.len())
            }
        })?;
        let total_bytes = header_bytes.checked_add(payload_bytes)?;
        if total_bytes > MAX_ASSEMBLED_FRAME_BYTES {
            return None;
        }
        let mut bytes = Vec::with_capacity(total_bytes);

        if emit {
            bytes.extend_from_slice(&sps);
            bytes.extend_from_slice(&pps);
            self.last_sps = sps;
            self.last_pps = pps;
            self.emitted_any = true;
            self.force_emit = false;
            self.saw_frame_num = true;
            self.last_frame_num = self.pp.frame_num;
        }

        for slice in slices {
            if slice.data.is_empty() {
                continue;
            }
            bytes.extend_from_slice(&START_CODE);
            bytes.extend_from_slice(&slice.data);
        }

        Some(AssembledFrame {
            bytes,
            emitted_headers: emit,
        })
    }

    /// Last synthesized SPS+PPS with start codes, for consumers that must
    /// re-feed headers to a fresh V4L2 session (e.g. after a mid-stream
    /// session rebuild). Empty until the first successful `assemble_frame`.
    pub(crate) fn header_bytes(&self) -> Vec<u8> {
        let mut v = self.last_sps.clone();
        v.extend_from_slice(&self.last_pps);
        v
    }
}

fn zeroed<T>() -> T {
    unsafe { std::mem::zeroed() }
}

fn profile_has_chroma_ext(profile_idc: u8) -> bool {
    matches!(
        profile_idc,
        100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128 | 138 | 139 | 134 | 135
    )
}

fn profile_to_idc(profile: VAProfile) -> u8 {
    match profile {
        VAProfile::VAProfileH264ConstrainedBaseline => 66,
        VAProfile::VAProfileH264Main => 77,
        VAProfile::VAProfileH264High => 100,
        _ => 77,
    }
}

/// Pick the smallest common H.264 level that can describe this picture.
///
/// The stateful Iris firmware accepts an over-specified SPS far enough to emit
/// SOURCE_CHANGE, but repeated-session testing showed it can then remain silent
/// before producing any decoded CAPTURE frame. Native `h264_v4l2m2m` feeds the
/// stream's original level; VA decode parameters do not carry that byte, so use
/// the level table instead of the old prototype shortcut that made all 720p
/// streams level 4.2.
fn pick_level_idc(pp: &VAPictureParameterBufferH264) -> u8 {
    let mb_w = u32::from(pp.picture_width_in_mbs_minus1) + 1;
    let mb_h = u32::from(pp.picture_height_in_mbs_minus1) + 1;
    let frame_mbs = mb_w.saturating_mul(mb_h);
    match frame_mbs {
        0..=99 => 10,
        100..=396 => 13,
        397..=792 => 21,
        793..=1620 => 22,
        1621..=3600 => 31,
        3601..=5120 => 32,
        5121..=8192 => 40,
        8193..=8704 => 42,
        _ => 51,
    }
}

fn constraint_flags(profile_idc: u8) -> u8 {
    match profile_idc {
        66 => 0xC0,
        77 => 0x40,
        _ => 0x00,
    }
}

fn write_sps_rbsp(
    bw: &mut BitWriter,
    pp: &VAPictureParameterBufferH264,
    profile: VAProfile,
) -> bool {
    let seq = unsafe { pp.seq_fields.bits };
    let profile_idc = profile_to_idc(profile);
    let level_idc = pick_level_idc(pp);
    let frame_mbs_only = seq.frame_mbs_only_flag();
    let poc_type = seq.pic_order_cnt_type();
    bw.put(u32::from(profile_idc), 8);
    bw.put(u32::from(constraint_flags(profile_idc)), 8);
    bw.put(u32::from(level_idc), 8);
    bw.put_ue(0);

    if profile_has_chroma_ext(profile_idc) {
        bw.put_ue(seq.chroma_format_idc());
        if seq.chroma_format_idc() == 3 {
            bw.put(seq.residual_colour_transform_flag(), 1);
        }
        bw.put_ue(u32::from(pp.bit_depth_luma_minus8));
        bw.put_ue(u32::from(pp.bit_depth_chroma_minus8));
        bw.put(0, 1);
        bw.put(0, 1);
    }

    bw.put_ue(seq.log2_max_frame_num_minus4());
    bw.put_ue(poc_type);
    if poc_type == 0 {
        bw.put_ue(seq.log2_max_pic_order_cnt_lsb_minus4());
    } else if poc_type == 1 {
        bw.put(seq.delta_pic_order_always_zero_flag(), 1);
        bw.put_se(0);
        bw.put_se(0);
        if seq.delta_pic_order_always_zero_flag() == 0 {
            bw.put_ue(1);
            bw.put_se(0);
        }
    }

    bw.put_ue(u32::from(pp.num_ref_frames));
    bw.put(seq.gaps_in_frame_num_value_allowed_flag(), 1);
    bw.put_ue(u32::from(pp.picture_width_in_mbs_minus1));
    bw.put_ue(u32::from(pp.picture_height_in_mbs_minus1) / (2 - frame_mbs_only));
    bw.put(frame_mbs_only, 1);
    if frame_mbs_only == 0 {
        bw.put(seq.mb_adaptive_frame_field_flag(), 1);
    }
    bw.put(seq.direct_8x8_inference_flag(), 1);
    bw.put(0, 1);

    bw.put(1, 1); // vui_parameters_present_flag
    bw.put(1, 1); // aspect_ratio_info_present_flag
    bw.put(1, 8); // aspect_ratio_idc: square pixels
    bw.put(0, 1); // overscan_info_present_flag
    bw.put(0, 1); // video_signal_type_present_flag
    bw.put(0, 1); // chroma_loc_info_present_flag
    bw.put(1, 1); // timing_info_present_flag
    bw.put(1, 32); // num_units_in_tick
    bw.put(60, 32); // time_scale: 30 fps when fixed_frame_rate_flag is 1
    bw.put(0, 1); // fixed_frame_rate_flag
    bw.put(0, 1); // nal_hrd_parameters_present_flag
    bw.put(0, 1); // vcl_hrd_parameters_present_flag
    bw.put(0, 1); // pic_struct_present_flag
    bw.put(1, 1); // bitstream_restriction_flag
    bw.put(1, 1); // motion_vectors_over_pic_boundaries_flag
    bw.put_ue(0); // max_bytes_per_pic_denom
    bw.put_ue(0); // max_bits_per_mb_denom
    bw.put_ue(11); // log2_max_mv_length_horizontal
    bw.put_ue(11); // log2_max_mv_length_vertical
    bw.put_ue(u32::from(pp.num_ref_frames.saturating_sub(2))); // max_num_reorder_frames
    bw.put_ue(u32::from(pp.num_ref_frames)); // max_dec_frame_buffering

    bw.rbsp_trailing()
}

fn write_pps_rbsp(
    bw: &mut BitWriter,
    pp: &VAPictureParameterBufferH264,
    sp: &VASliceParameterBufferH264,
    profile: VAProfile,
) -> bool {
    let pic = unsafe { pp.pic_fields.bits };
    let profile_idc = profile_to_idc(profile);
    let mut l0_default = sp.num_ref_idx_l0_active_minus1;
    if pp.num_ref_frames >= 3 {
        l0_default = 2;
    } else if pp.num_ref_frames > 0 && l0_default >= pp.num_ref_frames {
        l0_default = pp.num_ref_frames - 1;
    }

    bw.put_ue(0);
    bw.put_ue(0);
    bw.put(pic.entropy_coding_mode_flag(), 1);
    bw.put(pic.pic_order_present_flag(), 1);
    bw.put_ue(0);
    bw.put_ue(u32::from(l0_default));
    bw.put_ue(u32::from(sp.num_ref_idx_l1_active_minus1));
    bw.put(pic.weighted_pred_flag(), 1);
    bw.put(pic.weighted_bipred_idc(), 2);
    bw.put_se(i32::from(pp.pic_init_qp_minus26));
    bw.put_se(i32::from(pp.pic_init_qs_minus26));
    bw.put_se(i32::from(pp.chroma_qp_index_offset));
    bw.put(pic.deblocking_filter_control_present_flag(), 1);
    bw.put(pic.constrained_intra_pred_flag(), 1);
    bw.put(pic.redundant_pic_cnt_present_flag(), 1);

    if profile_has_chroma_ext(profile_idc) {
        bw.put(pic.transform_8x8_mode_flag(), 1);
        bw.put(0, 1);
        bw.put_se(i32::from(pp.second_chroma_qp_index_offset));
    }

    bw.rbsp_trailing()
}

pub(crate) fn synth_sps(pp: &VAPictureParameterBufferH264, profile: VAProfile) -> Option<Vec<u8>> {
    let mut bw = BitWriter::new(256);
    write_sps_rbsp(&mut bw, pp, profile)
        .then(|| emit_nal(0x67, bw.bytes()))
        .flatten()
}

pub(crate) fn synth_pps(
    pp: &VAPictureParameterBufferH264,
    sp: &VASliceParameterBufferH264,
    profile: VAProfile,
) -> Option<Vec<u8>> {
    let mut bw = BitWriter::new(256);
    write_pps_rbsp(&mut bw, pp, sp, profile)
        .then(|| emit_nal(0x68, bw.bytes()))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bindings::{
        _VAPictureParameterBufferH264__bindgen_ty_1,
        _VAPictureParameterBufferH264__bindgen_ty_1__bindgen_ty_1,
        _VAPictureParameterBufferH264__bindgen_ty_2,
        _VAPictureParameterBufferH264__bindgen_ty_2__bindgen_ty_1,
    };

    fn set_common(mb_w: u16, mb_h: u16) -> VAPictureParameterBufferH264 {
        let mut pp: VAPictureParameterBufferH264 = zeroed();
        pp.picture_width_in_mbs_minus1 = mb_w - 1;
        pp.picture_height_in_mbs_minus1 = mb_h - 1;
        pp.bit_depth_luma_minus8 = 0;
        pp.bit_depth_chroma_minus8 = 0;
        pp.num_ref_frames = 4;
        let seq_bits = _VAPictureParameterBufferH264__bindgen_ty_1__bindgen_ty_1::new_bitfield_1(
            1, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0,
        );
        pp.seq_fields = _VAPictureParameterBufferH264__bindgen_ty_1 {
            bits: _VAPictureParameterBufferH264__bindgen_ty_1__bindgen_ty_1 {
                _bitfield_align_1: [],
                _bitfield_1: seq_bits,
                __bindgen_padding_0: 0,
            },
        };
        pp.pic_init_qp_minus26 = 0;
        pp.pic_init_qs_minus26 = 0;
        pp.chroma_qp_index_offset = 0;
        pp.second_chroma_qp_index_offset = 0;
        let pic_bits = _VAPictureParameterBufferH264__bindgen_ty_2__bindgen_ty_1::new_bitfield_1(
            1, 0, 0, 0, 0, 0, 0, 1, 0, 0,
        );
        pp.pic_fields = _VAPictureParameterBufferH264__bindgen_ty_2 {
            bits: _VAPictureParameterBufferH264__bindgen_ty_2__bindgen_ty_1 {
                _bitfield_align_1: [],
                _bitfield_1: pic_bits,
                __bindgen_padding_0: 0,
            },
        };
        pp.frame_num = 0;
        pp
    }

    fn slice() -> VASliceParameterBufferH264 {
        let mut sp: VASliceParameterBufferH264 = zeroed();
        sp.num_ref_idx_l0_active_minus1 = 0;
        sp.num_ref_idx_l1_active_minus1 = 0;
        sp
    }

    fn bytes(hex: &str) -> Vec<u8> {
        assert_eq!(hex.len() % 2, 0, "hex fixture must contain full bytes");
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    fn bytes_from_array<const N: usize>(data: [u8; N]) -> Vec<u8> {
        data.to_vec()
    }

    #[test]
    fn synthesizes_same_main_headers_as_c() {
        let pp = set_common(80, 45);
        let sp = slice();
        let mut out = synth_sps(&pp, VAProfile::VAProfileH264Main).unwrap();
        out.extend_from_slice(&synth_pps(&pp, &sp, VAProfile::VAProfileH264Main).unwrap());
        assert_eq!(
            out,
            bytes_from_array([
                0, 0, 0, 1, 103, 77, 64, 31, 242, 128, 160, 11, 118, 2, 32, 0, 0, 3, 0, 32, 0, 0,
                7, 129, 227, 6, 50, 192, 0, 0, 0, 1, 104, 235, 143, 32
            ])
        );
    }

    #[test]
    fn synthesizes_same_high_headers_as_c() {
        let mut pp = set_common(80, 45);
        let mut pic = unsafe { pp.pic_fields.bits };
        pic.set_transform_8x8_mode_flag(1);
        pic.set_weighted_pred_flag(1);
        pp.pic_fields = _VAPictureParameterBufferH264__bindgen_ty_2 { bits: pic };
        let sp = slice();
        let mut out = synth_sps(&pp, VAProfile::VAProfileH264High).unwrap();
        out.extend_from_slice(&synth_pps(&pp, &sp, VAProfile::VAProfileH264High).unwrap());
        assert_eq!(
            out,
            bytes_from_array([
                0, 0, 0, 1, 103, 100, 0, 31, 172, 229, 1, 64, 22, 236, 4, 64, 0, 0, 3, 0, 64, 0, 0,
                15, 3, 198, 12, 101, 128, 0, 0, 0, 1, 104, 235, 207, 44
            ])
        );
    }

    #[test]
    fn synthesizes_same_constrained_baseline_headers_as_c() {
        let mut pp = set_common(80, 45);
        let mut pic = unsafe { pp.pic_fields.bits };
        pic.set_entropy_coding_mode_flag(0);
        pp.pic_fields = _VAPictureParameterBufferH264__bindgen_ty_2 { bits: pic };
        let sp = slice();
        let mut out = synth_sps(&pp, VAProfile::VAProfileH264ConstrainedBaseline).unwrap();
        out.extend_from_slice(
            &synth_pps(&pp, &sp, VAProfile::VAProfileH264ConstrainedBaseline).unwrap(),
        );
        assert_eq!(
            out,
            bytes_from_array([
                0, 0, 0, 1, 103, 66, 192, 31, 242, 128, 160, 11, 118, 2, 32, 0, 0, 3, 0, 32, 0, 0,
                7, 129, 227, 6, 50, 192, 0, 0, 0, 1, 104, 203, 143, 32
            ])
        );
    }

    #[test]
    fn picks_size_appropriate_level_for_small_pictures() {
        // 320x240 = 20x15 = 300 MBs: declare level 1.3 (0x0d), not the
        // over-specified level 4.2 used by the old prototype.
        let pp = set_common(20, 15);
        let out = synth_sps(&pp, VAProfile::VAProfileH264ConstrainedBaseline).unwrap();
        // start code + nal byte 0x67, profile 0x42, constraints 0xc0, level
        assert_eq!(&out[4..8], &bytes("6742c00d"));
    }

    #[test]
    fn picks_level_51_for_4k_like_c() {
        let pp = set_common(256, 135);
        let sp = slice();
        let mut out = synth_sps(&pp, VAProfile::VAProfileH264Main).unwrap();
        out.extend_from_slice(&synth_pps(&pp, &sp, VAProfile::VAProfileH264Main).unwrap());
        assert_eq!(
            out,
            bytes_from_array([
                0, 0, 0, 1, 103, 77, 64, 51, 242, 128, 32, 0, 33, 246, 2, 32, 0, 0, 3, 0, 32, 0, 0,
                7, 129, 227, 6, 50, 192, 0, 0, 0, 1, 104, 235, 143, 32
            ])
        );
    }

    #[test]
    fn frame_assembly_emits_headers_once_then_on_frame_num_zero() {
        let pp = set_common(80, 45);
        let mut sp = slice();
        sp.slice_data_size = 3;
        let mut syn = H264Synth::new(VAProfile::VAProfileH264Main);
        syn.set_picture_params(pp);
        let slices = [H264Slice {
            sp,
            data: vec![0x65, 0x88, 0x84],
        }];
        let first = syn.assemble_frame(&slices).unwrap();
        assert!(first.emitted_headers);
        assert!(first.bytes.starts_with(&bytes("00000001674d401f")));
        let second = syn.assemble_frame(&slices).unwrap();
        assert!(second.emitted_headers);
    }

    #[test]
    fn header_bytes_replays_last_synthesized_headers() {
        let pp = set_common(80, 45);
        let mut sp = slice();
        sp.slice_data_size = 3;
        let mut syn = H264Synth::new(VAProfile::VAProfileH264Main);
        assert!(syn.header_bytes().is_empty());
        syn.set_picture_params(pp);
        let slices = [H264Slice {
            sp,
            data: vec![0x65, 0x88, 0x84],
        }];
        let frame = syn.assemble_frame(&slices).unwrap();
        let headers = syn.header_bytes();
        assert!(!headers.is_empty());
        // The stored headers must be exactly the prefix the assembler put in
        // front of the first frame, so a rebuilt V4L2 session sees the same
        // parameter sets.
        assert_eq!(&frame.bytes[..headers.len()], &headers[..]);
        assert!(headers.starts_with(&bytes("00000001674d401f")));
    }
}
