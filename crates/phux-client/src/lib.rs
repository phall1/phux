//! phux TUI client.
//!
//! Receives `TERMINAL_OUTPUT` byte frames from a phux server (see
//! `phux-protocol`), feeds them into a local `libghostty_vt::Terminal`
//! per attached pane, and renders dirty rows back out to the outer
//! terminal via `RenderState`. Knows nothing about PTYs.
//!
//! See ADR-0013 for the bytes-on-wire decision and
//! `research/2026-05-25-libghostty-renderstate.md` for the renderer-side
//! contract this crate implements.
//!
//! # Render layering (epic phux-5ke)
//!
//! Pane interiors are painted by libghostty (VT bytes → `Terminal` →
//! stdout). Chrome — status bar, pane dividers, borders, overlays — is
//! painted by `ratatui` from the [`render`] module. The two layers
//! composite over disjoint screen regions, never interleaved. `ratatui`
//! is allowed only under `render/`; a CI grep guard
//! (`scripts/check-ratatui-boundary.sh`, hooked into `just ci`) enforces
//! the boundary. See (TBD) `ADR-0020`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod attach;
pub mod layout;
pub mod predict;
pub mod render;
