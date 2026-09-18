//! Minimal H.265 (HEVC) bitstream primitives: MSB-first bit writer/reader,
//! Exp-Golomb coding, and RBSP/EBSP/Annex-B conversion.
//!
//! NOTE on duplication: `h264::bitstream` owns nearly identical Exp-Golomb,
//! RBSP-to-EBSP, and start-code helpers, but those items are `pub(super)` to
//! the `h264` module and `h264.rs` is outside the Phase-5 track-C ownership
//! boundary. To avoid touching read-only files, these primitives are
//! deliberately re-implemented (and slimmed down: this writer grows its buffer
//! instead of tracking overflow). A later cleanup can lift them into one
//! shared internal module by widening visibility in `h264` only.

/// Annex-B start code used for assembled access units. H.265 byte streams
/// conventionally use the 3-byte form; a leading extra zero (4-byte form) is
/// also accepted by the splitter.
pub(super) const START_CODE: [u8; 3] = [0x00, 0x00, 0x01];

/// MSB-first bit writer with a growable backing buffer.
pub(super) struct BitWriter {
    buf: Vec<u8>,
    bit: u8,
}

impl BitWriter {
    pub(super) fn new() -> Self {
        Self {
            buf: Vec::new(),
            bit: 0,
        }
    }

    /// Write the low `nbits` of `val`, most-significant bit first.
    pub(super) fn put(&mut self, val: u64, nbits: u32) {
        debug_assert!(nbits <= 32);
        for i in (0..nbits).rev() {
            if self.bit == 0 {
                self.buf.push(0);
            }
            if (val >> i) & 1 != 0 {
                let last = self.buf.len() - 1;
                self.buf[last] |= 0x80 >> self.bit;
            }
            self.bit = (self.bit + 1) % 8;
        }
    }

    /// Unsigned Exp-Golomb: `k-1` zero bits, then the (k+1)-bit code of
    /// `val + 1`.
    pub(super) fn put_ue(&mut self, val: u32) {
        let x = u64::from(val) + 1;
        let k = 64 - x.leading_zeros();
        if k > 1 {
            self.put(0, k - 1);
        }
        self.put(x, k);
    }

    /// Signed Exp-Golomb per the H.265 `se(v)` mapping.
    pub(super) fn put_se(&mut self, val: i32) {
        let mapped: u64 = if val <= 0 {
            2 * (-(i64::from(val))) as u64
        } else {
            (2 * i64::from(val) - 1) as u64
        };
        debug_assert!(mapped <= u64::from(u32::MAX));
        self.put_ue(mapped as u32);
    }

