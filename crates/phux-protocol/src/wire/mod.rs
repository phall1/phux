//! Wire codec — length-prefixed TLV framing per `docs/spec/appendix-encoding.md`.
//!
//! All multi-byte integers are big-endian. Frames are length-prefixed.
//! Field IDs and message types match SPEC §7's catalog.
//!
//! Under [ADR-0013] terminal content rides as raw VT bytes on the wire (the
//! hot path `TERMINAL_OUTPUT` frame and the attach-time `TERMINAL_SNAPSHOT`
//! frame). There is no structured cell-level diff codec in this crate.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

pub mod decode;
pub mod encode;
pub mod error;
pub mod field;
pub mod frame;
pub mod info;
pub mod l2_agent;

pub use error::DecodeError;
pub use l2_agent::{
    TYPE_GET_TERMINAL_STATE, TYPE_L2_EVENT, TYPE_L2_RESPONSE, TYPE_SUBSCRIBE_TERMINAL_EVENTS,
};
