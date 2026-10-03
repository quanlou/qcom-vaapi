//! Host-only actual prefix reader harness: never opens a decoder or VA display.
#[path = "../rust/src/av1/transport_prefix.rs"]
mod prefix;
use std::{env, fs};

fn inspect(data: &[u8]) -> Result<Vec<String>, prefix::Error> {
    if data.len() < 32 || &data[..4] != b"DKIF" || &data[8..12] != b"AV01" {
        return Err(prefix::Error::Invalid);
    }
    let mut pos = 32usize;
    let mut seq = None;
    let mut rows = Vec::new();
    while pos < data.len() {
        let header = data.get(pos..pos + 12).ok_or(prefix::Error::Truncated)?;
        let size = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
        pos += 12;
        let end = pos.checked_add(size).ok_or(prefix::Error::Invalid)?;
        let packet = data.get(pos..end).ok_or(prefix::Error::Truncated)?;
        let mut frames = 0;
        for obu in prefix::obus(packet)? {
            match obu.kind {
                1 if frames == 0 => {
                    seq = Some(prefix::sequence(obu.body)?);
                }
                2 if frames == 0 && obu.body.is_empty() => {}
                6 if frames == 0 => {
                    let seq = seq.as_ref().ok_or(prefix::Error::Invalid)?;
                    let frame = prefix::frame(obu.body, seq)?;
                    if obu.body_offset + frame.prefix_bits.div_ceil(8) >= obu.end {
                        return Err(prefix::Error::Truncated);
                    }
                    rows.push(format!("{{\"frame_type\":{},\"refresh\":{},\"order_hint\":{},\"primary_ref\":{},\"error_resilient\":{},\"prefix_bits\":{},\"width\":{},\"height\":{},\"bit_depth\":{}}}",
                        frame.frame_type, frame.refresh, frame.order_hint, frame.primary_ref,
                        frame.error_resilient, frame.prefix_bits, seq.width, seq.height, seq.bit_depth));
                    frames += 1;
                }
                _ => return Err(prefix::Error::Unsupported),
            }
        }
        if frames != 1 {
            return Err(prefix::Error::Invalid);
        }
        pos = end;
    }
    Ok(rows)
}

fn main() {
    let args: Vec<_> = env::args_os().collect();
    if args.len() != 2 {
        std::process::exit(2);
    }
    let bytes = fs::read(&args[1]).unwrap();
    match inspect(&bytes) {
        Ok(rows) => {
            for row in rows {
                println!("{row}");
            }
        }
        Err(error) => {
            eprintln!("prefix validation failed: {error:?}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framing_rejects_unbounded_truncated_reserved_and_layered_inputs() {
        for bytes in [
            &[][..],
            &[0x32][..],
            &[0xb2, 0][..],
            &[0x33, 0][..],
            &[0x36, 0x20, 0][..],
            &[0x32, 0x80][..],
            &[0x32, 9, 0][..],
        ] {
            assert!(prefix::obus(bytes).is_err());
        }
        assert!(prefix::obus(&[0x32, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]).is_err());
    }
    #[test]
    fn random_small_input_and_all_truncations_never_panic() {
        let mut seed = 42u32;
        for length in 0..256 {
            let data: Vec<_> = (0..length)
                .map(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = prefix::obus(&data);
            let _ = prefix::sequence(&data);
            let seq = prefix::Sequence {
                width: 1280,
                height: 720,
                bit_depth: 8,
                order_bits: 8,
                screen_tools: 2,
                integer_mv: 2,
            };
            let _ = prefix::frame(&data, &seq);
        }
    }
}
