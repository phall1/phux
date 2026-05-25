//! Wire-level input event types.
//!
//! These mirror libghostty-vt's `key::Event`, `mouse::Event`, `focus::Event`,
//! and paste utilities one-to-one — see ADR-0006. Numeric discriminants are
//! chosen to match libghostty's enums verbatim so the server-side
//! `From<&phux_protocol::input::*>` conversions are field-for-field copies.
//!
//! Wire encoding for these types lives in [`crate::wire`].

pub mod focus;
pub mod key;
pub mod mouse;
pub mod paste;
