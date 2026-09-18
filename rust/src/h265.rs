//! H.265 (HEVC) bitstream plumbing skeleton — Phase 5 track C.
//!
//! Scope is deliberately narrow: NAL unit header parsing/classification,
//! minimal VPS/SPS/PPS parsing, and Annex-B access-unit assembly. Nothing
//! here is wired into a VA callback or V4L2 format setup yet (profile
//! advertisement lives in `config.rs` and is owned by another track); this
//! module only owns bitstream parsing/assembly and is validated by
//! synthetic-bitstream unit tests.
//!
//! Syntax reference: ITU-T H.265 v6+, clauses 7.3.2.1-7.3.2.3 and Annex B.

mod bitstream;

use bitstream::{BitReader, BitWriter, START_CODE, ebsp_to_rbsp, rbsp_to_ebsp};

/// 64 MiB ceiling on one assembled access unit, mirroring the H.264 frame
/// assembly guard in `h264.rs`.
const MAX_ASSEMBLED_ACCESS_UNIT_BYTES: usize = 64 * 1024 * 1024;

/// Parse/assembly failure. Every variant is a value error: malformed or
/// truncated input never panics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Error {
    /// Input ended before the syntax required.
    Truncated,
    /// `nal_forbidden_zero_bit` was set.
    ForbiddenZeroBit,
    /// `nuh_temporal_id_plus1` was zero, which the spec forbids.
    InvalidTemporalIdPlus1,
    /// Raw `00 00 00/01/02` inside a NAL payload — impossible in a valid
    /// Annex-B NAL unit.
    EmulationSequence,
    /// A field carried an impossible value (the payload names the field).
    OutOfRange(&'static str),
    /// A NAL unit type did not match the role requested during assembly.
    UnexpectedNalType,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Truncated => f.write_str("bitstream truncated"),
            Error::ForbiddenZeroBit => f.write_str("nal_forbidden_zero_bit set"),
            Error::InvalidTemporalIdPlus1 => f.write_str("nuh_temporal_id_plus1 is zero"),
            Error::EmulationSequence => {
                f.write_str("raw 00 00 00/01/02 sequence inside NAL payload")
            }
            Error::OutOfRange(what) => write!(f, "value out of range: {what}"),
            Error::UnexpectedNalType => f.write_str("NAL unit type does not match requested role"),
        }
    }
}

impl std::error::Error for Error {}

/// The two-byte NAL unit header (H.265 7.3.1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NalHeader {
    pub(crate) nal_unit_type: u8,
    pub(crate) nuh_layer_id: u8,
    pub(crate) nuh_temporal_id_plus1: u8,
}

impl NalHeader {
    fn write(&self, bw: &mut BitWriter) {
        debug_assert!(self.nal_unit_type <= 63 && self.nuh_layer_id <= 63);
        debug_assert!((1..=7).contains(&self.nuh_temporal_id_plus1));
        bw.put(0, 1); // forbidden_zero_bit
        bw.put(u64::from(self.nal_unit_type), 6);
        bw.put(u64::from(self.nuh_layer_id), 6);
        bw.put(u64::from(self.nuh_temporal_id_plus1), 3);
    }
}

/// Classification of the required NAL unit types (H.265 table 7-1). Every
/// other type is passed through as [`NalKind::Other`] instead of being
/// rejected, so unknown-but-legal streams still flow through the skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NalKind {
    TrailN,
    TrailR,
    IdrWRadl,
    IdrNlp,
    Cra,
    Vps,
    Sps,
    Pps,
    Other(u8),
}

impl NalKind {
    pub(crate) fn from_nal_unit_type(t: u8) -> Self {
        match t {
            0 => NalKind::TrailN,
            1 => NalKind::TrailR,
            19 => NalKind::IdrWRadl,
            20 => NalKind::IdrNlp,
            21 => NalKind::Cra,
            32 => NalKind::Vps,
            33 => NalKind::Sps,
            34 => NalKind::Pps,
            other => NalKind::Other(other),
        }
    }

    fn raw(&self) -> u8 {
        match self {
            NalKind::TrailN => 0,
            NalKind::TrailR => 1,
            NalKind::IdrWRadl => 19,
            NalKind::IdrNlp => 20,
            NalKind::Cra => 21,
            NalKind::Vps => 32,
            NalKind::Sps => 33,
            NalKind::Pps => 34,
            NalKind::Other(t) => *t,
        }
    }

    /// VCL NAL units (nal_unit_type 0..=23) carry slice data.
    pub(crate) fn is_slice(&self) -> bool {
        self.raw() <= 23
    }
}

/// One NAL unit: the parsed 2-byte header plus the EBSP payload that follows
/// it. The payload keeps its emulation-prevention bytes exactly as received,
/// so re-emitting the NAL into an Annex-B stream is a pure copy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Nal {
    pub(crate) header: NalHeader,
    pub(crate) payload_ebsp: Vec<u8>,
}

impl Nal {
    /// Parse a complete NAL unit in EBSP form (2-byte header plus escaped
    /// payload, without any start code).
    pub(crate) fn parse(nal: &[u8]) -> Result<Self, Error> {
        if nal.len() < 2 {
            return Err(Error::Truncated);
        }
        let mut br = BitReader::new(&nal[..2]);
        if br.get(1)? != 0 {
            return Err(Error::ForbiddenZeroBit);
        }
        let nal_unit_type = br.get(6)? as u8;
        let nuh_layer_id = br.get(6)? as u8;
        let nuh_temporal_id_plus1 = br.get(3)? as u8;
        if nuh_temporal_id_plus1 == 0 {
            return Err(Error::InvalidTemporalIdPlus1);
        }
        Ok(Self {
            header: NalHeader {
                nal_unit_type,
                nuh_layer_id,
                nuh_temporal_id_plus1,
            },
            payload_ebsp: nal[2..].to_vec(),
        })
    }

    /// Build a NAL unit from an RBSP payload, applying emulation prevention.
    pub(crate) fn build(
        nal_unit_type: u8,
        nuh_layer_id: u8,
        nuh_temporal_id_plus1: u8,
        rbsp: &[u8],
    ) -> Self {
        Self {
            header: NalHeader {
                nal_unit_type,
                nuh_layer_id,
                nuh_temporal_id_plus1,
            },
            payload_ebsp: rbsp_to_ebsp(rbsp),
        }
    }

    pub(crate) fn kind(&self) -> NalKind {
        NalKind::from_nal_unit_type(self.header.nal_unit_type)
    }

