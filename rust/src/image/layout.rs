//! Semiplanar 4:2:0 image layout and bounded CPU-copy helpers.
//!
//! This module contains no driver state or FFI callbacks. Keeping the stride,
//! plane-offset, and rectangle checks here makes the image entrypoints easier
//! to audit at the VA boundary.

use crate::bindings::*;
use crate::pixel_format::DecodedFormat;
use std::ffi::c_int;

pub(crate) fn image_format(format: DecodedFormat) -> VAImageFormat {
    VAImageFormat {
        fourcc: format.va_fourcc(),
        byte_order: VA_LSB_FIRST,
        bits_per_pixel: format.bits_per_pixel(),
        depth: format.depth(),
        red_mask: 0,
        green_mask: 0,
        blue_mask: 0,
        alpha_mask: 0,
        va_reserved: [0; 4],
    }
}

pub(crate) fn decoded_format_from_image(format: &VAImageFormat) -> Option<DecodedFormat> {
    match format.fourcc {
        VA_FOURCC_NV12 => Some(DecodedFormat::Nv12),
        VA_FOURCC_P010 => Some(DecodedFormat::P010),
        _ => None,
    }
}

pub(crate) fn aligned_pitch(format: DecodedFormat, width: u32) -> u32 {
    width
        .saturating_mul(format.bytes_per_sample())
        .saturating_add(127)
        & !127
}

pub(crate) fn image_data_size(pitch: u32, height: u32) -> u32 {
    // Subsampled chroma needs a whole row for an odd final luma row.
    pitch.saturating_mul(height.saturating_add(height.div_ceil(2)))
}

