//! Frozen actual FFmpeg VA callback captures. No device/display is opened.
use super::*;

fn buffer<T: Copy>(kind: VABufferType, params: &[T]) -> Buffer {
    let bytes = unsafe {
        std::slice::from_raw_parts(params.as_ptr().cast::<u8>(), std::mem::size_of_val(params))
    };
    Buffer {
        owner: VA_INVALID_ID,
        type_: kind,
        elem_size: std::mem::size_of::<T>() as u32,
        num_elements: params.len() as u32,
        data: bytes.to_vec(),
        mapped: false,
    }
}

fn assemble(
    decoder: &mut RawDecoder,
    pp: VADecPictureParameterBufferAV1,
    slices: &[VASliceParameterBufferAV1],
    data: &[u8],
    sequence: u64,
) -> Result<EncodedFrame, VAStatus> {
    decoder.begin_picture();
    decoder.render_buffer(&buffer(VABufferType::VAPictureParameterBufferType, &[pp]))?;
    decoder.render_buffer(&buffer(VABufferType::VASliceParameterBufferType, slices))?;
    decoder.render_buffer(&buffer(VABufferType::VASliceDataBufferType, data))?;
    decoder.finish_picture(sequence)
}

fn assemble_chromium_order(
    decoder: &mut RawDecoder,
    pp: VADecPictureParameterBufferAV1,
    slices: &[VASliceParameterBufferAV1],
    data: &[u8],
    sequence: u64,
) -> Result<EncodedFrame, VAStatus> {
    decoder.begin_picture();
    decoder.render_buffer(&buffer(VABufferType::VAPictureParameterBufferType, &[pp]))?;
    // Chromium MapAndCopyAndExecute submits the complete data before tiles.
    decoder.render_buffer(&buffer(VABufferType::VASliceDataBufferType, data))?;
    decoder.render_buffer(&buffer(VABufferType::VASliceParameterBufferType, slices))?;
    decoder.finish_picture(sequence)
}

#[test]
fn complete_data_before_tiles_stays_bounded_and_requires_finish_validation() {
    let mut decoder = RawDecoder::new_cbs_transport_for_test();
    decoder.begin_picture();
    let data = buffer(VABufferType::VASliceDataBufferType, &[0x12_u8, 0]);
    assert!(decoder.render_buffer(&data).is_ok());
    assert!(decoder.render_buffer(&data).is_err());
    assert!(decoder.finish_picture(0).is_err()); // Missing picture and tiles.
    decoder.begin_picture();
    assert!(
        decoder
            .render_buffer(&buffer::<u8>(VABufferType::VASliceDataBufferType, &[],))
            .is_err()
    );
}

#[test]
#[ignore = "requires frozen actual producer captures; run with AV1_TRANSPORT_HOST_FIXTURES"]
fn actual_producer_captures_match_driver_bytes_maps_and_fail_closed() {
    let root = std::path::PathBuf::from(
        std::env::var_os("AV1_TRANSPORT_HOST_FIXTURES").expect("capture directory"),
    );
    check_actual_captures(&root, &[300, 99, 96, 96]);
}

#[test]
#[ignore = "requires frozen YouTube format401 captures; run with AV1_YOUTUBE_HOST_FIXTURES"]
fn actual_youtube_4k_captures_match_driver_bytes_maps_and_fail_closed() {
    let root = std::path::PathBuf::from(
        std::env::var_os("AV1_YOUTUBE_HOST_FIXTURES").expect("YouTube capture directory"),
    );
    check_actual_captures(&root, &[375]);
}

#[test]
#[ignore = "requires generated six-display-frame 8K corpus; run with AV1_8K_HOST_FIXTURES"]
fn actual_eight_k_captures_match_driver_bytes_maps_and_fail_closed() {
    let root = std::path::PathBuf::from(
        std::env::var_os("AV1_8K_HOST_FIXTURES").expect("8K capture directory"),
    );
    // The fixed libaom fixture has seven coded generations (two hidden) and
    // six displayed frames, including one show_existing reference.
    check_actual_captures(&root, &[7]);
}