    /// RBSP payload with emulation prevention removed.
    pub(crate) fn rbsp(&self) -> Result<Vec<u8>, Error> {
        ebsp_to_rbsp(&self.payload_ebsp)
    }

    /// The two header bytes as they appear on the wire.
    pub(crate) fn header_bytes(&self) -> [u8; 2] {
        let mut bw = BitWriter::new();
        self.header.write(&mut bw);
        let bytes = bw.into_bytes();
        [bytes[0], bytes[1]]
    }

    /// This NAL as Annex-B bytes with a 3-byte `00 00 01` start code.
    pub(crate) fn annex_b(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(START_CODE.len() + 2 + self.payload_ebsp.len());
        out.extend_from_slice(&START_CODE);
        out.extend_from_slice(&self.header_bytes());
        out.extend_from_slice(&self.payload_ebsp);
        out
    }
}

/// Assemble one Annex-B access unit: optional `[VPS][SPS][PPS]` followed by
/// the slice NAL, each prefixed with a 3-byte `00 00 01` start code.
///
/// The inputs are already-EBSP NAL units, so emulation prevention is
/// preserved byte-for-byte; only start codes are added. Roles are validated
/// before any bytes are produced.
pub(crate) fn assemble_access_unit(
    vps: Option<&Nal>,
    sps: &Nal,
    pps: &Nal,
    slice: &Nal,
) -> Result<Vec<u8>, Error> {
    let role_ok = |nal: &Nal, want: NalKind| nal.kind() == want;
    if !vps.is_none_or(|n| role_ok(n, NalKind::Vps))
        || !role_ok(sps, NalKind::Sps)
        || !role_ok(pps, NalKind::Pps)
        || !slice.kind().is_slice()
    {
        return Err(Error::UnexpectedNalType);
    }

    let payloads = [
        vps.map_or(0, |n| n.payload_ebsp.len()),
        sps.payload_ebsp.len(),
        pps.payload_ebsp.len(),
        slice.payload_ebsp.len(),
    ];
    let mut total = 0usize;
    for len in payloads {
        total = total
            .checked_add(START_CODE.len() + 2)
            .and_then(|t| t.checked_add(len))
            .ok_or(Error::OutOfRange("access unit size"))?;
    }
    if total > MAX_ASSEMBLED_ACCESS_UNIT_BYTES {
        return Err(Error::OutOfRange("access unit too large"));
    }

    let mut out = Vec::with_capacity(total);
    for nal in [vps, Some(sps), Some(pps), Some(slice)]
        .into_iter()
        .flatten()
    {
        out.extend_from_slice(&nal.annex_b());
    }
    Ok(out)
}

/// Split an Annex-B byte stream into raw NAL units (header plus EBSP payload,
/// without start codes). Both 3- and 4-byte start codes are handled, and the
/// separator zero bytes in front of each start code are dropped.
///
/// Known limitation, acceptable for this skeleton: a NAL whose payload
/// legitimately ends in zero bytes (for example `cabac_zero_words`) loses
/// them to the separator trimming.
pub(crate) fn split_annex_b(buf: &[u8]) -> Vec<Vec<u8>> {
    let Some(first) = find_start_code(buf, 0) else {
        return Vec::new();
    };
    let mut nals = Vec::new();
    let mut pos = first + START_CODE.len();
    while let Some(sc) = find_start_code(buf, pos) {
        let mut end = sc;
        while end > pos && buf[end - 1] == 0 {
            end -= 1;
        }
        if end > pos {
            nals.push(buf[pos..end].to_vec());
        }
        pos = sc + START_CODE.len();
    }
    let mut end = buf.len();
    while end > pos && buf[end - 1] == 0 {
        end -= 1;
    }
    if end > pos {
        nals.push(buf[pos..end].to_vec());
    }
    nals
}

fn find_start_code(buf: &[u8], from: usize) -> Option<usize> {
    let last = buf.len().checked_sub(START_CODE.len())?;
    (from..=last).find(|&i| buf[i] == 0 && buf[i + 1] == 0 && buf[i + 2] == 1)
}

/// General-tier fields kept by the PTL parser; the per-sub-layer profile and
/// level copies are skipped, not retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProfileTierLevel {
    pub(crate) general_profile_space: u8,
    pub(crate) general_tier_flag: bool,
    pub(crate) general_profile_idc: u8,
    pub(crate) general_level_idc: u8,
}

/// Parse `profile_tier_level(profilePresentFlag=1, maxNumSubLayersMinus1)`.
///
/// The general-tier fields are kept. The sub-layer loop is applied exactly as
/// specified — present flags per sub-layer, the `reserved_zero_2bits` run for
/// unused indices, then the conditional 88-bit profile and 8-bit level
/// copies — so parsing stays in sync even for multi-sub-layer streams.
fn parse_profile_tier_level(
    br: &mut BitReader<'_>,
    max_sub_layers_minus1: u8,
) -> Result<ProfileTierLevel, Error> {
    let general_profile_space = br.get(2)? as u8;
    let general_tier_flag = br.get(1)? != 0;
    let general_profile_idc = br.get(5)? as u8;
    br.skip(32)?; // general_profile_compatibility_flag[0..32]
    br.skip(4)?; // progressive/interlaced/non_packed/frame_only constraint flags
    br.skip(44)?; // reserved_zero_43bits + inbld/reserved bit
    let general_level_idc = br.get(8)? as u8;

    let max = usize::from(max_sub_layers_minus1);
    let mut profile_present = [false; 7];
    let mut level_present = [false; 7];
    for i in 0..max {
        profile_present[i] = br.get(1)? != 0;
        level_present[i] = br.get(1)? != 0;
    }
    if max_sub_layers_minus1 > 0 {
        br.skip(2 * u32::from(8 - max_sub_layers_minus1))?; // reserved_zero_2bits
    }
    for present in profile_present.iter().take(max) {
        if *present {
            br.skip(88)?; // sub-layer copy of the general profile fields
        }
    }
    for present in level_present.iter().take(max) {
        if *present {
            br.skip(8)?; // sub_layer_level_idc
        }
    }
    Ok(ProfileTierLevel {
        general_profile_space,
        general_tier_flag,
        general_profile_idc,
        general_level_idc,
    })
}

