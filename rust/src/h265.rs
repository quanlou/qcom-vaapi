//! H.265 (HEVC) bitstream parsing and Annex-B assembly.
//!
//! The parser validates NAL units and parameter sets used by tests and
//! diagnostics. `synth` rebuilds VPS/SPS/PPS headers from VA picture
//! parameters for the stateful V4L2 decoder.
//!
//! Syntax reference: ITU-T H.265 v6+, clauses 7.3.2.1-7.3.2.3 and Annex B.

mod bitstream;

use bitstream::{BitReader, BitWriter, START_CODE, ebsp_to_rbsp, rbsp_to_ebsp};

mod parse;

/// 64 MiB ceiling on one assembled access unit, mirroring the H.264 frame
/// assembly guard in `h264.rs`.
const MAX_ASSEMBLED_ACCESS_UNIT_BYTES: usize = 64 * 1024 * 1024;

mod synth;
pub(crate) use synth::{slice_pps_id, synthesize_parameter_sets};

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

#[cfg(test)]
mod tests {
    use super::parse::tests::{synth_pps_rbsp, synth_sps_rbsp, synth_vps_rbsp};
    use super::parse::{parse_pps, parse_sps, parse_vps};
    use super::*;
    use crate::bindings::VAPictureParameterBufferHEVC;

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect()
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
    fn va_picture_parameters_produce_parseable_main_parameter_sets() {
        let mut pp: VAPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
        pp.pic_width_in_luma_samples = 1280;
        pp.pic_height_in_luma_samples = 720;
        pp.sps_max_dec_pic_buffering_minus1 = 4;
        pp.log2_max_pic_order_cnt_lsb_minus4 = 4;
        pp.log2_min_luma_coding_block_size_minus3 = 1;
        pp.log2_diff_max_min_luma_coding_block_size = 1;
        pp.log2_diff_max_min_transform_block_size = 3;
        unsafe {
            pp.pic_fields.bits.set_chroma_format_idc(1);
            pp.slice_parsing_fields
                .bits
                .set_sps_temporal_mvp_enabled_flag(1);
        }

        let headers = synthesize_parameter_sets(&pp, 3).unwrap();
        let nals = split_annex_b(&headers);
        assert_eq!(nals.len(), 3);
        let vps = Nal::parse(&nals[0]).unwrap();
        let sps = Nal::parse(&nals[1]).unwrap();
        let pps = Nal::parse(&nals[2]).unwrap();
        assert_eq!(vps.kind(), NalKind::Vps);
        assert_eq!(sps.kind(), NalKind::Sps);
        assert_eq!(pps.kind(), NalKind::Pps);
        assert_eq!(
            parse_vps(&vps.rbsp().unwrap())
                .unwrap()
                .vps_max_dec_pic_buffering_minus1,
            4
        );
        assert_eq!(
            parse_sps(&sps.rbsp().unwrap())
                .unwrap()
                .pic_width_in_luma_samples,
            1280
        );
        assert_eq!(
            parse_pps(&pps.rbsp().unwrap())
                .unwrap()
                .pps_pic_parameter_set_id,
            3
        );
    }

    #[test]
    fn slice_pps_id_reads_original_irap_header() {
        let mut writer = BitWriter::new();
        writer.put(1, 1); // first_slice_segment_in_pic_flag
        writer.put(0, 1); // no_output_of_prior_pics_flag
        writer.put_ue(7);
        writer.rbsp_trailing();
        let slice = Nal::build(19, 0, 1, &writer.into_bytes());
        let raw = [&slice.header_bytes()[..], &slice.payload_ebsp[..]].concat();
        assert_eq!(slice_pps_id(&raw), Ok(7));
    }

    #[test]
    fn synthesis_rejects_unrepresentable_sps_reference_sets() {
        let mut pp: VAPictureParameterBufferHEVC = unsafe { std::mem::zeroed() };
        pp.num_short_term_ref_pic_sets = 1;
        assert_eq!(
            synthesize_parameter_sets(&pp, 0),
            Err(Error::OutOfRange("SPS short-term reference picture sets"))
        );
    }
}
