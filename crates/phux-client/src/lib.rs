//! phux TUI client.
//!
//! Receives `PANE_OUTPUT` byte frames from a phux server (see
//! `phux-protocol`), feeds them into a local `libghostty_vt::Terminal`
//! per attached pane, and renders dirty rows back out to the outer
//! terminal via `RenderState`. Knows nothing about PTYs.
//!
//! See ADR-0013 for the bytes-on-wire decision and
//! `research/2026-05-25-libghostty-renderstate.md` for the renderer-side
//! contract this crate implements.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod attach;
