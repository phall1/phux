//! Capability advertisements (SPEC §6.2).
//!
//! Capabilities live in HELLO and apply for the life of the connection. The
//! types here are wire-level: they appear in `ClientCapabilities` /
//! `ServerCapabilities` envelopes and drive the server-side VT byte-stream
//! rewriter per [ADR-0013].
//!
//! Under ADR-0013 the cell-level [`Color`](libghostty_vt::style::StyleColor)
//! downsampling helper is gone; the server rewrites SGR sequences in the
//! outbound byte stream instead (see `phux_server::downsample`). What
//! survives on the protocol side is the *advertised tier itself* —
//! [`ColorSupport`] — which the rewriter consults to decide what to emit.
//!
//! [ADR-0013]: https://github.com/phall1/phux/blob/main/ADR/0013-libghostty-bytes-on-wire.md

/// A client's color tier (SPEC §6.2).
///
/// Advertised once at HELLO time; the server rewrites outbound VT bytes to
/// fit. `TrueColor` is the most-permissive tier — clients that have not yet
/// advertised caps default here so we never silently downgrade.
///
/// Variants are ordered from most-permissive to least-permissive, but the
/// enum is `#[non_exhaustive]`: protocol additions (e.g. a future palette
/// negotiation tier) must not break downstream consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ColorSupport {
    /// 24-bit direct RGB. The server forwards SGR truecolor sequences
    /// (`CSI 38;2;R;G;B m` / `CSI 48;2;R;G;B m`) verbatim.
    #[default]
    TrueColor,
    /// xterm 256-color palette: 16 system colors, a 6x6x6 RGB cube
    /// (indices 16..=231), and 24-step grayscale (232..=255).
    Indexed256,
    /// 16 system colors only (the ANSI base set + 8 bright variants).
    Indexed16,
}
