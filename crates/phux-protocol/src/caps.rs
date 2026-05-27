//! Capability advertisements (SPEC §6.2).
//!
//! Capabilities live in HELLO and apply for the life of the connection. The
//! types here are wire-level: they appear in [`ClientCapabilities`] /
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
    /// Monochrome — the renderer cannot distinguish color at all. SGR color
    /// sequences MUST be stripped from the outbound byte stream.
    ///
    /// Currently unused by [`detect_color_support`] (which never returns
    /// `Mono`); reserved for future explicit opt-in via configuration or
    /// for accessibility profiles. Added here so the wire codec has a
    /// stable tag for it.
    Mono,
}

impl ColorSupport {
    /// Wire tag for the [`ColorSupport`] variant.
    ///
    /// Discriminants are stable within the v0.x protocol; new variants
    /// append. Decoders that see an unknown tag MUST fall back to
    /// [`ColorSupport::TrueColor`] (the safe most-permissive default)
    /// rather than reject the frame — `#[non_exhaustive]` is the
    /// load-bearing contract.
    #[must_use]
    pub const fn as_wire(self) -> u8 {
        match self {
            Self::TrueColor => 0,
            Self::Indexed256 => 1,
            Self::Indexed16 => 2,
            Self::Mono => 3,
        }
    }

    /// Inverse of [`Self::as_wire`]. Unknown tags map to `None`; the
    /// decoder applies a default at the call site (typically
    /// [`ColorSupport::TrueColor`]) so a forward-compat HELLO from a
    /// future client never fails to decode.
    #[must_use]
    pub const fn from_wire(tag: u8) -> Option<Self> {
        Some(match tag {
            0 => Self::TrueColor,
            1 => Self::Indexed256,
            2 => Self::Indexed16,
            3 => Self::Mono,
            _ => return None,
        })
    }
}

// -----------------------------------------------------------------------------
// Layer / LayerSet — SPEC §6.2 conformance-tier bitset (ADR-0015).
// -----------------------------------------------------------------------------

/// A single conformance tier from SPEC §6.2 / §16.
///
/// L1 (Terminal substrate) is always implied and always implemented; L2
/// (Collection lifecycle) and L3 (Metadata storage) are optional services
/// negotiated via [`LayerSet`] in HELLO / `HELLO_OK`.
///
/// Per ADR-0015 the **negotiated tier set** is the intersection of the
/// client's and server's advertised layers. Out-of-tier messages MUST
/// surface as protocol errors (SPEC §16.4).
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Layer {
    /// Terminal substrate. Always implemented; always implied.
    L1 = 0x01,
    /// Collection lifecycle (OPTIONAL). SPEC §7.3 / §11.L2.
    L2 = 0x02,
    /// Metadata storage (OPTIONAL). SPEC §7.4 / §11.L3.
    L3 = 0x04,
}

/// A bit-field of [`Layer`]s. Wire encoding: a single `u8` carrying the
/// OR of the variants' raw discriminants.
///
/// Construction goes through [`Self::new`] / [`Self::with`] / [`Self::insert`]
/// so the L1-always-on invariant is preserved. Direct field-literal
/// construction is intentionally NOT supported — `Layer` may grow with
/// future tiers and the bitset must remain forward-compat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct LayerSet(u8);

impl LayerSet {
    /// The L1-only set. Equivalent to `LayerSet::default()`.
    ///
    /// L1 is always implied per SPEC §6.2; the bit is always present in
    /// the wire encoding regardless of construction path.
    #[must_use]
    pub const fn new() -> Self {
        Self(Layer::L1 as u8)
    }

    /// Build a set containing all listed layers (plus the always-on L1).
    #[must_use]
    pub const fn with(layers: &[Layer]) -> Self {
        let mut bits = Layer::L1 as u8;
        let mut i = 0;
        while i < layers.len() {
            bits |= layers[i] as u8;
            i += 1;
        }
        Self(bits)
    }

    /// The full set: L1 + L2 + L3. Used by the reference TUI which
    /// advertises every tier it speaks (SPEC §16.3).
    #[must_use]
    pub const fn all() -> Self {
        Self((Layer::L1 as u8) | (Layer::L2 as u8) | (Layer::L3 as u8))
    }

    /// Insert `layer` into the set. L1 cannot be removed.
    pub const fn insert(&mut self, layer: Layer) {
        self.0 |= layer as u8;
    }

    /// Test whether `layer` is in the set.
    #[must_use]
    pub const fn contains(self, layer: Layer) -> bool {
        self.0 & (layer as u8) != 0
    }