fn check_actual_captures(root: &std::path::Path, expected_counts: &[u64]) {
    let mut total = 0;
    let mut hidden = 0;
    for (sample, expected_count) in expected_counts.iter().enumerate() {
        let fixture = std::fs::read(root.join(format!("{sample}.bin"))).unwrap();
        let complete = &fixture[..8] == b"AV1VAO01";
        assert!(complete || &fixture[..8] == b"AV1VAH01");
        let normalized =
            complete.then(|| std::fs::read(root.join(format!("{sample}.normalized.bin"))).unwrap());
        let mut normalized_pos = 8;
        let mut pos = 8;
        let mut count = 0;
        let mut decoder = RawDecoder::new(Codec::Av1);
        decoder.av1_transport = Some(transport::Transport::new());
        let mut omitted_display = RawDecoder::new_cbs_transport_for_test();
        if complete {
            let path = std::env::var_os("AV1_COMPLETE_HOST_LIBRARY").expect("companion path");
            decoder.av1_transport = Some(transport::Transport::with_library(&path));
            omitted_display.av1_transport = Some(transport::Transport::with_library(&path));
        }
        while pos < fixture.len() {
            let fields: Vec<_> = fixture[pos..pos + 20]
                .chunks_exact(4)
                .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                .collect();
            pos += 20;
            assert_eq!(
                fields[0],
                std::mem::size_of::<VADecPictureParameterBufferAV1>()
            );
            assert_eq!(fields[1], std::mem::size_of::<VASliceParameterBufferAV1>());
            let pp = unsafe {
                ptr::read_unaligned(
                    fixture[pos..]
                        .as_ptr()
                        .cast::<VADecPictureParameterBufferAV1>(),
                )
            };
            pos += fields[0];
            let mut slices = Vec::new();
            for _ in 0..fields[2] {
                slices.push(unsafe {
                    ptr::read_unaligned(fixture[pos..].as_ptr().cast::<VASliceParameterBufferAV1>())
                });
                pos += fields[1];
            }
            let data = &fixture[pos..pos + fields[3]];
            pos += fields[3];
            // Rejected submissions must not commit sequence/ref-map state.
            let mut bad_slices = slices.clone();
            bad_slices[0].slice_data_offset = 0;
            assert!(assemble(&mut decoder, pp, &bad_slices, data, count).is_err());
            let mut bad_pp = pp;
            bad_pp.ref_frame_map[0] ^= 1;
            assert!(assemble(&mut decoder, bad_pp, &slices, data, count).is_err());
            let mut bad_data = data.to_vec();
            bad_data[0] |= 0x80;
            assert!(assemble(&mut decoder, pp, &slices, &bad_data, count).is_err());
            let mut foreign_display = pp;
            foreign_display.current_display_picture = pp.current_frame + 100;
            assert!(assemble(&mut decoder, foreign_display, &slices, data, count).is_err());
            if unsafe { pp.pic_info_fields.bits }.frame_type() == 1 {
                let mut alias = pp;
                alias.current_frame = *pp
                    .ref_frame_map
                    .iter()
                    .find(|id| **id != VA_INVALID_ID)
                    .unwrap();
                alias.current_display_picture = alias.current_frame;
                assert!(assemble(&mut decoder, alias, &slices, data, count).is_err());
            }
            let encoded = assemble(&mut decoder, pp, &slices, data, count).unwrap();
            let mut no_display = pp;
            no_display.current_display_picture = VA_INVALID_ID;
            let alternate =
                assemble_chromium_order(&mut omitted_display, no_display, &slices, data, count)
                    .unwrap();
            assert_eq!(alternate.bytes, encoded.bytes);
            if let Some(normalized) = &normalized {
                let fields: Vec<_> = normalized[normalized_pos..normalized_pos + 20]
                    .chunks_exact(4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
                    .collect();
                normalized_pos += 20 + fields[0] + fields[1] * fields[2];
                assert_eq!(
                    encoded.bytes,
                    normalized[normalized_pos..normalized_pos + fields[3]]
                );
                normalized_pos += fields[3];
            } else {
                assert_eq!(encoded.bytes, data);
            }
            assert!(encoded.expects_output && !encoded.headers.is_empty());
            assert_eq!(encoded.timestamp_usec, count * 33_333);
            assert_eq!(
                encoded.keyframe,
                unsafe { pp.pic_info_fields.bits }.frame_type() == 0
            );
            let mut seq = None;
            for unit in crate::av1::transport_prefix::obus(&encoded.headers).unwrap() {
                if unit.kind == 1 {
                    seq = Some(crate::av1::transport_prefix::sequence(unit.body).unwrap());
                }
            }
            let frame_obu = crate::av1::transport_prefix::obus(&encoded.bytes)
                .unwrap()
                .into_iter()
                .find(|o| o.kind == 6)
                .unwrap();
            let parsed =
                crate::av1::transport_prefix::frame(frame_obu.body, &seq.unwrap()).unwrap();
            assert_eq!(usize::from(parsed.refresh), fields[4]);
            if unsafe { pp.pic_info_fields.bits }.show_frame() == 0 {
                hidden += 1;
            }
            count += 1;
        }
        assert_eq!(pos, fixture.len());
        if let Some(normalized) = normalized {
            assert_eq!(normalized_pos, normalized.len());
        }
        assert_eq!(count, *expected_count);
        total += count;
    }
    assert_eq!(total, expected_counts.iter().sum::<u64>());
    assert!(hidden > 0);
    eprintln!("actual producer/driver assembly: {total} coded frames, {hidden} hidden; host only");
}

// First coded frame of the reported YouTube format 401 stream, copied without
// re-encoding. Its 32nd tile starts immediately after the 31st tile payload.
#[test]
fn youtube_4k_final_tile_has_no_size_prefix() {
    let fixture = include_bytes!("fixtures/youtube-format401-first-frame.bin");
    assert_eq!(&fixture[..8], b"AV1VAO01");
    let fields: Vec<_> = fixture[8..28]
        .chunks_exact(4)
        .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
        .collect();
    assert_eq!(
        fields[0],
        std::mem::size_of::<VADecPictureParameterBufferAV1>()
    );
    assert_eq!(fields[1], std::mem::size_of::<VASliceParameterBufferAV1>());
    let pp = unsafe {
        ptr::read_unaligned(
            fixture[28..]
                .as_ptr()
                .cast::<VADecPictureParameterBufferAV1>(),
        )
    };
    assert_eq!((pp.tile_cols, pp.tile_rows), (8, 4));
    let mut pos = 28 + fields[0];
    let mut slices = Vec::new();
    for _ in 0..fields[2] {
        slices.push(unsafe {
            ptr::read_unaligned(fixture[pos..].as_ptr().cast::<VASliceParameterBufferAV1>())
        });
        pos += fields[1];
    }
    let data = &fixture[pos..];
    assert_eq!(data.len(), fields[3]);
    assert_eq!(slices[31].slice_data_offset, 405);
    assert_eq!(
        slices[30].slice_data_offset + slices[30].slice_data_size,
        405
    );
    let mut decoder = RawDecoder::new_cbs_transport_for_test();
    for (tile, offset, size) in [
        (31, 404, 8), // Final tile overlaps its predecessor.
        (31, 406, 6), // Final tile must not leave a size-prefix gap.
        (1, 42, 13),  // Interior tile cannot omit its size prefix.
        (1, 47, 8),   // Interior size prefix cannot exceed four bytes.
        (31, 405, 0),
        (31, u32::MAX, 7),
    ] {
        let mut malformed = slices.clone();
        malformed[tile].slice_data_offset = offset;
        malformed[tile].slice_data_size = size;
        assert_eq!(
            assemble_chromium_order(&mut decoder, pp, &malformed, data, 0).err(),
            Some(err(VA_STATUS_ERROR_INVALID_PARAMETER)),
            "malformed tile {tile} offset {offset} size {size}",
        );
    }
    let mut wrong_index = slices.clone();
    wrong_index[31].tile_column = 6;
    assert!(assemble_chromium_order(&mut decoder, pp, &wrong_index, data, 0).is_err());
    let mut trailing = data.to_vec();
    trailing.push(0);
    assert!(assemble_chromium_order(&mut decoder, pp, &slices, &trailing, 0).is_err());
    // All rejected inputs above must leave sequence and reference state intact.
    let encoded = assemble_chromium_order(&mut decoder, pp, &slices, data, 0).unwrap();
    assert_eq!(encoded.bytes, data);
}

#[test]
#[ignore = "requires the frozen AKeUssuu3Is corpus"]
fn exact_user_4k_captures_match_driver_assembly() {
    let root = std::path::PathBuf::from(std::env::var_os("AV1_USER_HOST_FIXTURES").unwrap());
    check_actual_captures(&root, &[510]);
}

// Private offline audit: original source packets plus VA-shaped parameters.
// These are not captured live VA callbacks and prove no firmware behavior.
#[test]
#[ignore = "requires VP9_SOURCE_PACKET_IVF; opens ordinary files only"]
fn failing_vp9_first_two_source_packets_survive_collection_byte_exact() {
    let path = std::env::var_os("VP9_SOURCE_PACKET_IVF").expect("source IVF");
    let data = std::fs::read(path).unwrap();
    assert_eq!(&data[..4], b"DKIF");
    assert_eq!(&data[8..12], b"VP90");
    assert_eq!(u32::from_le_bytes(data[24..28].try_into().unwrap()), 2);
    let mut position = 32;
    let mut decoder = RawDecoder::new(Codec::Vp9);
    for (sequence, size) in [45669, 12533].into_iter().enumerate() {
        let length = u32::from_le_bytes(data[position..position + 4].try_into().unwrap()) as usize;
        assert_eq!(length, size);
        position += 12;
        let packet = &data[position..position + length];
        position += length;
        assert_eq!(vp9::hidden_reference(packet), Ok(None));
        let mut pp: VADecPictureParameterBufferVP9 = unsafe { std::mem::zeroed() };
        unsafe {
            pp.pic_fields.bits.set_frame_type(sequence as u32);
        }
        let mut slice: VASliceParameterBufferVP9 = unsafe { std::mem::zeroed() };
        slice.slice_data_offset = 13;
        slice.slice_data_size = length as u32;
        let mut padded = vec![0xee; 13];
        padded.extend_from_slice(packet);
        padded.extend_from_slice(&[0xdd; 17]);
        decoder.begin_picture();
        decoder
            .render_buffer(&buffer(VABufferType::VAPictureParameterBufferType, &[pp]))
            .unwrap();
        decoder
            .render_buffer(&buffer(VABufferType::VASliceParameterBufferType, &[slice]))
            .unwrap();
        decoder
            .render_buffer(&buffer(VABufferType::VASliceDataBufferType, &padded))
            .unwrap();
        let frame = decoder.finish_picture(sequence as u64).unwrap();
        assert_eq!(frame.bytes, packet);
        assert!(frame.headers.is_empty());
        assert_eq!(frame.keyframe, sequence == 0);
        assert!(frame.expects_output);
        assert_eq!(frame.vp9_show_existing, None);
    }
    assert_eq!(position, data.len());
}
