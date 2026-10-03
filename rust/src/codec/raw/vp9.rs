//! Bridge VP9 hidden references to stateful output. VA clients retain pixels
//! for later show_existing_frame, even though Iris omits invisible output.

pub(super) fn hidden_reference(data: &[u8]) -> Result<Option<u8>, ()> {
    let mut pos = 0usize;
    let mut bits = |n: usize| -> Result<u32, ()> {
        let mut value = 0;
        for _ in 0..n {
            let byte = *data.get(pos / 8).ok_or(())?;
            value = (value << 1) | u32::from((byte >> (7 - pos % 8)) & 1);
            pos += 1;
        }
        Ok(value)
    };
    if bits(2)? != 2 || bits(2)? != 0 {
        return Err(());
    }
    if bits(1)? != 0 {
        return Ok(None);
    }
    let inter = bits(1)? != 0;
    let shown = bits(1)? != 0;
    let resilient = bits(1)? != 0;
    if shown {
        return Ok(None);
    }
    if !inter {
        if bits(24)? != 0x498342 {
            return Err(());
        }
        return Ok(Some(0));
    } // Keyframes refresh every reference.
    let intra_only = bits(1)? != 0;
    if !resilient {
        bits(2)?;
    }
    if intra_only && bits(24)? != 0x498342 {
        return Err(());
    }
    let refresh = bits(8)?;
    if refresh == 0 {
        return Err(());
    }
    Ok(Some(refresh.trailing_zeros() as u8))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(bits: &str) -> Vec<u8> {
        let mut data = vec![0; bits.len().div_ceil(8)];
        for (pos, bit) in bits.bytes().enumerate() {
            assert!(bit == b'0' || bit == b'1');
            data[pos / 8] |= (bit - b'0') << (7 - pos % 8);
        }
        data
    }

    #[test]
    fn real_youtube_headers_preserve_hidden_reference_slot() {
        assert_eq!(hidden_reference(&[0x82]), Ok(None));
        assert_eq!(hidden_reference(&[0x84, 0x00, 0x40]), Ok(Some(1)));
        assert_eq!(hidden_reference(&[0x86]), Ok(None));
        assert_eq!(hidden_reference(&[0x8f]), Ok(None));
        assert!(hidden_reference(&[0x84]).is_err());
        assert!(hidden_reference(&[0x84, 0, 0]).is_err());
    }

    #[test]
    fn hidden_headers_select_a_refreshed_slot_without_changing_frame_bits() {
        // Profile zero, inter, hidden, resilient, not intra-only, slot 7.
        assert_eq!(hidden_reference(&packed("10000101010000000")), Ok(Some(7)));
        // Hidden intra-only: reset context, sync code, refresh slots 2 and 4.
        let intra = packed(concat!(
            "10000100",
            "1",
            "00",
            "010010011000001101000010",
            "00010100"
        ));
        assert_eq!(hidden_reference(&intra), Ok(Some(2)));
        let key = [0x80, 0x49, 0x83, 0x42];
        assert_eq!(hidden_reference(&key), Ok(Some(0)));
        assert!(hidden_reference(&[0x80]).is_err());
        assert!(hidden_reference(&[0x80, 0, 0, 0]).is_err());
        assert!(hidden_reference(&[0xa6]).is_err()); // Profile 1.
        assert!(hidden_reference(&[]).is_err());
    }
}
