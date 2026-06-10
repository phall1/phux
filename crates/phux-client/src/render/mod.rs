//! Layered render: ratatui-driven chrome composited over libghostty pane
//! interiors.
//!
//! phux-client uses two renderers with disjoint screen regions:
//!
//! - **Chrome** (this module, [`chrome`]) — ratatui paints the status bar,
//!   pane dividers, borders, and overlays. Layout math, widget composition,
//!   and modal stacking live here.
//! - **Pane interior** (outside this module) — libghostty drives VT bytes
//!   straight to stdout, preserving kitty graphics, sixel, OSC 8 hyperlinks,
//!   and the Kitty key protocol on the hot path. See `attach::render`.
//!
//! The two layers are composited, not interleaved: chrome carves skip-cell
//! rectangles for pane rects so libghostty owns those cells exclusively;
//! cursor and SGR state are explicitly handed off at the boundary.
//!
//! `ratatui` is confined to this crate (`phux-client`); the pane-interior
//! substrate lives in `phux-client-core`, which has no `ratatui`
//! dependency, so the boundary is compiler-enforced rather than grep-checked
//! (ADR-0020 replaced `scripts/check-ratatui-boundary.sh` with the crate
//! split in phux-0fv). See epic `phux-5ke` and `ADR-0020`.

pub mod chrome;
pub mod overlay;
mod sgr;
pub mod theme;

/// Color-preserving SGR emitter for chrome painted outside the ratatui-buffer
/// path (the driver's copy-mode status strip).
pub use sgr::write_sgr_color;
pub use theme::Theme;