    /// Raw wire byte. The encoder writes this directly; the decoder
    /// passes the byte to [`Self::from_wire`]. L1 is always forced on
    /// so peers can rely on the invariant.
    #[must_use]
    pub const fn as_wire(self) -> u8 {
        self.0 | (Layer::L1 as u8)
    }

    /// Inverse of [`Self::as_wire`]. Unknown bits beyond L1/L2/L3 are
    /// silently dropped (forward-compat per Appendix A) but L1 is
    /// always forced on.
    #[must_use]
    pub const fn from_wire(byte: u8) -> Self {
        let known = (Layer::L1 as u8) | (Layer::L2 as u8) | (Layer::L3 as u8);
        Self((byte & known) | (Layer::L1 as u8))
    }
}

impl Default for LayerSet {
    fn default() -> Self {
        Self::new()
    }
}

/// The client's advertised capability set, per SPEC §6.2.
///
/// SPEC §6.2 enumerates `kbd_protocols`, `mouse_protocols`, `color`,
/// `images`, `hyperlinks`, `unicode_version`, the deprecated `rendering`
/// mode, and the `layers` bitset. As of phux-4li.2 [`Self::color_support`]
/// and [`Self::layers`] are populated; sibling tickets add the remaining
/// fields behind their own wire bumps. The struct is `#[non_exhaustive]`
/// so additive fields don't break downstream literal construction.
///
/// Construct via [`Self::new`] (defaults across the board) plus the
/// builder setters; that's the path that survives field-set growth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub struct ClientCapabilities {
    /// The client's color tier (SPEC §6.2). See [`ColorSupport`].
    pub color_support: ColorSupport,
    /// The set of conformance tiers (SPEC §6.2 / §16) the client speaks.
    /// L1 is always implied; clients add L2 / L3 to opt in to the
    /// respective optional services. The reference TUI advertises
    /// [`LayerSet::all`]; an agent / recorder advertises [`LayerSet::new`]
    /// (L1-only).
    pub layers: LayerSet,
}

impl ClientCapabilities {
    /// Build a default capability set: `ColorSupport::TrueColor` plus the
    /// L1-only layer set. Call sites that want to override one field call
    /// the matching `.with_*` setter.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            color_support: ColorSupport::TrueColor,
            layers: LayerSet::new(),
        }
    }

    /// Builder setter for [`Self::color_support`].
    #[must_use]
    pub const fn with_color_support(mut self, color_support: ColorSupport) -> Self {
        self.color_support = color_support;
        self
    }

    /// Builder setter for [`Self::layers`].
    #[must_use]
    pub const fn with_layers(mut self, layers: LayerSet) -> Self {
        self.layers = layers;
        self
    }
}

/// Detect the client terminal's color tier from environment hints.
///
/// The heuristic mirrors what well-known TUIs (tmux, neovim, htop) use:
///
/// 1. **`$COLORTERM`** is the canonical signal — values `truecolor` and
///    `24bit` mean direct RGB is safe.
/// 2. **`$TERM`** suffixes (`*-256color`, `*-direct`, `*-truecolor`) carry
///    the next-most-reliable signal.
/// 3. **`$TERM_PROGRAM`** covers macOS Terminal.app / iTerm.app where
///    `$COLORTERM` is often unset.
/// 4. Fallback: [`ColorSupport::TrueColor`] (most-permissive). The server
///    downsamples on the way out; an over-claim is recoverable. An
///    under-claim would silently degrade output even on capable terminals,
///    so we err generous.
///
/// This intentionally never returns [`ColorSupport::Mono`] — that tier is
/// reserved for explicit opt-in (config flag, accessibility profile) and
/// is not a signal any environment variable carries reliably.
#[must_use]
pub fn detect_color_support() -> ColorSupport {
    detect_from_env(|key| std::env::var(key).ok())
}

