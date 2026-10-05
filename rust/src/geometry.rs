//! Visible-frame bounds shared by capability reporting and pixel storage.
//!
//! The experimental variant admits Iris's 8K macroblock envelope. A maximum
//! side alone would also admit 8192-square frames, which exceed that envelope.

pub(crate) const MIN_DIM: i32 = 16;
pub(crate) const MAX_DIM: i32 = if cfg!(feature = "experimental-8k") {
    8192
} else {
    4096
};
const MAX_MACROBLOCKS: u32 = (8192 / 16) * (4352 / 16);

pub(crate) fn valid_dimensions(width: u32, height: u32) -> bool {
    let range = MIN_DIM as u32..=MAX_DIM as u32;
    range.contains(&width)
        && range.contains(&height)
        && width
            .div_ceil(16)
            .checked_mul(height.div_ceil(16))
            .is_some_and(|blocks| blocks <= MAX_MACROBLOCKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_envelope_checks_sides_and_rounded_area() {
        assert!(valid_dimensions(3840, 2160));
        assert!(valid_dimensions(4096, 4096));
        for (w, h) in [
            (0, 16),
            (15, 16),
            (16, 15),
            (8193, 16),
            (16, 8193),
            (8192, 8192),
            (u32::MAX, 16),
            (16, u32::MAX),
        ] {
            assert!(!valid_dimensions(w, h));
        }
        let enabled = cfg!(feature = "experimental-8k");
        for (w, h) in [(7680, 4320), (8192, 4352), (4352, 8192)] {
            assert_eq!(valid_dimensions(w, h), enabled);
        }
        // One extra row consumes another macroblock row, even when the raw
        // pixel count might appear close enough to the boundary.
        assert!(!valid_dimensions(8192, 4353));
    }
}
