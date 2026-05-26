//! Wire protocol for phux.
//!
//! This crate defines the protocol described in [`SPEC.md`] at the workspace
//! root: framing, message catalog, version negotiation, and the VT-bytes-on-
//! wire terminal content shape (per [ADR-0013]).
//!
//! The protocol is the source of truth. Code in this crate is normative;
//! implementations elsewhere defer to it.
//!
//! # Crate features
//!
//! - **`server`** (off by default): enables the full type surface —
//!   the [`input`] and [`wire`] modules, plus all re-exports of
//!   `libghostty-vt` input atoms (per [ADR-0008]). Every in-workspace
//!   consumer enables this feature. Without `server` this crate is a
//!   near-empty shell exposing only [`ids`], [`caps`], and [`Version`];
//!   that subset exists so the crate can be published to crates.io
//!   (where git-only deps like `libghostty-vt` are disallowed) and
//!   rendered on docs.rs.
//!
//! [`SPEC.md`]: https://github.com/phall1/phux/blob/main/SPEC.md
//! [ADR-0008]: https://github.com/phall1/phux/blob/main/ADR/0008-use-libghostty-types-directly.md
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod input;
#[cfg(feature = "server")]
#[cfg_attr(docsrs, doc(cfg(feature = "server")))]
pub mod wire;

pub mod caps;
pub mod ids;

pub use caps::ColorSupport;
pub use ids::{ClientId, FrameId, SessionId, TerminalId, WindowId};

/// Protocol version this crate implements.
pub const PROTOCOL_VERSION: Version = Version {
    major: 0,
    minor: 1,
    patch: 0,
};

/// A semantic protocol version: `major.minor.patch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Version {
    /// Wire-breaking changes bump this.
    pub major: u16,
    /// Additive changes bump this.
    pub minor: u16,
    /// Editorial; behavior unchanged.
    pub patch: u16,
}
