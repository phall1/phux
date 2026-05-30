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
//! `ratatui` is allowed *only* under this module. A CI grep guard
//! (`scripts/check-ratatui-boundary.sh`, wired into `just ci`) enforces the
//! invariant. See epic `phux-5ke` and (TBD) `ADR-0020`.

pub mod chrome;
pub mod overlay;
mod sgr;