/// Pure (testable) form of [`detect_color_support`]: takes a lookup
/// closure so tests can simulate arbitrary environments without
/// `unsafe { std::env::set_var }`.
fn detect_from_env<F>(env: F) -> ColorSupport
where
    F: Fn(&str) -> Option<String>,
{
    // 1. $COLORTERM — the most authoritative signal.
    if let Some(ct) = env("COLORTERM") {
        let ct_lc = ct.to_ascii_lowercase();
        if ct_lc == "truecolor" || ct_lc == "24bit" {
            return ColorSupport::TrueColor;
        }
    }

    // 2. $TERM suffix.
    let term = env("TERM").unwrap_or_default();
    let term_lc = term.to_ascii_lowercase();
    if term_lc.ends_with("-direct") || term_lc.ends_with("-truecolor") {
        return ColorSupport::TrueColor;
    }
    if term_lc.ends_with("-256color") {
        return ColorSupport::Indexed256;
    }
    if !term_lc.is_empty() && !term_lc.contains("color") {
        // `xterm`, `linux`, `vt100`, etc. — assume 16-color baseline.
        // Anything richer would have advertised a `-256color` or
        // `-direct` suffix.
        // Common exception: macOS Terminal.app sets `TERM=xterm-256color`
        // so this branch only catches the genuine vt100/linux/etc cases.
        if term_lc == "dumb" {
            return ColorSupport::Mono;
        }
        return ColorSupport::Indexed16;
    }

    // 3. $TERM_PROGRAM — macOS native terminals.
    if let Some(tp) = env("TERM_PROGRAM") {
        let tp_lc = tp.to_ascii_lowercase();
        // iTerm.app and WezTerm advertise truecolor; Apple_Terminal
        // (macOS Terminal.app) is 256-color only.
        if tp_lc == "iterm.app" || tp_lc == "wezterm" {
            return ColorSupport::TrueColor;
        }
        if tp_lc == "apple_terminal" {
            return ColorSupport::Indexed256;
        }
    }

    // 4. Fallback: assume the user is on a modern truecolor terminal that
    // forgot to advertise. Over-claiming is recoverable (server downsamples
    // anyway if a later signal arrives); under-claiming silently degrades.
    ColorSupport::TrueColor
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn color_support_wire_roundtrips_every_variant() {
        for v in [
            ColorSupport::TrueColor,
            ColorSupport::Indexed256,
            ColorSupport::Indexed16,
            ColorSupport::Mono,
        ] {
            let tag = v.as_wire();
            let back = ColorSupport::from_wire(tag).expect("known tag");
            assert_eq!(back, v);
        }
    }

    #[test]
    fn unknown_color_support_tag_is_none() {
        assert!(ColorSupport::from_wire(0xFF).is_none());
    }

    #[test]
    fn colorterm_truecolor_wins() {
        let env = env_map(&[("COLORTERM", "truecolor"), ("TERM", "xterm-256color")]);
        assert_eq!(detect_from_env(env), ColorSupport::TrueColor);
    }

    #[test]
    fn colorterm_24bit_wins() {
        let env = env_map(&[("COLORTERM", "24bit"), ("TERM", "xterm")]);
        assert_eq!(detect_from_env(env), ColorSupport::TrueColor);
    }

    #[test]
    fn term_256color_maps_to_indexed256() {
        let env = env_map(&[("TERM", "xterm-256color")]);
        assert_eq!(detect_from_env(env), ColorSupport::Indexed256);
    }

    #[test]
    fn term_direct_maps_to_truecolor() {
        let env = env_map(&[("TERM", "xterm-direct")]);
        assert_eq!(detect_from_env(env), ColorSupport::TrueColor);
    }

    #[test]
    fn term_xterm_maps_to_indexed16() {
        let env = env_map(&[("TERM", "xterm")]);
        assert_eq!(detect_from_env(env), ColorSupport::Indexed16);
    }

    #[test]
    fn term_dumb_maps_to_mono() {
        let env = env_map(&[("TERM", "dumb")]);
        assert_eq!(detect_from_env(env), ColorSupport::Mono);
    }

    #[test]
    fn macos_terminal_falls_back_to_indexed256() {
        let env = env_map(&[("TERM_PROGRAM", "Apple_Terminal")]);
        assert_eq!(detect_from_env(env), ColorSupport::Indexed256);
    }

    #[test]
    fn iterm_advertises_truecolor() {
        let env = env_map(&[("TERM_PROGRAM", "iTerm.app")]);
        assert_eq!(detect_from_env(env), ColorSupport::TrueColor);
    }

    #[test]
    fn unknown_env_falls_back_to_truecolor() {
        let env = env_map(&[]);
        assert_eq!(detect_from_env(env), ColorSupport::TrueColor);
    }

    #[test]
    fn client_capabilities_default_is_truecolor() {
        let caps = ClientCapabilities::default();
        assert_eq!(caps.color_support, ColorSupport::TrueColor);
    }

    #[test]
    fn client_capabilities_builder() {
        let caps = ClientCapabilities::new().with_color_support(ColorSupport::Indexed16);
        assert_eq!(caps.color_support, ColorSupport::Indexed16);
    }
}
