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
    pitch.saturating_mul(height).saturating_mul(3) / 2
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
) {
    let bytes_per_sample = format.bytes_per_sample() as usize;
    let cap_stride = cap_stride as usize;
    let cap_h = cap_h as usize;
    let img_pitch = img_pitch as usize;
    let img_plane1_off = img_plane1_off as usize;
    let x_bytes = x.saturating_mul(bytes_per_sample);
    let w_bytes = w.saturating_mul(bytes_per_sample);
    for row in 0..h {
        let src_off = (y + row).saturating_mul(cap_stride).saturating_add(x_bytes);
        let dst_off = row.saturating_mul(img_pitch);
        let Some(src_end) = src_off.checked_add(w_bytes) else {
            continue;
        };
        let Some(dst_end) = dst_off.checked_add(w_bytes) else {
            continue;
        };
        if src_end <= cap.len() && dst_end <= dst.len() {
            dst[dst_off..dst_end].copy_from_slice(&cap[src_off..src_end]);
        }
    }
    let chroma_src = cap_stride.saturating_mul(cap_h);
    for row in 0..(h / 2) {
        let src_off = chroma_src
            .saturating_add((y / 2 + row).saturating_mul(cap_stride))
            .saturating_add(x_bytes);
        let dst_off = img_plane1_off.saturating_add(row.saturating_mul(img_pitch));
        let Some(src_end) = src_off.checked_add(w_bytes) else {
            continue;
        };
        let Some(dst_end) = dst_off.checked_add(w_bytes) else {
            continue;
        };
        if src_end <= cap.len() && dst_end <= dst.len() {
            dst[dst_off..dst_end].copy_from_slice(&cap[src_off..src_end]);
        }
    }
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
    if x < 0 || y < 0 || width == 0 || height == 0 {
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
            1,
            4,
            2,
        );

        assert_eq!(&image[0..4], &[10, 11, 12, 13]);
        assert_eq!(&image[4..8], &[18, 19, 20, 21]);
        assert_eq!(&image[12..16], &[34, 35, 36, 37]);
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
}