pub(crate) fn make_image(
    image_id: VAImageID,
    buffer_id: VABufferID,
    width: u16,
    height: u16,
    pitch: u32,
    storage_height: u32,
    format: VAImageFormat,
) -> VAImage {
    // VAImage is a libva ABI POD whose reserved fields must be zero. The
    // generated bindings do not provide a constructor or Default impl.
    let mut image: VAImage = unsafe { std::mem::zeroed() };
    image.image_id = image_id;
    image.format = format;
    image.buf = buffer_id;
    image.width = width;
    image.height = height;
    image.data_size = image_data_size(pitch, storage_height);
    image.num_planes = 2;
    image.pitches[0] = pitch;
    image.pitches[1] = pitch;
    image.offsets[0] = 0;
    image.offsets[1] = pitch.saturating_mul(storage_height);
    image
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn copy_semiplanar_region(
    format: DecodedFormat,
    cap: &[u8],
    cap_stride: u32,
    cap_h: u32,
    dst: &mut [u8],
    img_pitch: u32,
    img_plane1_off: u32,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
) -> bool {
    let bytes_per_sample = format.bytes_per_sample() as usize;
    let cap_stride = cap_stride as usize;
    let cap_h = cap_h as usize;
    let img_pitch = img_pitch as usize;
    let img_plane1_off = img_plane1_off as usize;
    let Some((x_bytes, y_end, luma_bytes, chroma_bytes)) = x
        .checked_mul(bytes_per_sample)
        .zip(y.checked_add(h))
        .zip(w.checked_mul(bytes_per_sample))
        .zip(
            w.div_ceil(2)
                .checked_mul(2)
                .and_then(|n| n.checked_mul(bytes_per_sample)),
        )
        .map(|(((x_bytes, y_end), luma_bytes), chroma_bytes)| {
            (x_bytes, y_end, luma_bytes, chroma_bytes)
        })
    else {
        return false;
    };
    let Some(source_size) = cap_h
        .checked_add(cap_h.div_ceil(2))
        .and_then(|rows| rows.checked_mul(cap_stride))
    else {
        return false;
    };
    let Some(luma_size) = h.checked_mul(img_pitch) else {
        return false;
    };
    let Some(destination_size) = h
        .div_ceil(2)
        .checked_mul(img_pitch)
        .and_then(|size| img_plane1_off.checked_add(size))
    else {
        return false;
    };
    if w == 0
        || h == 0
        || !x.is_multiple_of(2)
        || !y.is_multiple_of(2)
        || y_end > cap_h
        || source_size > cap.len()
        || x_bytes
            .checked_add(chroma_bytes)
            .is_none_or(|end| end > cap_stride)
        || chroma_bytes > img_pitch
        || img_plane1_off < luma_size
        || destination_size > dst.len()
    {
        // Validate the entire operation before touching the destination.
        // Silent partial copies must never be reported as successful frames.
        return false;
    }
    for row in 0..h {
        let src_off = (y + row) * cap_stride + x_bytes;
        let dst_off = row * img_pitch;
        dst[dst_off..dst_off + luma_bytes].copy_from_slice(&cap[src_off..src_off + luma_bytes]);
    }
    let chroma_src = cap_stride * cap_h;
    for row in 0..h.div_ceil(2) {
        let src_off = chroma_src + (y / 2 + row) * cap_stride + x_bytes;
        let dst_off = img_plane1_off + row * img_pitch;
        dst[dst_off..dst_off + chroma_bytes].copy_from_slice(&cap[src_off..src_off + chroma_bytes]);
    }
    true
}

pub(crate) fn region_within(
    surface: (u32, u32),
    image: (u32, u32),
    origin: (c_int, c_int),
    size: (u32, u32),
) -> bool {
    let (surface_width, surface_height) = surface;
    let (image_width, image_height) = image;
    let (x, y) = origin;
    let (width, height) = size;
    // Arbitrary chroma phase shifts require resampling, which this raw-copy
    // path does not implement. Never shift the interleaved U/V pair by a byte.
    if x < 0 || y < 0 || x % 2 != 0 || y % 2 != 0 || width == 0 || height == 0 {
        return false;
    }
    let x = x as u32;
    let y = y as u32;
    let Some(x_end) = x.checked_add(width) else {
        return false;
    };
    let Some(y_end) = y.checked_add(height) else {
        return false;
    };
    x_end <= surface_width
        && y_end <= surface_height
        && width <= image_width
        && height <= image_height
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_aligned_nv12_image_layout() {
        let image = make_image(
            0x7000_0000,
            0x6000_0000,
            130,
            16,
            aligned_pitch(DecodedFormat::Nv12, 130),
            16,
            image_format(DecodedFormat::Nv12),
        );

        assert_eq!(image.format.fourcc, VA_FOURCC_NV12);
        assert_eq!(image.pitches[0], 256);
        assert_eq!(image.pitches[1], 256);
        assert_eq!(image.offsets[0], 0);
        assert_eq!(image.offsets[1], 4096);
        assert_eq!(image.data_size, 6144);
    }

    #[test]
    fn builds_aligned_p010_image_layout() {
        let image = make_image(
            0x7000_0000,
            0x6000_0000,
            130,
            16,
            aligned_pitch(DecodedFormat::P010, 130),
            16,
            image_format(DecodedFormat::P010),
        );

        assert_eq!(image.format.fourcc, VA_FOURCC_P010);
        assert_eq!(image.pitches[0], 384);
        assert_eq!(image.pitches[1], 384);
        assert_eq!(image.offsets[1], 6144);
        assert_eq!(image.data_size, 9216);
    }

    #[test]
    fn copies_luma_and_chroma_region() {
        let mut capture = vec![0u8; 8 * 6];
        for (i, byte) in capture.iter_mut().enumerate() {
            *byte = i as u8;
        }
        let mut image = vec![0u8; 4 * 3 + 4 * 2];

        copy_semiplanar_region(
            DecodedFormat::Nv12,
            &capture,
            8,
            4,
            &mut image,
            4,
            12,
            2,
            2,
            4,
            2,
        );

        assert_eq!(&image[0..4], &[18, 19, 20, 21]);
        assert_eq!(&image[4..8], &[26, 27, 28, 29]);
        assert_eq!(&image[12..16], &[42, 43, 44, 45]);
    }

    #[test]
    fn rejects_regions_outside_surface_or_destination_image() {
        assert!(region_within((64, 64), (32, 32), (0, 0), (32, 32)));
        assert!(!region_within((64, 64), (32, 32), (0, 0), (33, 32)));
        assert!(!region_within((64, 64), (64, 64), (60, 0), (5, 4)));
        assert!(!region_within((64, 64), (64, 64), (i32::MAX, 0), (8, 4)));
        assert!(!region_within((64, 64), (64, 64), (-1, 0), (1, 1)));
    }

    #[test]
    fn copy_region_ignores_overflowing_offsets() {
        let mut image = vec![0_u8; 8];
        copy_semiplanar_region(
            DecodedFormat::Nv12,
            &[1, 2, 3, 4],
            u32::MAX,
            u32::MAX,
            &mut image,
            u32::MAX,
            u32::MAX,
            0,
            0,
            8,
            2,
        );
        assert_eq!(image, vec![0; 8]);
    }
    #[test]
    fn odd_sized_images_copy_the_complete_final_chroma_row_and_uv_pair() {
        for format in [DecodedFormat::Nv12, DecodedFormat::P010] {
            let pitch = aligned_pitch(format, 17);
            let rows = 17 + 9;
            assert_eq!(image_data_size(pitch, 17), pitch * rows);
            let source: Vec<u8> = (0..pitch * rows)
                .map(|i| (i as u8).wrapping_add(1))
                .collect();
            let mut dest = vec![0xEE; source.len()];
            copy_semiplanar_region(
                format,
                &source,
                pitch,
                17,
                &mut dest,
                pitch,
                pitch * 17,
                0,
                0,
                17,
                17,
            );
            let start = (pitch * 25) as usize;
            let chroma_bytes = 18 * format.bytes_per_sample() as usize;
            assert_eq!(
                &dest[start..start + chroma_bytes],
                &source[start..start + chroma_bytes]
            );
            let luma_bytes = 17 * format.bytes_per_sample() as usize;
            assert_eq!(&dest[..luma_bytes], &source[..luma_bytes]);
            assert_eq!(dest[luma_bytes], 0xEE);
        }
    }

    #[test]
    fn truncated_copy_leaves_the_whole_destination_untouched() {
        let source = vec![0xA5; 128 * 24 - 1];
        let mut dest = vec![0xEE; 128 * 24];
        copy_semiplanar_region(
            DecodedFormat::Nv12,
            &source,
            128,
            16,
            &mut dest,
            128,
            128 * 16,
            0,
            0,
            16,
            16,
        );
        assert!(dest.iter().all(|byte| *byte == 0xEE));
    }

    #[test]
    fn crop_rejects_chroma_phase_changes_that_would_swap_uv_bytes() {
        assert!(!region_within((32, 32), (16, 16), (1, 0), (16, 16)));
        assert!(!region_within((32, 32), (16, 16), (0, 1), (16, 16)));
        assert!(region_within((32, 32), (17, 17), (2, 2), (17, 17)));
    }
}
