//! Attach loop — the runtime that makes `phux attach <session>` work.
//!
//! Wires together four collaborators per the phux-9gw.3 design:
//!
//! * [`connection`] — UDS transport plus length-prefixed frame I/O.
//! * [`driver`] — the `tokio::select!` lifecycle, the file that owns the
//!   process's stdout, stdin, and SIGWINCH handles for the duration of the
//!   attach.
//! * [`render`] — VT emission from a local `libghostty_vt::Terminal` /
//!   `RenderState` pair per ADR-0013.
//! * [`input`] — stdin bytes → structured input events for the keybinding
//!   resolver and pane input forwarding.
//!
//! The public entry point is [`run`]. It expects to be called from a tokio
//! current-thread runtime (matching ADR-0003); embedders are responsible for
//! the runtime lifecycle. The function takes over the controlling terminal
//! (raw mode + alt screen) and restores it on every exit path including
//! panic — see [`driver::RawModeGuard`].
//!
//! # Scope
//!
//! This module deliberately does **not** implement:
//!
//! * Predictive local echo — that's phux-9gw.1 layered on top.
//! * `VIEWPORT_RESIZE` — the wire frame doesn't exist yet; tracked under
//!   phux-4hp.
//! * Mouse / bracketed-paste parsing — keyboard input (ASCII, UTF-8,
//!   CSI / SS3 sequences, modifier-bearing chords, Alt-chords) is
//!   handled by [`input::StdinParser`]; mouse reports and bracketed
//!   paste are deferred follow-ups (see the input module docs).

pub mod actions;
pub mod connection;
pub mod driver;
pub mod input;
pub mod input_dispatch;
pub mod multi_pane;
pub mod paint;
pub mod reflow;
pub mod render;
pub mod server_frame;

pub use driver::{AttachError, run, run_with_predict, run_with_stdout, write_terminal_reset};
// Status bar lives under `crate::render::chrome::status_bar` post
// phux-5ke.2 (ADR-0020). Re-exported here so external callers (the
// `phux-client::attach::status_bar::*` integration test path included)
// keep working without changing their imports.
pub use crate::render::chrome::status_bar;
pub use crate::render::chrome::status_bar::{Position, StatusBarPainter, make_context};
