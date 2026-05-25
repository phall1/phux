//! Wire decode errors.
//!
//! Owned by phux-6yl.4. See `SPEC.md` §5 and Appendix A.

use thiserror::Error;

/// Errors that can occur while decoding a wire frame.
///
/// Every variant corresponds to a malformed-input condition that the decoder
/// MUST surface without panicking. See `SPEC.md` §5 (framing) and Appendix A
/// (encoding primitives).
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum DecodeError {
    /// The input buffer was exhausted before the decoder finished reading the
    /// expected number of bytes for a primitive, field, or frame.
    #[error("unexpected end of input")]
    UnexpectedEof,

    /// A length-prefixed string field did not contain valid UTF-8.
    #[error("invalid UTF-8 in string field")]
    InvalidUtf8,

    /// The frame's type byte did not match any known `FrameKind` discriminant.
    #[error("unknown frame kind: 0x{tag:04x}")]
    UnknownFrameKind {
        /// The unrecognised discriminant. Stored as `u16` so future minor
        /// versions can extend the space without changing the error shape.
        tag: u16,
    },

    /// A declared length prefix would, if accepted, exceed the remaining
    /// buffer or the protocol's hard frame-size cap of 16 MiB (`SPEC.md` §5).
    #[error("declared length exceeds buffer or protocol cap")]
    LengthOverflow,

    /// A `SessionId` carried the `SATELLITE` tag from ADR-0007 §3. v0.1
    /// decoders accept only `LOCAL`; satellite routing arrives in v0.2+.
    #[error("satellite-routed session ids are not supported in this protocol version")]
    UnsupportedSatelliteRoute,

    /// An enumerated field on the wire carried a value the decoder does not
    /// recognise. Used for libghostty atoms (`Key`, `KeyAction`, `MouseAction`,
    /// `MouseButton`) where minor protocol versions MAY add values; v0.1
    /// rejects unknown discriminants so that misinterpretation can't silently
    /// corrupt downstream encoding.
    #[error("unknown enum discriminant in field '{field}': {value}")]
    UnknownEnumValue {
        /// Logical name of the field that carried the bad value, for diagnostics.
        field: &'static str,
        /// The unrecognised discriminant, widened to `u32` to cover every
        /// enumerated type the codec emits.
        value: u32,
    },
}
