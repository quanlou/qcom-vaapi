//! Small H.264 bitstream helpers.
//!
//! The VA API gives the driver parsed H.264 fields, while the stateful V4L2
//! decoder expects Annex-B bytes. This module owns the pure byte-writing
//! pieces shared by SPS/PPS synthesis: bit packing, Exp-Golomb coding,
//! emulation-prevention escaping, and NAL start-code wrapping.

pub(super) const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

/// Read the parameter-set identity omitted from VA's parsed picture fields.
/// Only the bounded slice prefix is needed; retain the compressed payload.
pub(super) fn slice_pps_id(data: &[u8]) -> Option<u32> {
    let header = *data.first()?;
    if header & 0x80 != 0
        || !matches!(header & 0x1f, 1 | 5)
        || (header & 0x1f == 5 && header & 0x60 == 0)
    {
        return None;
    }
    let mut rbsp = Vec::with_capacity(32);
    let mut zeros = 0;
    for &byte in data.iter().skip(1).take(32) {
        if zeros == 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut bit = 0;
    let mut read_bit = || {
        let byte = *rbsp.get(bit / 8)?;
        let value = (byte >> (7 - bit % 8)) & 1;
        bit += 1;
        Some(u32::from(value))
    };
    let mut read_ue = || {
        let mut zeros = 0;
        while read_bit()? == 0 {
            zeros += 1;
            if zeros >= 32 {
                return None;
            }
        }
        let mut value = 1u32;
        for _ in 0..zeros {
            value = (value << 1) | read_bit()?;
        }
        Some(value - 1)
    };
    let _first_mb = read_ue()?;
    if read_ue()? > 9 {
        return None;
    }
    read_ue().filter(|id| *id <= 255)
}

pub(super) struct BitWriter {
    buf: Vec<u8>,
    byte: usize,
    bit: u8,
    ovf: bool,
}

impl BitWriter {
    pub(super) fn new(cap: usize) -> Self {
        Self {
            buf: vec![0; cap],
            byte: 0,
            bit: 0,
            ovf: false,
        }
    }

    pub(super) fn put(&mut self, val: u32, nbits: i32) {
        if self.ovf {
            return;
        }
        if !(0..=32).contains(&nbits) {
            self.ovf = true;
            return;
        }
        for i in (0..nbits).rev() {
            if self.byte >= self.buf.len() {
                self.ovf = true;
                return;
            }
            if ((val >> i) & 1) != 0 {
                self.buf[self.byte] |= 0x80 >> self.bit;
            }
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.byte += 1;
            }
        }
    }

    pub(super) fn put_ue(&mut self, v: u32) {
        let Some(x) = v.checked_add(1) else {
            self.ovf = true;
            return;
        };
        let k = 32 - x.leading_zeros();
        if k > 16 {
            self.ovf = true;
            return;
        }
        self.put(x, (2 * k - 1) as i32);
    }

    pub(super) fn put_se(&mut self, v: i32) {
        let mapped = if v <= 0 {
            (-(i64::from(v)) * 2) as u64
        } else {
            (i64::from(v) * 2 - 1) as u64
        };
        if mapped > u64::from(u32::MAX) {
            self.ovf = true;
            return;
        }
        self.put_ue(mapped as u32);
    }

    pub(super) fn rbsp_trailing(&mut self) -> bool {
        self.put(1, 1);
        while self.bit != 0 && !self.ovf {
            self.put(0, 1);
        }
        !self.ovf
    }

    pub(super) fn bytes(&self) -> &[u8] {
        &self.buf[..self.byte + usize::from(self.bit != 0)]
    }
}

fn rbsp_to_ebsp(src: &[u8]) -> Vec<u8> {
    let mut dst = Vec::with_capacity(src.len().saturating_add(src.len() / 2));
    let mut zeros = 0;
    for &b in src {
        if zeros == 2 && b <= 3 {
            dst.push(0x03);
            zeros = 0;
        }
        dst.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    dst
}

pub(super) fn emit_nal(nal_hdr: u8, rbsp: &[u8]) -> Option<Vec<u8>> {
    if rbsp.is_empty() {
        return None;
    }
    let ebsp = rbsp_to_ebsp(rbsp);
    let mut out = Vec::with_capacity(START_CODE.len() + 1 + ebsp.len());
    out.extend_from_slice(&START_CODE);
    out.push(nal_hdr);
    out.extend_from_slice(&ebsp);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_parameter_identity_handles_nonzero_ids_and_escaping() {
        for id in [0, 1, 31, 255] {
            let mut writer = BitWriter::new(32);
            writer.put(0, 31);
            writer.put(u32::MAX, 32); // large first_mb forces an escaped prefix
            writer.put_ue(2);
            writer.put_ue(id);
            assert!(writer.rbsp_trailing());
            let nal = emit_nal(0x65, writer.bytes()).unwrap();
            assert!(nal[4..].windows(3).any(|bytes| bytes == [0, 0, 3]));
            assert_eq!(slice_pps_id(&nal[4..]), Some(id));
        }
    }

    #[test]
    fn slice_parameter_identity_rejects_truncation_and_out_of_range_ids() {
        for bytes in [&[][..], &[0x65][..], &[0x65, 0][..], &[0xe5, 0xb8][..]] {
            assert_eq!(slice_pps_id(bytes), None);
        }
        let mut writer = BitWriter::new(32);
        writer.put_ue(0);
        writer.put_ue(2);
        writer.put_ue(256);
        assert!(writer.rbsp_trailing());
        let nal = emit_nal(0x65, writer.bytes()).unwrap();
        assert_eq!(slice_pps_id(&nal[4..]), None);
    }

    #[test]
    fn bit_writer_rejects_unrepresentable_values_without_panicking() {
        let mut writer = BitWriter::new(8);
        writer.put_ue(u32::MAX);
        assert!(writer.ovf);

        let mut writer = BitWriter::new(8);
        writer.put_se(i32::MIN);
        assert!(writer.ovf);

        let mut writer = BitWriter::new(8);
        writer.put(0, 33);
        assert!(writer.ovf);
        assert!(!writer.rbsp_trailing());
    }
}
