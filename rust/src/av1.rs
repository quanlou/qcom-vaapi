//! AV1 access-unit synthesis for the stateful V4L2 decoder.
//!
//! AV1 uses OBU (Open Bitstream Unit) framing rather than H.264/HEVC's
//! Annex-B start codes. VA-API delivers picture parameters and per-tile
//! payloads separately (`VADecPictureParameterBufferAV1` +
//! `VASliceDataBufferAV1`), so before iris can decode a frame the driver
//! must rebuild:
//!
//!   1. Temporal Delimiter OBU (`obu_type = 2`, no payload)
//!   2. Sequence Header OBU (`obu_type = 1`, on keyframes)
//!   3. Frame OBU (`obu_type = 6`, uncompressed_header + tile_group_obu
//!      wrapping the tile payload)
//!
//! This module owns the bit-level writers (OBU header, LEB128 size, byte-
//! aligned bit stream) plus the higher-level syntax helpers used by the
//! frame assembler. Callers stay in `rust/src/codec/raw.rs`.
//!
//! Kept scope: this file implements the bit-writer + OBU framing primitives
//! with unit coverage. The full sequence and frame syntax writers live in
//! `synth.rs` — currently a skeleton that will grow to full parity as the
//! AV1 profile advertisement is unblocked. The advertisement in
//! `rust/src/config.rs` stays gated on both V4L2 OUTPUT-format enumeration
//! AND a passing native (or software) parity sample.

pub(crate) mod bitstream;
pub(crate) mod synth;

// Re-exports are pre-wired for the follow-up integration in `codec/raw.rs`
// that emits TD + Sequence Header + Frame OBU around each AV1 access unit.
// Keeping them here at introduction time means the AV1 lane can hook in
// without shuffling paths; #[allow] silences the interim "unused" warning.
#[allow(unused_imports)]
pub(crate) use bitstream::{BitWriter, ObuType, ObuWriter, leb128_size, write_leb128};
#[allow(unused_imports)]
pub(crate) use synth::{
    ColorDescription, SeqProfile, SequenceHeaderInput, synthesize_sequence_header,
};
