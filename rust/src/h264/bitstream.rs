//! Small H.264 bitstream helpers.
//!
//! The VA API gives the driver parsed H.264 fields, while the stateful V4L2
//! decoder expects Annex-B bytes. This module owns the pure byte-writing
//! pieces shared by SPS/PPS synthesis: bit packing, Exp-Golomb coding,
//! emulation-prevention escaping, and NAL start-code wrapping.

pub(super) const START_CODE: [u8; 4] = [0x00, 0x00, 0x00, 0x01];

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
