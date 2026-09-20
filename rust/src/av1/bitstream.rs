//! AV1 OBU framing and bit-level writers.
//!
//! Reference: AV1 Bitstream & Decoding Process Specification 5.3 (OBU
//! syntax) and 4.10.5 (leb128 encoding).
//!
//! Kept-narrow scope: this file only owns the byte-level OBU framing (header
//! byte, extension byte, LEB128 payload-size prefix) and a most-significant-
//! bit-first bit writer. Sequence and frame syntax live in `synth.rs`.

/// AV1 OBU types (spec 6.2.1).
///
/// Only the types this driver produces or recognises are listed; extras
/// exist in the spec (metadata, redundant frame headers, padding) but the
/// stateful iris pipeline does not require them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ObuType {
    SequenceHeader = 1,
    TemporalDelimiter = 2,
    /// The Frame OBU (uncompressed_header + tile_group_obu) — combined
    /// form for streams that never split them apart.
    Frame = 6,
    /// Standalone tile-group when Frame is split. Rare in practice.
    #[allow(dead_code)]
    TileGroup = 4,
}

/// LEB128 (little-endian base-128) size encoder used for OBU payload
/// sizes (spec 4.10.5). Writes 1-8 bytes with a continuation bit in each
/// byte's high position; the driver never emits payloads over 2^56 bytes,
/// but the writer supports the full u64 range for correctness.
pub(crate) fn write_leb128(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Number of bytes `write_leb128` would emit for `value`. Used to compute
/// a Frame OBU's size prefix before assembling its payload.
pub(crate) fn leb128_size(mut value: u64) -> usize {
    let mut count = 1;
    while value >= 0x80 {
        value >>= 7;
        count += 1;
    }
    count
}

/// AV1 MSB-first bit writer used for uncompressed_header, sequence header
/// bodies, and any other bit-level syntax. Bytes accumulate lazily; call
/// `finish()` to flush the trailing partial byte with zeros.
#[allow(dead_code)]
pub(crate) struct BitWriter {
    bytes: Vec<u8>,
    cur: u8,
    nbits: u8,
}

impl BitWriter {
    pub(crate) fn new() -> Self {
        Self {
            bytes: Vec::new(),
            cur: 0,
            nbits: 0,
        }
    }

    /// Write the low `bits` of `value` MSB-first. `bits` must be <= 32.
    pub(crate) fn write_bits(&mut self, value: u32, bits: u8) {
        debug_assert!(bits <= 32);
        for i in (0..bits).rev() {
            let bit = ((value >> i) & 1) as u8;
            self.cur = (self.cur << 1) | bit;
            self.nbits += 1;
            if self.nbits == 8 {
                self.bytes.push(self.cur);
                self.cur = 0;
                self.nbits = 0;
            }
        }
    }

    /// Convenience: one bit.
    pub(crate) fn write_flag(&mut self, flag: bool) {
        self.write_bits(u32::from(flag), 1);
    }

    /// Byte-align the stream by padding with zero bits (spec 5.3.4
    /// `trailing_bits`). Callers requiring the trailing_bits `1` marker
    /// (uncompressed_header, sequence_header) should call `write_flag(true)`
    /// first, then `align_to_byte`.
    pub(crate) fn align_to_byte(&mut self) {
        if self.nbits != 0 {
            self.cur <<= 8 - self.nbits;
            self.bytes.push(self.cur);
            self.cur = 0;
            self.nbits = 0;
        }
    }

    /// Consume the writer, byte-aligning if needed and returning the
    /// accumulated bytes. The trailing partial byte is padded with zero
    /// bits (spec `trailing_bits` when the caller has already emitted
    /// the `1` marker).
    pub(crate) fn finish(mut self) -> Vec<u8> {
        self.align_to_byte();
        self.bytes
    }
}

impl Default for BitWriter {
    fn default() -> Self {
        Self::new()
    }
}

/// Emit a single OBU frame with the given type and byte-aligned payload.
///
/// Layout (spec 5.3.2):
///   header byte = obu_forbidden_bit(0) | obu_type(4) | obu_extension_flag(0)
///               | obu_has_size_field(1) | obu_reserved_1bit(0)
///   LEB128 payload size
///   payload bytes
///
/// This driver always sets `obu_has_size_field = 1` (Annex-B / MP4 raw
/// framing). It never sets `obu_extension_flag`, since we do not synthesise
/// multi-layer streams.
pub(crate) struct ObuWriter;

impl ObuWriter {
    /// Full OBU framing around a payload.
    pub(crate) fn wrap(kind: ObuType, payload: &[u8]) -> Vec<u8> {
        let header = obu_header_byte(kind);
        let size = payload.len() as u64;
        let mut out = Vec::with_capacity(1 + leb128_size(size) + payload.len());
        out.push(header);
        write_leb128(&mut out, size);
        out.extend_from_slice(payload);
        out
    }

    /// The Temporal Delimiter OBU has no payload. Emit it before every
    /// frame's OBUs when the container does not already carry one.
    pub(crate) fn temporal_delimiter() -> [u8; 2] {
        [obu_header_byte(ObuType::TemporalDelimiter), 0]
    }
}

/// Encode the OBU header byte for a given OBU type. `obu_has_size_field`
/// is always set; `obu_extension_flag` and `obu_reserved_1bit` are always
/// clear. Layout is MSB-first per spec 5.3.2.
fn obu_header_byte(kind: ObuType) -> u8 {
    let type_bits = (kind as u8) & 0x0f;
    // Bit layout (MSB first):
    //   0        : obu_forbidden_bit (always 0)
    //   1..4     : obu_type
    //   5        : obu_extension_flag (0 — no extension header)
    //   6        : obu_has_size_field (1)
    //   7        : obu_reserved_1bit (0)
    (type_bits << 3) | 0b0000_0010
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leb128_encodes_short_values_in_one_byte() {
        let mut out = Vec::new();
        write_leb128(&mut out, 0);
        assert_eq!(out, vec![0x00]);

        let mut out = Vec::new();
        write_leb128(&mut out, 11);
        assert_eq!(out, vec![0x0b]);

        let mut out = Vec::new();
        write_leb128(&mut out, 0x7f);
        assert_eq!(out, vec![0x7f]);
    }

    #[test]
    fn leb128_encodes_longer_values_with_continuation_bits() {
        // 128 → 0x80 continuation, 0x01 high bits.
        let mut out = Vec::new();
        write_leb128(&mut out, 128);
        assert_eq!(out, vec![0x80, 0x01]);

        // 16384 → 0x80 0x80 0x01
        let mut out = Vec::new();
        write_leb128(&mut out, 16384);
        assert_eq!(out, vec![0x80, 0x80, 0x01]);
    }

    #[test]
    fn leb128_size_matches_write_leb128_length() {
        for value in [0u64, 1, 0x7f, 0x80, 0xff, 0x3fff, 0x4000, 0x1fffff] {
            let mut out = Vec::new();
            write_leb128(&mut out, value);
            assert_eq!(
                out.len(),
                leb128_size(value),
                "leb128_size mismatch for value {value:#x}"
            );
        }
    }

    #[test]
    fn temporal_delimiter_matches_spec() {
        // Header 0x12 (obu_type=2 shifted to bits 1..4 | has_size), size=0.
        assert_eq!(ObuWriter::temporal_delimiter(), [0x12, 0x00]);
    }

    #[test]
    fn obu_wrap_prepends_header_and_size() {
        let payload = vec![0xaa, 0xbb, 0xcc];
        let wrapped = ObuWriter::wrap(ObuType::SequenceHeader, &payload);
        // Header 0x0a (obu_type=1 | has_size), size=3, payload.
        assert_eq!(wrapped, vec![0x0a, 0x03, 0xaa, 0xbb, 0xcc]);
    }

    #[test]
    fn obu_wrap_uses_leb128_size_for_large_payloads() {
        let payload = vec![0u8; 128];
        let wrapped = ObuWriter::wrap(ObuType::Frame, &payload);
        // Header 0x32 (obu_type=6 | has_size), size 128 → LEB128 0x80 0x01.
        assert_eq!(wrapped[0], 0x32);
        assert_eq!(&wrapped[1..3], &[0x80, 0x01]);
        assert_eq!(wrapped.len(), 1 + 2 + payload.len());
    }

    #[test]
    fn bit_writer_packs_bits_msb_first() {
        // 5 bits from 0b10110 → 0xB0 (10110 000).
        let mut w = BitWriter::new();
        w.write_bits(0b10110, 5);
        let bytes = w.finish();
        assert_eq!(bytes, vec![0xB0]);

        // Interleave: 4 bits 0b1010 + 4 bits 0b0011 → 0xA3.
        let mut w = BitWriter::new();
        w.write_bits(0b1010, 4);
        w.write_bits(0b0011, 4);
        assert_eq!(w.finish(), vec![0xA3]);
    }

    #[test]
    fn bit_writer_write_flag_is_a_single_bit() {
        let mut w = BitWriter::new();
        w.write_flag(true);
        w.write_flag(false);
        w.write_flag(true);
        w.write_flag(true);
        w.write_flag(false);
        w.write_flag(false);
        w.write_flag(false);
        w.write_flag(true);
        assert_eq!(w.finish(), vec![0b1011_0001]);
    }

    #[test]
    fn bit_writer_zero_pads_partial_trailing_byte() {
        // 3 bits then finish → 0b101 padded with zeros → 0b1010_0000.
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        assert_eq!(w.finish(), vec![0b1010_0000]);
    }

    #[test]
    fn bit_writer_align_to_byte_preserves_full_bytes() {
        let mut w = BitWriter::new();
        w.write_bits(0xAB, 8);
        w.write_bits(0b11, 2);
        w.align_to_byte();
        // First byte 0xAB verbatim; second byte 0b11 padded → 0xC0.
        assert_eq!(w.finish(), vec![0xAB, 0xC0]);
    }

    #[test]
    fn obu_header_type_bits_land_in_positions_1_to_4() {
        // The obu_type field is bits 1..4 (MSB is 0/forbidden).
        assert_eq!(obu_header_byte(ObuType::SequenceHeader), 0x0a);
        assert_eq!(obu_header_byte(ObuType::TemporalDelimiter), 0x12);
        assert_eq!(obu_header_byte(ObuType::Frame), 0x32);
        assert_eq!(obu_header_byte(ObuType::TileGroup), 0x22);
    }
}