    /// `rbsp_trailing_bits`: stop bit, then zero bits to the byte boundary.
    pub(super) fn rbsp_trailing(&mut self) {
        self.put(1, 1);
        while self.bit != 0 {
            self.put(0, 1);
        }
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.buf
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// MSB-first bit reader over an in-memory RBSP/EBSP slice.
pub(super) struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining_bits(&self) -> usize {
        self.data.len() * 8 - self.pos
    }

    /// Read `nbits` as an MSB-first unsigned value.
    pub(super) fn get(&mut self, nbits: u32) -> Result<u32, crate::h265::Error> {
        debug_assert!(nbits <= 32);
        if self.remaining_bits() < nbits as usize {
            return Err(crate::h265::Error::Truncated);
        }
        let mut val = 0u32;
        for _ in 0..nbits {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            val = (val << 1) | u32::from(bit);
            self.pos += 1;
        }
        Ok(val)
    }

    /// Skip `nbits` without materializing them (used for fields wider than
    /// one machine word, e.g. the 43-bit reserved PTL run).
    pub(super) fn skip(&mut self, nbits: u32) -> Result<(), crate::h265::Error> {
        if self.remaining_bits() < nbits as usize {
            return Err(crate::h265::Error::Truncated);
        }
        self.pos += nbits as usize;
        Ok(())
    }

    /// Unsigned Exp-Golomb.
    pub(super) fn ue(&mut self) -> Result<u32, crate::h265::Error> {
        let mut zeros = 0u32;
        while self.get(1)? == 0 {
            zeros += 1;
            if zeros >= 32 {
                return Err(crate::h265::Error::OutOfRange("exp-golomb prefix too long"));
            }
        }
        if zeros == 0 {
            return Ok(0);
        }
        let suffix = self.get(zeros)?;
        Ok((u64::from(1u32 << zeros) - 1 + u64::from(suffix)) as u32)
    }

    /// Signed Exp-Golomb per the H.265 `se(v)` mapping.
    pub(super) fn se(&mut self) -> Result<i32, crate::h265::Error> {
        let k = self.ue()?;
        let val: i64 = if k % 2 == 1 {
            (i64::from(k) + 1) / 2
        } else {
            -(i64::from(k) / 2)
        };
        i32::try_from(val).map_err(|_| crate::h265::Error::OutOfRange("se(v) out of i32 range"))
    }

    /// Read and discard one unsigned Exp-Golomb value.
    pub(super) fn skip_ue(&mut self) -> Result<(), crate::h265::Error> {
        self.ue().map(|_| ())
    }
}

/// Insert emulation-prevention bytes: inside a NAL unit payload, any run of
/// two zero bytes followed by a byte <= 0x03 gets a `0x03` inserted, so the
/// payload can never contain a start code. A trailing zero pair (as produced
/// by `cabac_zero_words`) needs no escape and is preserved as-is.
pub(super) fn rbsp_to_ebsp(src: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len() + src.len() / 2);
    let mut zeros = 0usize;
    for &b in src {
        if zeros >= 2 && b <= 3 {
            dst.push(0x03);
            zeros = 0;
        }
        dst.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    dst
}

/// Strip emulation-prevention bytes, returning the RBSP payload.
///
/// Raw `00 00 00/01/02` inside a NAL payload is impossible in a valid Annex-B
/// stream (it would have terminated the NAL unit), so it is reported as
/// [`crate::h265::Error::EmulationSequence`] rather than silently accepted. A
/// dangling `00 00 03` at the end of the payload is a truncated escape and is
/// reported as [`crate::h265::Error::Truncated`].
pub(super) fn ebsp_to_rbsp(src: &[u8]) -> Result<Vec<u8>, crate::h265::Error> {
    let mut dst = Vec::with_capacity(src.len());
    let mut zeros = 0usize;
    let mut i = 0;
    while i < src.len() {
        let b = src[i];
        if zeros >= 2 {
            match b {
                0x03 => {
                    i += 1;
                    if i == src.len() {
                        return Err(crate::h265::Error::Truncated);
                    }
                    zeros = 0;
                    continue;
                }
                0x00..=0x02 => return Err(crate::h265::Error::EmulationSequence),
                _ => zeros = 0,
            }
        }
        dst.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
        i += 1;
    }
    Ok(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_reader_round_trip_bits_and_exp_golomb() {
        let mut bw = BitWriter::new();
        bw.put(0b101, 3);
        bw.put_ue(0);
        bw.put_ue(1);
        bw.put_ue(300);
        bw.put_se(-7);
        bw.put_se(0);
        bw.put_se(13);
        bw.rbsp_trailing();

        let mut br = BitReader::new(bw.bytes());
        assert_eq!(br.get(3).unwrap(), 0b101);
        assert_eq!(br.ue().unwrap(), 0);
        assert_eq!(br.ue().unwrap(), 1);
        assert_eq!(br.ue().unwrap(), 300);
        assert_eq!(br.se().unwrap(), -7);
        assert_eq!(br.se().unwrap(), 0);
        assert_eq!(br.se().unwrap(), 13);
        // 41 syntax bits, then the trailing stop bit and 6 padding zeros.
        assert_eq!(br.get(1).unwrap(), 1);
        assert_eq!(br.skip(6).unwrap(), ());
        assert_eq!(br.get(1), Err(crate::h265::Error::Truncated));
    }

    #[test]
    fn reader_truncation_returns_clean_errors() {
        let mut br = BitReader::new(&[0b1010_1010]);
        assert_eq!(br.get(9), Err(crate::h265::Error::Truncated));
        assert_eq!(br.get(8).unwrap(), 0b1010_1010);
        assert_eq!(br.get(1), Err(crate::h265::Error::Truncated));

        // An all-zero byte cannot complete a ue() code.
        let mut br = BitReader::new(&[0x00]);
        assert_eq!(br.ue(), Err(crate::h265::Error::Truncated));
        assert_eq!(BitReader::new(&[]).ue(), Err(crate::h265::Error::Truncated));
        assert_eq!(BitReader::new(&[]).se(), Err(crate::h265::Error::Truncated));
    }

    #[test]
    fn reader_rejects_overlong_exp_golomb_prefix() {
        // 32 leading zero bits: no representable ue(v) value.
        let mut br = BitReader::new(&[0x00, 0x00, 0x00, 0x00]);
        assert_eq!(
            br.ue(),
            Err(crate::h265::Error::OutOfRange("exp-golomb prefix too long"))
        );
    }

    #[test]
    fn rbsp_to_ebsp_inserts_emulation_prevention() {
        // 00 00 00 -> 00 00 03 00 (the following 01 now sits behind a single
        // zero, so it needs no escape of its own).
        assert_eq!(
            rbsp_to_ebsp(&[0x00, 0x00, 0x00, 0x01, 0xFF]),
            vec![0x00, 0x00, 0x03, 0x00, 0x01, 0xFF]
        );
        // 00 00 02 and 00 00 03 also get the escape.
        assert_eq!(
            rbsp_to_ebsp(&[0x00, 0x00, 0x02]),
            vec![0x00, 0x00, 0x03, 0x02]
        );
        assert_eq!(
            rbsp_to_ebsp(&[0x00, 0x00, 0x03]),
            vec![0x00, 0x00, 0x03, 0x03]
        );
        // Plain data passes through unchanged.
        assert_eq!(
            rbsp_to_ebsp(&[0xAA, 0x55, 0x00, 0xAA]),
            vec![0xAA, 0x55, 0x00, 0xAA]
        );
        // A trailing zero pair (cabac_zero_words style) needs no escape.
        assert_eq!(rbsp_to_ebsp(&[0x42, 0x00, 0x00]), vec![0x42, 0x00, 0x00]);
    }

    #[test]
    fn ebsp_round_trips_adversarial_payload() {
        let rbsp: Vec<u8> = [
            0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0xFF, 0x00, 0x00, 0x02, 0x00,
        ]
        .iter()
        .copied()
        .chain((0u8..=255).step_by(7))
        .collect();
        let ebsp = rbsp_to_ebsp(&rbsp);
        // The escaped payload can never contain a start code.
        assert!(!ebsp.windows(3).any(|w| w == [0x00, 0x00, 0x01]));
        assert_eq!(ebsp_to_rbsp(&ebsp).unwrap(), rbsp);
    }

    #[test]
    fn ebsp_to_rbsp_rejects_raw_zero_sequences() {
        for bad in [
            &[0x00u8, 0x00, 0x01, 0xFF][..],
            &[0x00, 0x00, 0x00][..],
            &[0x00, 0x00, 0x02][..],
        ] {
            assert_eq!(
                ebsp_to_rbsp(bad),
                Err(crate::h265::Error::EmulationSequence)
            );
        }
        // A dangling escape byte at the very end is truncated.
        assert_eq!(
            ebsp_to_rbsp(&[0x00, 0x00, 0x03]),
            Err(crate::h265::Error::Truncated)
        );
        // Trailing zero pairs are legal payload and survive.
        assert_eq!(
            ebsp_to_rbsp(&[0x01, 0x00, 0x00]).unwrap(),
            vec![0x01, 0x00, 0x00]
        );
    }
}
