//! phux TUI client substrate — the ratatui-free pane-interior layer.
//!
//! This crate holds the parts of the phux TUI client that paint and
//! reason about **pane interiors**, never chrome:
//!
//! - [`layout`] — the pane-geometry layout tree, split math, and the CBOR
//!   metadata envelope that persists it server-side.
//! - [`multi_pane`] — layout tree → per-pane rectangles + the divider
//!   cells between them (pure compute; the chrome layer rasterizes the
//!   `DividerCell`s to VT).
//! - [`predict`] — Mosh-class predictive local echo over the pane mirror.
//!
//! # Why a separate crate (ADR-0020)
//!
//! phux-client composites two renderers: `ratatui` chrome (status bar,
//! dividers, overlays) over libghostty pane interiors. The architectural
//! invariant is that pane-interior code — layout math, the pane mirror,
//! and predictive echo — stays `ratatui`-free so libghostty owns the hot
//! path unmodified. This crate carries **no `ratatui` dependency**, so the
//! boundary is enforced by the compiler: a `use ratatui` here fails to
//! build. The chrome and the attach loop live in `phux-client`, which
//! depends on this crate (never the reverse).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod layout;
pub mod multi_pane;
pub mod predict;
