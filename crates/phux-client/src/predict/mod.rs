//! Predictive local echo — Mosh-class latency hiding for `phux attach`.
//!
//! When the user types over a slow link, naive client/server VT round-trips
//! make every keystroke wait for the server to echo it. Mosh's State
//! Synchronization Protocol paper popularised speculative local rendering:
//! the client applies a *guess* of what the server will do to its own
//! mirror, decorates the guess so the user can tell it apart from
//! authoritative output, and reconciles when the real answer arrives.
//!
//! `phux-9gw.1` lands the third leg of Mosh's value decomposition (per the
//! `mosh-decomposition` bd memory): (1) per-consumer state sync —
//! ADR-0018, server-side, already landed; (2) UDP transport — deferred to
//! the QUIC migration in ADR-0007; (3) **predictive local echo** — this
//! module.
//!
//! # Surface
//!
//! Three small types compose the feature:
//!
//! - [`state::PredictionState`] — the queue of in-flight predictions plus
//!   the client-side cursor estimate used to anchor them.
//! - [`overlay::Overlay`] — writes the prediction layer to the outer
//!   terminal as VT escapes (positioned writes with an
//!   underline SGR attribute).
//! - [`reconcile::reconcile_terminal_output`] — drops predictions once a
//!   `TerminalOutput` frame has arrived. Cumulative semantics match
//!   `FRAME_ACK` (SPEC §12.2): one server output drains *all* predictions
//!   issued before it.
//!
//! # Visual decoration: underline
//!
//! Mosh ships with underline as the default decoration and the choice
//! survives a decade of field use. We follow suit for two reasons specific
//! to phux:
//!
//! 1. The renderer in [`crate::attach::render`] already emits
//!    `Style::faint` (SGR 2) for any program that asks for dim text —
//!    using dim for predictions would collide with `man`, `less`, vim
//!    "concealed" regions, and any TUI that paints a dimmed status line.
//!    Underline (SGR 4) is rare in interactive content. Mosh made the
//!    same call for the same reason.
//! 2. Underline survives the "no SGR" path in our `emit_sgr_delta` —
//!    re-emitting cells doesn't accidentally lose the prediction bit
//!    because we paint the overlay *after* the renderer flushes, so the
//!    next renderer pass cleanly stomps it on reconciliation.
//!
//! # Safety classes (v0)
//!
//! Only two key classes are predicted today:
//!
//! - Printable ASCII (`0x20..=0x7E`). The server's terminal will echo
//!   exactly one cell of advance for each — the prediction is a
//!   one-character forward step at the cursor.
//! - Backspace (`PhysicalKey::Backspace`) **at end-of-line**, defined as
//!   "the cell to the left of the cursor is non-empty and the cursor is
//!   not at column 0". A naïve backspace prediction over a wrapped line,
//!   the prompt, or after a programmatic SGR change would diverge
//!   visibly. End-of-line is the conservative subset that covers the
//!   "typing then immediately deleting" case which is the bulk of why
//!   users notice latency.
//!
//! Everything else — arrow keys, control chords, function keys, Tab,
//! Enter, Alt-chords, IME composition — is not predicted. They are still
//! sent upstream as normal; only the local echo is skipped. A future
//! ticket can widen the safe set (Enter at EOL → newline + carriage-
//! return + cursor-to-col-0; cursor-motion arrows over a known line;
//! full-line backspace given a known prompt) once we have a real
//! reconciliation strategy for divergence.
//!
//! # Off by default
//!
//! Predictive echo is gated behind [`PredictiveConfig::enabled`], wired
//! through [`crate::attach::run_with_predict`]. The default is `false`
//! until the feature has miles on it. Wiring the TOML `[experimental]
//! predictive-echo = true` knob into `phux-config` is deferred to a
//! follow-up — the Rust-level toggle is what the test plan exercises.
//!
//! # Reconciliation policy
//!
//! `reconcile_terminal_output` drops the entire pending queue when *any*
//! `TerminalOutput` arrives for the active terminal. Three reasons:
//!
//! 1. The renderer is about to overwrite the affected rows wholesale —
//!    `TerminalRenderer::render` does dirty-row redraws and our overlay
//!    sits on top of stdout, not inside the libghostty Terminal. The
//!    next paint correctly shows server truth.
//! 2. Cumulative ack semantics — `FRAME_ACK seq = N` says "I have
//!    applied everything up to N". When the server sends N, by that
//!    semantics it has also "covered" all earlier predictions.
//! 3. Simpler invariant: zero predictions outstanding after every server
//!    frame. A per-character match game would correctly preserve
//!    *some* predictions (the ones still ahead of what the server has
//!    confirmed), but the bookkeeping is fragile under SGR changes,
//!    scrollback, and resize. We trade a tiny bit of flicker (the
//!    underline disappears) for a state machine we can reason about.

mod overlay;
mod reconcile;
mod state;

pub use overlay::Overlay;
pub use reconcile::reconcile_terminal_output;
pub use state::{Prediction, PredictionOutcome, PredictionState, PredictiveConfig};