/// Fields kept from a parsed `seq_parameter_set_rbsp()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SpsInfo {
    pub(crate) sps_video_parameter_set_id: u8,
    pub(crate) sps_max_sub_layers_minus1: u8,
    pub(crate) sps_temporal_id_nesting_flag: bool,
    pub(crate) ptl: ProfileTierLevel,
    pub(crate) sps_seq_parameter_set_id: u32,
    pub(crate) chroma_format_idc: u32,
    pub(crate) separate_colour_plane_flag: bool,
    pub(crate) pic_width_in_luma_samples: u32,
    pub(crate) pic_height_in_luma_samples: u32,
    pub(crate) conformance_window_flag: bool,
    pub(crate) bit_depth_luma_minus8: u32,
    pub(crate) bit_depth_chroma_minus8: u32,
    pub(crate) log2_max_pic_order_cnt_lsb_minus4: u32,
    pub(crate) sps_sub_layer_ordering_present_flag: bool,
    /// Indexed by temporal sub-layer; only the first
    /// `sps_max_sub_layers_minus1 + 1` entries are written by the parser.
    pub(crate) sps_max_dec_pic_buffering_minus1: [u32; 8],
    pub(crate) sps_max_num_reorder_pics: [u32; 8],
    pub(crate) log2_min_luma_coding_block_size_minus3: u32,
    pub(crate) log2_diff_max_min_luma_coding_block_size: u32,
    pub(crate) amp_enabled_flag: bool,
    pub(crate) sample_adaptive_offset_enabled_flag: bool,
}

/// Parse the leading syntax of a `seq_parameter_set_rbsp()` (RBSP form: NAL
/// header and emulation prevention already removed).
///
/// Parsing is real through `sample_adaptive_offset_enabled_flag` — including
/// the full `profile_tier_level()` and the sub-layer ordering loop — and
/// deliberately stops there: PCM, scaling-list data, reference picture sets,
/// VUI, and extensions are not consumed by this skeleton. If scaling-list
/// data is announced, an `OutOfRange` error is returned rather than skipping
/// a structure this skeleton does not implement.
pub(crate) fn parse_sps(rbsp: &[u8]) -> Result<SpsInfo, Error> {
    let mut br = BitReader::new(rbsp);
    let sps_video_parameter_set_id = br.get(4)? as u8;
    let sps_max_sub_layers_minus1 = br.get(3)? as u8;
    let sps_temporal_id_nesting_flag = br.get(1)? != 0;
    let ptl = parse_profile_tier_level(&mut br, sps_max_sub_layers_minus1)?;

    let sps_seq_parameter_set_id = br.ue()?;
    if sps_seq_parameter_set_id > 15 {
        return Err(Error::OutOfRange("sps_seq_parameter_set_id"));
    }
    let chroma_format_idc = br.ue()?;
    if chroma_format_idc > 3 {
        return Err(Error::OutOfRange("chroma_format_idc"));
    }
    let separate_colour_plane_flag = if chroma_format_idc == 3 {
        br.get(1)? != 0
    } else {
        false
    };
    let pic_width_in_luma_samples = br.ue()?;
    let pic_height_in_luma_samples = br.ue()?;
    if pic_width_in_luma_samples == 0 || pic_height_in_luma_samples == 0 {
        return Err(Error::OutOfRange("picture size"));
    }
    let conformance_window_flag = br.get(1)? != 0;
    if conformance_window_flag {
        for _ in 0..4 {
            br.skip_ue()?; // conf_win_{left,right,top,bottom}_offset
        }
    }
    let bit_depth_luma_minus8 = br.ue()?;
    let bit_depth_chroma_minus8 = br.ue()?;
    if bit_depth_luma_minus8 > 8 || bit_depth_chroma_minus8 > 8 {
        return Err(Error::OutOfRange("bit_depth_*_minus8"));
    }
    let log2_max_pic_order_cnt_lsb_minus4 = br.ue()?;
    if log2_max_pic_order_cnt_lsb_minus4 > 12 {
        return Err(Error::OutOfRange("log2_max_pic_order_cnt_lsb_minus4"));
    }

    let sps_sub_layer_ordering_present_flag = br.get(1)? != 0;
    let mut sps_max_dec_pic_buffering_minus1 = [0u32; 8];
    let mut sps_max_num_reorder_pics = [0u32; 8];
    // Spec loop bounds: every sub-layer when the ordering info is present,
    // otherwise only the highest sub-layer.
    let first = if sps_sub_layer_ordering_present_flag {
        0
    } else {
        usize::from(sps_max_sub_layers_minus1)
    };
    for i in first..=usize::from(sps_max_sub_layers_minus1) {
        sps_max_dec_pic_buffering_minus1[i] = br.ue()?;
        sps_max_num_reorder_pics[i] = br.ue()?;
        br.skip_ue()?; // sps_max_latency_increase_plus1[i]
    }

    let log2_min_luma_coding_block_size_minus3 = br.ue()?;
    let log2_diff_max_min_luma_coding_block_size = br.ue()?;
    let ctb_log2 = 3u32
        .checked_add(log2_min_luma_coding_block_size_minus3)
        .and_then(|v| v.checked_add(log2_diff_max_min_luma_coding_block_size))
        .ok_or(Error::OutOfRange("luma coding block sizes"))?;
    if ctb_log2 > 7 {
        return Err(Error::OutOfRange("luma coding block sizes"));
    }

    // Parsed to reach the flags below; not retained by this skeleton.
    let _log2_min_luma_transform_block_size_minus2 = br.ue()?;
    let _log2_diff_max_min_luma_transform_block_size = br.ue()?;
    let _max_transform_hierarchy_depth_inter = br.ue()?;
    let _max_transform_hierarchy_depth_intra = br.ue()?;

    let scaling_list_enabled_flag = br.get(1)? != 0;
    if scaling_list_enabled_flag && br.get(1)? != 0 {
        // sps_scaling_list_data_present_flag: skipping scaling_list_data()
        // faithfully is not worth the code in a skeleton.
        return Err(Error::OutOfRange("sps_scaling_list_data_present_flag"));
    }
    let amp_enabled_flag = br.get(1)? != 0;
    let sample_adaptive_offset_enabled_flag = br.get(1)? != 0;

    Ok(SpsInfo {
        sps_video_parameter_set_id,
        sps_max_sub_layers_minus1,
        sps_temporal_id_nesting_flag,
        ptl,
        sps_seq_parameter_set_id,
        chroma_format_idc,
        separate_colour_plane_flag,
        pic_width_in_luma_samples,
        pic_height_in_luma_samples,
        conformance_window_flag,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        log2_max_pic_order_cnt_lsb_minus4,
        sps_sub_layer_ordering_present_flag,
        sps_max_dec_pic_buffering_minus1,
        sps_max_num_reorder_pics,
        log2_min_luma_coding_block_size_minus3,
        log2_diff_max_min_luma_coding_block_size,
        amp_enabled_flag,
        sample_adaptive_offset_enabled_flag,
    })
}

