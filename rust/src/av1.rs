//! AV1 OBU assembly for the stateful V4L2 decoder.
//!
//! VA supplies picture parameters and tile payloads separately. This module
//! writes sequence and frame headers and wraps those payloads in OBUs.
//! Bit-level framing, sequence syntax, and frame syntax have separate owners.
//!
//! The VA adapter in `codec/raw/av1.rs` still lacks authoritative reference
//! refresh flags and some sequence fields. The path therefore requires an
//! explicit experimental opt-in until full-stream parity is established.

mod bitstream;
mod frame;
mod synth;
pub(crate) mod transport_prefix;

pub(crate) use bitstream::{BitWriter, ObuWriter};
pub(crate) use frame::{
    FrameHeaderInput, FrameType, synthesize_frame_obu, synthesize_uncompressed_header,
};
pub(crate) use synth::{SeqProfile, SequenceHeaderInput, synthesize_sequence_header};
