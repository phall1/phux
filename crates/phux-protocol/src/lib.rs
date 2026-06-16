//! Wire protocol for phux.
//!
//! This crate defines the protocol described in [`docs/spec/`] at the workspace
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
//! [`docs/spec/`]: https://github.com/phall1/phux/tree/main/docs/spec
//! [ADR-0008]: https://github.com/phall1/phux/blob/main/ADR/0008-use-libghostty-types-directly.md
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![cfg_attr(docsrs, feature(doc_cfg))]

// The wire codec and its input atoms are libghostty-free (ADR-0024) and so
// build for any target, including wasm browser consumers. libghostty
// conversions for the atoms live behind the `server` feature.
pub mod input;
pub mod wire;

pub mod caps;
pub mod ids;

#[cfg(feature = "server")]
pub mod policy;

#[cfg(feature = "server")]
pub mod sgr;

pub use caps::{
    ClientCapabilities, ColorSupport, ImageProtocol, ImageProtocolSet, KeyboardProtocol,
    KeyboardProtocolSet, Layer, LayerSet,
};
pub use ids::{ClientId, FrameId, GroupId, SatelliteHost, SessionId, TerminalId, WindowId};

/// Protocol version this crate implements.
///
/// Bumped from `0.1.0` to `0.2.0` in phux-vp0.4: [`TerminalId`] becomes a
/// tagged union (`Local` / `Satellite`) per ADR-0016, which prepends a
/// 1-byte tag to every `TerminalId` field on the wire.
///
/// Bumped from `0.2.0` to `0.3.0` by the "Option B" wire re-tier
/// (ADR-0019 / ADR-0027): the L2 collection lifecycle verbs
/// `CREATE_SESSION` / `KILL_COLLECTION` / `RENAME_SESSION` (command tags
/// `0x09`..=`0x0b`) are removed and replaced by a single atomic
/// multi-terminal op, `KILL_TERMINALS` (reusing tag `0x09`); grouping
/// (membership + names) moves to L3 metadata + client logic. Removing wire
/// verbs is wire-breaking, so pre-1.0 this bumps the minor.
///
/// Bumped from `0.3.0` to `0.4.0` by the field-tagged TLV wire migration:
/// every message body changes from positional, fixed-order fields to
/// field-tagged TLV (`field_id: varint || wire_type: u8 || length-delimited
/// value`) per `docs/spec/appendix-encoding.md`. Decoders now match top-level
/// fields by stable id (start at `1`, contiguous per message) and skip any id
/// they do not recognise by its declared length; optional / trailing fields
/// become simply-absent tagged fields. Nested tagged unions and sub-records
/// (`TerminalId`, `ViewportInfo`, `Command`, `SessionSnapshot`, ...) stay
/// positional inside a field's value. Every body's bytes change, so this is
/// wire-breaking; pre-1.0 it bumps the minor.
///
/// Bumped from `0.4.0` to `0.5.0` by phux-q1ni (ADR-0030): the `INPUT_SELECTION`
/// frame (type `0x15`), its `Selection` input-event tag (`0x04`), and the
/// `SelectionEvent` / `SelectionMode` wire types are removed. Selection is a
/// client-side projection over the consumer's own engine, never a wire tier —
/// the client extracts the selected text from its own libghostty `Terminal` and
/// copies it locally (OSC 52). Removing a wire frame is wire-breaking, so pre-1.0
/// this bumps the minor.
///
/// Bumped from `0.5.0` to `0.6.0` by phux-2sl6: a new `AgentEvent` variant,
/// `ASKED` (event tag `0x08`), carries an agent's pending human-answerable
/// question (id, text, suggested answers, optional elapsed seconds) on the
/// `EVENT` (`0xB3`) stream so a projection consumer can render the waiting
/// prompt without re-deriving it from the grid. The addition is forward-compat
/// — an older decoder skips the unknown event tag to `AgentEvent::Unknown` by
/// its length prefix — but new wire bytes a peer may emit means the minor bumps
/// pre-1.0.
pub const PROTOCOL_VERSION: Version = Version {
    major: 0,
    minor: 6,
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