/// Fields kept from a parsed `video_parameter_set_rbsp()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VpsInfo {
    pub(crate) vps_video_parameter_set_id: u8,
    pub(crate) vps_max_layers_minus1: u8,
    pub(crate) vps_max_sub_layers_minus1: u8,
    pub(crate) vps_temporal_id_nesting_flag: bool,
    pub(crate) ptl: ProfileTierLevel,
    pub(crate) vps_sub_layer_ordering_info_present_flag: bool,
    /// Value for sub-layer 0 (the only entry the driver cares about).
    pub(crate) vps_max_dec_pic_buffering_minus1: u32,
    pub(crate) vps_max_num_reorder_pics: u32,
    pub(crate) vps_max_layer_id: u8,
    pub(crate) vps_num_layer_sets_minus1: u32,
}

/// Parse a `video_parameter_set_rbsp()` down to the layer-set list.
///
/// Enough to identify the VPS and keep the PTL; the timing/HRD and extension
/// syntax after the layer sets is deliberately not consumed, because the VPS
/// is carried opaquely toward the hardware by the rest of the driver.
pub(crate) fn parse_vps(rbsp: &[u8]) -> Result<VpsInfo, Error> {
    let mut br = BitReader::new(rbsp);
    let vps_video_parameter_set_id = br.get(4)? as u8;
    br.skip(2)?; // vps_base_layer_internal/available_flag
    let vps_max_layers_minus1 = br.get(6)? as u8;
    let vps_max_sub_layers_minus1 = br.get(3)? as u8;
    let vps_temporal_id_nesting_flag = br.get(1)? != 0;
    br.skip(16)?; // vps_reserved_0xffff_16bits
    let ptl = parse_profile_tier_level(&mut br, vps_max_sub_layers_minus1)?;

    let vps_sub_layer_ordering_info_present_flag = br.get(1)? != 0;
    let first = if vps_sub_layer_ordering_info_present_flag {
        0
    } else {
        usize::from(vps_max_sub_layers_minus1)
    };
    let mut vps_max_dec_pic_buffering_minus1 = 0;
    let mut vps_max_num_reorder_pics = 0;
    for i in first..=usize::from(vps_max_sub_layers_minus1) {
        let dec = br.ue()?;
        let reorder = br.ue()?;
        br.skip_ue()?; // vps_max_latency_increase_plus1[i]
        if i == 0 {
            vps_max_dec_pic_buffering_minus1 = dec;
            vps_max_num_reorder_pics = reorder;
        }
    }

    let vps_max_layer_id = br.get(6)? as u8;
    let vps_num_layer_sets_minus1 = br.ue()?;
    if vps_num_layer_sets_minus1 > 1023 {
        return Err(Error::OutOfRange("vps_num_layer_sets_minus1"));
    }
    for _ in 1..=vps_num_layer_sets_minus1 {
        for _ in 0..=u32::from(vps_max_layer_id) {
            br.skip(1)?; // layer_id_included_flag[i][j]
        }
    }

    Ok(VpsInfo {
        vps_video_parameter_set_id,
        vps_max_layers_minus1,
        vps_max_sub_layers_minus1,
        vps_temporal_id_nesting_flag,
        ptl,
        vps_sub_layer_ordering_info_present_flag,
        vps_max_dec_pic_buffering_minus1,
        vps_max_num_reorder_pics,
        vps_max_layer_id,
        vps_num_layer_sets_minus1,
    })
}

/// Fields kept from a parsed `pic_parameter_set_rbsp()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PpsInfo {
    pub(crate) pps_pic_parameter_set_id: u32,
    pub(crate) pps_seq_parameter_set_id: u32,
    pub(crate) dependent_slice_segments_enabled_flag: bool,
    pub(crate) output_flag_present_flag: bool,
    pub(crate) num_extra_slice_header_bits: u8,
    pub(crate) sign_data_hiding_enabled_flag: bool,
    pub(crate) cabac_init_present_flag: bool,
    pub(crate) num_ref_idx_l0_default_active_minus1: u32,
    pub(crate) num_ref_idx_l1_default_active_minus1: u32,
    pub(crate) init_qp_minus26: i32,
    pub(crate) constrained_intra_pred_flag: bool,
    pub(crate) transform_skip_enabled_flag: bool,
    pub(crate) cu_qp_delta_enabled_flag: bool,
    /// Only meaningful when `cu_qp_delta_enabled_flag` is set.
    pub(crate) diff_cu_qp_delta_depth: u32,
    pub(crate) pps_cb_qp_offset: i32,
    pub(crate) pps_cr_qp_offset: i32,
    pub(crate) pps_slice_chroma_qp_offsets_present_flag: bool,
    pub(crate) weighted_pred_flag: bool,
    pub(crate) weighted_bipred_flag: bool,
    pub(crate) transquant_bypass_enabled_flag: bool,
    pub(crate) tiles_enabled_flag: bool,
    /// Only meaningful when `tiles_enabled_flag` is set.
    pub(crate) num_tile_columns_minus1: u32,
    pub(crate) num_tile_rows_minus1: u32,
    pub(crate) uniform_spacing_flag: bool,
    pub(crate) loop_filter_across_tiles_enabled_flag: bool,
}

