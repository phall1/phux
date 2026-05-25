//! Wire codec — length-prefixed TLV framing per `SPEC.md` Appendix A.
//!
//! All multi-byte integers are big-endian. Frames are length-prefixed.
//! Field IDs and message types match SPEC §7's catalog.

pub mod decode;
pub mod diff;
pub mod encode;
pub mod error;
pub mod field;
pub mod frame;
pub mod info;

pub use error::DecodeError;
