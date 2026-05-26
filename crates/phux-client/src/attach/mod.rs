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
//! * [`input`] — stdin bytes → structured input events plus the hardcoded
//!   `Ctrl-b d` detach chord.
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
//! * Configurable detach key — `Ctrl-b d` is hardcoded for v0; tracked
//!   under phux-631.
//! * Mouse / bracketed-paste parsing — keyboard input (ASCII, UTF-8,
//!   CSI / SS3 sequences, modifier-bearing chords, Alt-chords) is
//!   handled by [`input::StdinParser`]; mouse reports and bracketed
//!   paste are deferred follow-ups (see the input module docs).

pub mod connection;
pub mod driver;
pub mod input;
pub mod render;

pub use driver::{AttachError, run, run_with_stdout, write_terminal_reset};
pub use input::DETACH_CHORD_DESCRIPTION;