/// Parse a `pic_parameter_set_rbsp()` down to the tile syntax.
///
/// The parsed fields are the ones a future integration step needs to
/// sanity-check the PPS against the SPS (IDs, QP, slice flags, tiles);
/// deblocking and list/extension syntax after the tile block is deliberately
/// not consumed.
pub(crate) fn parse_pps(rbsp: &[u8]) -> Result<PpsInfo, Error> {
    let mut br = BitReader::new(rbsp);
    let pps_pic_parameter_set_id = br.ue()?;
    if pps_pic_parameter_set_id > 63 {
        return Err(Error::OutOfRange("pps_pic_parameter_set_id"));
    }
    let pps_seq_parameter_set_id = br.ue()?;
    if pps_seq_parameter_set_id > 15 {
        return Err(Error::OutOfRange("pps_seq_parameter_set_id"));
    }
    let dependent_slice_segments_enabled_flag = br.get(1)? != 0;
    let output_flag_present_flag = br.get(1)? != 0;
    let num_extra_slice_header_bits = br.get(3)? as u8;
    let sign_data_hiding_enabled_flag = br.get(1)? != 0;
    let cabac_init_present_flag = br.get(1)? != 0;
    let num_ref_idx_l0_default_active_minus1 = br.ue()?;
    let num_ref_idx_l1_default_active_minus1 = br.ue()?;
    let init_qp_minus26 = br.se()?;
    let constrained_intra_pred_flag = br.get(1)? != 0;
    let transform_skip_enabled_flag = br.get(1)? != 0;
    let cu_qp_delta_enabled_flag = br.get(1)? != 0;
    let diff_cu_qp_delta_depth = if cu_qp_delta_enabled_flag {
        br.ue()?
    } else {
        0
    };
    let pps_cb_qp_offset = br.se()?;
    let pps_cr_qp_offset = br.se()?;
    let pps_slice_chroma_qp_offsets_present_flag = br.get(1)? != 0;
    let weighted_pred_flag = br.get(1)? != 0;
    let weighted_bipred_flag = br.get(1)? != 0;
    let transquant_bypass_enabled_flag = br.get(1)? != 0;

    let tiles_enabled_flag = br.get(1)? != 0;
    let (num_tile_columns_minus1, num_tile_rows_minus1, uniform_spacing_flag, across_tiles) =
        if tiles_enabled_flag {
            let cols = br.ue()?;
            let rows = br.ue()?;
            // Loose garbage filter; the real spec limit derives from the
            // picture size in CTBs, which this skeleton does not cross-check.
            if cols > 65_535 || rows > 65_535 {
                return Err(Error::OutOfRange("tile counts"));
            }
            let uniform = br.get(1)? != 0;
            if !uniform {
                for _ in 0..cols {
                    br.skip_ue()?; // column_width_minus1[i]
                }
                for _ in 0..rows {
                    br.skip_ue()?; // row_height_minus1[i]
                }
            }
            (cols, rows, uniform, br.get(1)? != 0)
        } else {
            (0, 0, false, false)
        };

    Ok(PpsInfo {
        pps_pic_parameter_set_id,
        pps_seq_parameter_set_id,
        dependent_slice_segments_enabled_flag,
        output_flag_present_flag,
        num_extra_slice_header_bits,
        sign_data_hiding_enabled_flag,
        cabac_init_present_flag,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        init_qp_minus26,
        constrained_intra_pred_flag,
        transform_skip_enabled_flag,
        cu_qp_delta_enabled_flag,
        diff_cu_qp_delta_depth,
        pps_cb_qp_offset,
        pps_cr_qp_offset,
        pps_slice_chroma_qp_offsets_present_flag,
        weighted_pred_flag,
        weighted_bipred_flag,
        transquant_bypass_enabled_flag,
        tiles_enabled_flag,
        num_tile_columns_minus1,
        num_tile_rows_minus1,
        uniform_spacing_flag,
        loop_filter_across_tiles_enabled_flag: across_tiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
    }

    /// Write a Main-profile `profile_tier_level()` exactly as the parser
    /// reads it, with one flag pair per sub-layer below the top one.
    fn write_ptl(bw: &mut BitWriter, max_sub_layers_minus1: u32, flags: &[(bool, bool)]) {
        bw.put(0, 2); // general_profile_space
        bw.put(0, 1); // general_tier_flag
        bw.put(1, 5); // general_profile_idc = Main
        for _ in 0..32 {
            bw.put(1, 1); // general_profile_compatibility_flag
        }
        bw.put(1, 1); // general_progressive_source_flag
        bw.put(0, 1); // general_interlaced_source_flag
        bw.put(1, 1); // general_non_packed_constraint_flag
        bw.put(1, 1); // general_frame_only_constraint_flag
        bw.put(0, 32); // reserved_zero_43bits (+ inbld) — low half
        bw.put(0, 12); // — high half
        bw.put(120, 8); // general_level_idc = level 4
        for (profile, level) in flags.iter().take(max_sub_layers_minus1 as usize) {
            bw.put(u64::from(*profile), 1); // sub_layer_profile_present_flag
            bw.put(u64::from(*level), 1); // sub_layer_level_present_flag
        }
        if max_sub_layers_minus1 > 0 {
            for _ in max_sub_layers_minus1..8 {
                bw.put(0, 2); // reserved_zero_2bits
            }
        }
        for (profile, _) in flags.iter().take(max_sub_layers_minus1 as usize) {
            if *profile {
                bw.put(0, 3); // space + tier
                bw.put(1, 5); // profile idc
                bw.put(0xFFFF_FFFF, 32); // compatibility flags
                bw.put(0b1011, 4); // progressive/interlaced/non_packed/frame_only
                bw.put(0, 32); // reserved 43 bits + inbld — low half
                bw.put(0, 12); // — high half
            }
        }
        for (_, level) in flags.iter().take(max_sub_layers_minus1 as usize) {
            if *level {
                bw.put(93, 8); // sub_layer_level_idc
            }
        }
    }

    fn synth_sps_rbsp(ordering_present: bool, max_sub_layers_minus1: u32) -> Vec<u8> {
        let mut bw = BitWriter::new();
        bw.put(0, 4); // sps_video_parameter_set_id
        bw.put(u64::from(max_sub_layers_minus1), 3);
        bw.put(1, 1); // sps_temporal_id_nesting_flag
        write_ptl(
            &mut bw,
            max_sub_layers_minus1,
            &[(true, false), (false, true), (true, true)],
        );
        bw.put_ue(5); // sps_seq_parameter_set_id
        bw.put_ue(1); // chroma_format_idc = 4:2:0
        bw.put_ue(1280); // pic_width_in_luma_samples
        bw.put_ue(720); // pic_height_in_luma_samples
        bw.put(0, 1); // conformance_window_flag
        bw.put_ue(0); // bit_depth_luma_minus8
        bw.put_ue(2); // bit_depth_chroma_minus8 (10-bit)
        bw.put_ue(4); // log2_max_pic_order_cnt_lsb_minus4
        bw.put(u64::from(ordering_present), 1);
        let first = if ordering_present {
            0
        } else {
            max_sub_layers_minus1
        };
        for _ in first..=max_sub_layers_minus1 {
            bw.put_ue(3); // *_max_dec_pic_buffering_minus1
            bw.put_ue(1); // *_max_num_reorder_pics
            bw.put_ue(0); // *_max_latency_increase_plus1
        }
        bw.put_ue(0); // log2_min_luma_coding_block_size_minus3
        bw.put_ue(3); // log2_diff_max_min_luma_coding_block_size (CTB 64)
        bw.put_ue(0); // log2_min_luma_transform_block_size_minus2
        bw.put_ue(3); // log2_diff_max_min_luma_transform_block_size
        bw.put_ue(2); // max_transform_hierarchy_depth_inter
        bw.put_ue(2); // max_transform_hierarchy_depth_intra
        bw.put(0, 1); // scaling_list_enabled_flag
        bw.put(1, 1); // amp_enabled_flag
        bw.put(1, 1); // sample_adaptive_offset_enabled_flag
        bw.rbsp_trailing();
        bw.into_bytes()
    }

    fn synth_vps_rbsp() -> Vec<u8> {
        let mut bw = BitWriter::new();
        bw.put(0, 4); // vps_video_parameter_set_id
        bw.put(1, 1); // vps_base_layer_internal_flag
        bw.put(1, 1); // vps_base_layer_available_flag
        bw.put(0, 6); // vps_max_layers_minus1
        bw.put(0, 3); // vps_max_sub_layers_minus1
        bw.put(1, 1); // vps_temporal_id_nesting_flag
        bw.put(0xFFFF, 16); // vps_reserved_0xffff_16bits
        write_ptl(&mut bw, 0, &[]);
        bw.put(1, 1); // vps_sub_layer_ordering_info_present_flag
        bw.put_ue(3); // vps_max_dec_pic_buffering_minus1[0]
        bw.put_ue(1); // vps_max_num_reorder_pics[0]
        bw.put_ue(0); // vps_max_latency_increase_plus1[0]
        bw.put(0, 6); // vps_max_layer_id
        bw.put_ue(0); // vps_num_layer_sets_minus1
        bw.rbsp_trailing();
        bw.into_bytes()
    }

    fn synth_pps_rbsp() -> Vec<u8> {
        let mut bw = BitWriter::new();
        bw.put_ue(2); // pps_pic_parameter_set_id
        bw.put_ue(5); // pps_seq_parameter_set_id (matches the synthetic SPS)
        bw.put(1, 1); // dependent_slice_segments_enabled_flag
        bw.put(1, 1); // output_flag_present_flag
        bw.put(0, 3); // num_extra_slice_header_bits
        bw.put(1, 1); // sign_data_hiding_enabled_flag
        bw.put(0, 1); // cabac_init_present_flag
        bw.put_ue(0); // num_ref_idx_l0_default_active_minus1
        bw.put_ue(1); // num_ref_idx_l1_default_active_minus1
        bw.put_se(-5); // init_qp_minus26
        bw.put(0, 1); // constrained_intra_pred_flag
        bw.put(1, 1); // transform_skip_enabled_flag
        bw.put(1, 1); // cu_qp_delta_enabled_flag
        bw.put_ue(2); // pps_diff_cu_qp_delta_depth
        bw.put_se(6); // pps_cb_qp_offset
        bw.put_se(-6); // pps_cr_qp_offset
        bw.put(0, 1); // pps_slice_chroma_qp_offsets_present_flag
        bw.put(0, 1); // weighted_pred_flag
        bw.put(1, 1); // weighted_bipred_flag
        bw.put(1, 1); // transquant_bypass_enabled_flag
        bw.put(1, 1); // tiles_enabled_flag
        bw.put_ue(1); // num_tile_columns_minus1 (2 columns)
        bw.put_ue(0); // num_tile_rows_minus1
        bw.put(1, 1); // uniform_spacing_flag
        bw.put(1, 1); // loop_filter_across_tiles_enabled_flag
        bw.rbsp_trailing();
        bw.into_bytes()
    }

    #[test]
    fn nal_header_round_trip_and_required_type_classification() {
        for (t, expected) in [
            (0u8, NalKind::TrailN),
            (1, NalKind::TrailR),
            (19, NalKind::IdrWRadl),
            (20, NalKind::IdrNlp),
            (21, NalKind::Cra),
            (32, NalKind::Vps),
            (33, NalKind::Sps),
            (34, NalKind::Pps),
        ] {
            let nal = Nal::build(t, 0, 1, &[0xAA, 0xBB]);
            assert_eq!(nal.kind(), expected);
            let annex = nal.annex_b();
            // 3-byte start code, then the 2-byte header.
            assert_eq!(&annex[..3], &[0x00, 0x00, 0x01]);
            let reparsed = Nal::parse(&annex[3..]).unwrap();
            assert_eq!(reparsed, nal);
            assert_eq!(reparsed.header.nuh_layer_id, 0);
            assert_eq!(reparsed.header.nuh_temporal_id_plus1, 1);
        }
        // VCL types 0..=23 are slices; parameter sets are not.
        assert!(NalKind::TrailN.is_slice());
        assert!(NalKind::IdrNlp.is_slice());
        assert!(NalKind::Cra.is_slice());
        assert!(!NalKind::Vps.is_slice());
    }

    #[test]
    fn unknown_nal_unit_type_passes_through_as_other() {
        let nal = Nal::build(62, 1, 2, &[0x55]);
        assert_eq!(nal.kind(), NalKind::Other(62));
        let parsed = Nal::parse(&nal.annex_b()[3..]).unwrap();
        assert_eq!(parsed.kind(), NalKind::Other(62));
        assert_eq!(parsed.header.nuh_layer_id, 1);
        assert_eq!(parsed.header.nuh_temporal_id_plus1, 2);
        // Reserved type 35 (AUD) is a passthrough and not a slice.
        assert_eq!(NalKind::from_nal_unit_type(35), NalKind::Other(35));
        assert!(!NalKind::from_nal_unit_type(35).is_slice());
    }

    #[test]
    fn forbidden_zero_bit_is_rejected() {
        let mut raw = Nal::build(33, 0, 1, &[0x01]).annex_b();
        raw[3] |= 0x80; // first header byte, forbidden_zero_bit position
        assert_eq!(Nal::parse(&raw[3..]), Err(Error::ForbiddenZeroBit));
    }

    #[test]
    fn zero_temporal_id_plus1_is_rejected() {
        // Hand-write a header with nuh_temporal_id_plus1 = 0.
        let mut bw = BitWriter::new();
        bw.put(0, 1); // forbidden_zero_bit
        bw.put(1, 6); // TRAIL_R
        bw.put(0, 6); // nuh_layer_id
        bw.put(0, 3); // nuh_temporal_id_plus1 = 0
        assert_eq!(
            Nal::parse(&bw.into_bytes()),
            Err(Error::InvalidTemporalIdPlus1)
        );
    }

    #[test]
    fn short_nal_is_truncated_not_panicking() {
        assert_eq!(Nal::parse(&[0x42]), Err(Error::Truncated));
        assert_eq!(Nal::parse(&[]), Err(Error::Truncated));
    }

    #[test]
    fn sps_parse_assemble_reparse_round_trip() {
        let vps = Nal::build(32, 0, 1, &synth_vps_rbsp());
        let sps = Nal::build(33, 0, 1, &synth_sps_rbsp(true, 2));
        let pps = Nal::build(34, 0, 1, &synth_pps_rbsp());
        let slice = Nal::build(19, 0, 1, &[0xAB, 0xCD, 0xEF]);

        let info = parse_sps(&sps.rbsp().unwrap()).unwrap();
        assert_eq!(info.sps_video_parameter_set_id, 0);
        assert_eq!(info.sps_max_sub_layers_minus1, 2);
        assert!(info.sps_temporal_id_nesting_flag);
        assert_eq!(info.ptl.general_profile_space, 0);
        assert!(!info.ptl.general_tier_flag);
        assert_eq!(info.ptl.general_profile_idc, 1);
        assert_eq!(info.ptl.general_level_idc, 120);
        assert_eq!(info.sps_seq_parameter_set_id, 5);
        assert_eq!(info.chroma_format_idc, 1);
        assert!(!info.separate_colour_plane_flag);
        assert_eq!(info.pic_width_in_luma_samples, 1280);
        assert_eq!(info.pic_height_in_luma_samples, 720);
        assert!(!info.conformance_window_flag);
        assert_eq!(info.bit_depth_luma_minus8, 0);
        assert_eq!(info.bit_depth_chroma_minus8, 2);
        assert_eq!(info.log2_max_pic_order_cnt_lsb_minus4, 4);
        assert!(info.sps_sub_layer_ordering_present_flag);
        assert_eq!(info.sps_max_dec_pic_buffering_minus1[..=2], [3u32; 3]);
        assert_eq!(info.sps_max_num_reorder_pics[..=2], [1u32; 3]);
        assert_eq!(info.log2_min_luma_coding_block_size_minus3, 0);
        assert_eq!(info.log2_diff_max_min_luma_coding_block_size, 3);
        assert!(info.amp_enabled_flag);
        assert!(info.sample_adaptive_offset_enabled_flag);

        let vpsi = parse_vps(&vps.rbsp().unwrap()).unwrap();
        assert_eq!(vpsi.vps_video_parameter_set_id, 0);
        assert_eq!(vpsi.vps_max_layers_minus1, 0);
        assert_eq!(vpsi.vps_max_sub_layers_minus1, 0);
        assert!(vpsi.vps_temporal_id_nesting_flag);
        assert_eq!(vpsi.ptl.general_profile_idc, 1);
        assert_eq!(vpsi.ptl.general_level_idc, 120);
        assert!(vpsi.vps_sub_layer_ordering_info_present_flag);
        assert_eq!(vpsi.vps_max_dec_pic_buffering_minus1, 3);
        assert_eq!(vpsi.vps_max_num_reorder_pics, 1);
        assert_eq!(vpsi.vps_max_layer_id, 0);
        assert_eq!(vpsi.vps_num_layer_sets_minus1, 0);

        let ppsi = parse_pps(&pps.rbsp().unwrap()).unwrap();
        assert_eq!(ppsi.pps_pic_parameter_set_id, 2);
        assert_eq!(ppsi.pps_seq_parameter_set_id, 5);
        assert!(ppsi.dependent_slice_segments_enabled_flag);
        assert!(ppsi.output_flag_present_flag);
        assert_eq!(ppsi.num_extra_slice_header_bits, 0);
        assert!(ppsi.sign_data_hiding_enabled_flag);
        assert!(!ppsi.cabac_init_present_flag);
        assert_eq!(ppsi.num_ref_idx_l0_default_active_minus1, 0);
        assert_eq!(ppsi.num_ref_idx_l1_default_active_minus1, 1);
        assert_eq!(ppsi.init_qp_minus26, -5);
        assert!(!ppsi.constrained_intra_pred_flag);
        assert!(ppsi.transform_skip_enabled_flag);
        assert!(ppsi.cu_qp_delta_enabled_flag);
        assert_eq!(ppsi.diff_cu_qp_delta_depth, 2);
        assert_eq!(ppsi.pps_cb_qp_offset, 6);
        assert_eq!(ppsi.pps_cr_qp_offset, -6);
        assert!(!ppsi.pps_slice_chroma_qp_offsets_present_flag);
        assert!(!ppsi.weighted_pred_flag);
        assert!(ppsi.weighted_bipred_flag);
        assert!(ppsi.transquant_bypass_enabled_flag);
        assert!(ppsi.tiles_enabled_flag);
        assert_eq!(ppsi.num_tile_columns_minus1, 1);
        assert_eq!(ppsi.num_tile_rows_minus1, 0);
        assert!(ppsi.uniform_spacing_flag);
        assert!(ppsi.loop_filter_across_tiles_enabled_flag);

        let au = assemble_access_unit(Some(&vps), &sps, &pps, &slice).unwrap();
        let parts = split_annex_b(&au);
        assert_eq!(parts.len(), 4);
        assert_eq!(Nal::parse(&parts[0]).unwrap().kind(), NalKind::Vps);
        assert_eq!(Nal::parse(&parts[1]).unwrap(), sps);
        assert_eq!(Nal::parse(&parts[2]).unwrap(), pps);
        assert_eq!(Nal::parse(&parts[3]).unwrap(), slice);
    }

    #[test]
    fn sps_sub_layer_ordering_present_flag_clear_reads_only_top_sub_layer() {
        // ordering flag = 0 with sps_max_sub_layers_minus1 = 3: the spec loop
        // runs for i = 3 only. The CTB sizes and SAO flag that follow must
        // still parse in sync, proving the loop bounds were applied.
        let sps = Nal::build(33, 0, 1, &synth_sps_rbsp(false, 3));
        let info = parse_sps(&sps.rbsp().unwrap()).unwrap();
        assert!(!info.sps_sub_layer_ordering_present_flag);
        assert_eq!(info.sps_max_sub_layers_minus1, 3);
        assert_eq!(info.sps_max_dec_pic_buffering_minus1[3], 3);
        assert_eq!(info.sps_max_dec_pic_buffering_minus1[0], 0);
        assert_eq!(info.sps_max_num_reorder_pics[3], 1);
        assert_eq!(info.log2_min_luma_coding_block_size_minus3, 0);
        assert_eq!(info.log2_diff_max_min_luma_coding_block_size, 3);
        assert!(info.sample_adaptive_offset_enabled_flag);
    }

    #[test]
    fn assembled_access_unit_uses_three_byte_start_codes() {
        let sps = Nal::build(33, 0, 1, &[0x01]);
        let pps = Nal::build(34, 0, 1, &[0x01]);
        let slice = Nal::build(19, 0, 1, &[0x02]);
        let au = assemble_access_unit(None, &sps, &pps, &slice).unwrap();
        // SPS header for type 33 / layer 0 / tid+1 = 1 is 42 01; PPS is
        // 44 01; IDR_W_RADL is 26 01.
        assert_eq!(au, bytes("000001420101000001440101000001260102"));
    }

    #[test]
    fn assemble_rejects_mismatched_nal_roles() {
        let sps = Nal::build(33, 0, 1, &[0x01]);
        let pps = Nal::build(34, 0, 1, &[0x01]);
        let slice = Nal::build(19, 0, 1, &[0x02]);
        assert_eq!(
            assemble_access_unit(None, &pps, &sps, &slice).unwrap_err(),
            Error::UnexpectedNalType
        );
        assert_eq!(
            assemble_access_unit(None, &sps, &sps, &slice).unwrap_err(),
            Error::UnexpectedNalType
        );
        assert_eq!(
            assemble_access_unit(None, &sps, &pps, &pps).unwrap_err(),
            Error::UnexpectedNalType
        );
    }

    #[test]
    fn emulation_prevention_in_payload_survives_assembly_and_reparse() {
        // RBSP containing every sequence that must be escaped.
        let rbsp = [0x00u8, 0x00, 0x00, 0x01, 0x02, 0x03, 0xFF, 0x00, 0x00, 0x02];
        let slice = Nal::build(19, 0, 1, &rbsp);
        assert_eq!(
            slice.payload_ebsp,
            vec![
                0x00, 0x00, 0x03, 0x00, 0x01, 0x02, 0x03, 0xFF, 0x00, 0x00, 0x03, 0x02
            ]
        );
        let sps = Nal::build(33, 0, 1, &[0x01]);
        let pps = Nal::build(34, 0, 1, &[0x01]);
        let au = assemble_access_unit(None, &sps, &pps, &slice).unwrap();
        assert!(au.windows(3).any(|w| w == [0x00, 0x00, 0x03]));
        let parts = split_annex_b(&au);
        assert_eq!(parts.len(), 3);
        let parsed = Nal::parse(&parts[2]).unwrap();
        assert_eq!(parsed, slice);
        assert_eq!(parsed.rbsp().unwrap(), rbsp);
    }

    #[test]
    fn multiple_nals_in_one_annexb_buffer_split_and_reparse() {
        let vps = Nal::build(32, 0, 1, &synth_vps_rbsp());
        let sps = Nal::build(33, 0, 1, &synth_sps_rbsp(true, 2));
        let pps = Nal::build(34, 0, 1, &synth_pps_rbsp());
        let s1 = Nal::build(19, 0, 1, &[0x11, 0x22]);
        let s2 = Nal::build(1, 0, 1, &[0x33]);
        let mut buf = Vec::new();
        for nal in [&vps, &sps, &pps, &s1, &s2] {
            buf.extend_from_slice(&nal.annex_b());
        }
        let parts = split_annex_b(&buf);
        assert_eq!(parts.len(), 5);
        for (part, original) in parts.iter().zip([&vps, &sps, &pps, &s1, &s2]) {
            assert_eq!(&Nal::parse(part).unwrap(), original);
        }

        // A 4-byte start code (extra leading zero) is also handled.
        let mut padded = vec![0x00, 0x00, 0x00, 0x01];
        padded.extend_from_slice(&sps.header_bytes());
        padded.extend_from_slice(&sps.payload_ebsp);
        padded.extend_from_slice(&s1.annex_b());
        let parts = split_annex_b(&padded);
        assert_eq!(parts.len(), 2);
        assert_eq!(Nal::parse(&parts[0]).unwrap(), sps);
        assert_eq!(Nal::parse(&parts[1]).unwrap(), s1);
    }

    #[test]
    fn parse_validates_field_ranges() {
        // pps_pic_parameter_set_id above the spec maximum of 63.
        let mut bw = BitWriter::new();
        bw.put_ue(64);
        bw.put_ue(0);
        assert_eq!(
            parse_pps(&bw.into_bytes()).unwrap_err(),
            Error::OutOfRange("pps_pic_parameter_set_id")
        );

        // sps_seq_parameter_set_id above 15.
        let mut bw = BitWriter::new();
        bw.put(0, 8); // sps ids + nesting flag
        write_ptl(&mut bw, 0, &[]);
        bw.put_ue(16);
        assert_eq!(
            parse_sps(&bw.into_bytes()).unwrap_err(),
            Error::OutOfRange("sps_seq_parameter_set_id")
        );

        // chroma_format_idc 4 does not exist.
        let mut bw = BitWriter::new();
        bw.put(0, 8);
        write_ptl(&mut bw, 0, &[]);
        bw.put_ue(0);
        bw.put_ue(4);
        assert_eq!(
            parse_sps(&bw.into_bytes()).unwrap_err(),
            Error::OutOfRange("chroma_format_idc")
        );
    }

    #[test]
    fn truncated_nals_return_clean_errors() {
        assert_eq!(parse_sps(&[]), Err(Error::Truncated));
        assert_eq!(parse_vps(&[]), Err(Error::Truncated));
        assert_eq!(parse_pps(&[]), Err(Error::Truncated));

        // Every prefix of a valid SPS payload must fail cleanly (either the
        // EBSP slice is mid-escape, or the SPS syntax is incomplete).
        let sps = Nal::build(33, 0, 1, &synth_sps_rbsp(true, 2));
        let full = sps.rbsp().unwrap();
        for cut in 0..full.len() / 2 {
            let partial = Nal::build(33, 0, 1, &full[..cut]);
            let rbsp = partial.rbsp().unwrap();
            assert!(
                parse_sps(&rbsp).is_err(),
                "SPS truncated at {cut} bytes must not parse"
            );
        }
    }
}
