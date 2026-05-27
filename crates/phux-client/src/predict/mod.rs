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
//! - [`reconcile::reconcile_terminal_output_per_cell`] — the v1.1
//!   match game (phux-9gw.1.1). On each `TerminalOutput`, walks the
//!   prediction queue against the freshly painted authoritative cells
//!   and the new cursor position; drops confirmed predictions, drops
//!   the suffix from any contradiction, and keeps predictions still
//!   ahead of confirmed state. The older wholesale-drain
//!   [`reconcile::reconcile_terminal_output`] is retained for
//!   `TERMINAL_SNAPSHOT` replays where the entire viewport is stomped.
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
//! # Safety classes (v1.1)
//!
//! Three key classes are predicted today:
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
//! - Enter (`PhysicalKey::Enter`) **past column 0**, on any row except
//!   the last. Models a pure cursor jump to `(row+1, 0)` — no cell
//!   paint, just a forward anchor so subsequent inserts queue on the
//!   correct row. The per-cell reconcile confirms the prediction once
//!   the authoritative cursor advances past the original row;
//!   contradicts (drop) if the server stayed put (program intercepted
//!   the keystroke, e.g. password prompt swallow).
//!
//! Everything else — arrow keys, control chords, function keys, Tab,
//! Alt-chords, IME composition, UTF-8 multi-byte, full-line backspace
//! from a known prompt — is not predicted. They are still sent upstream
//! as normal; only the local echo is skipped. Follow-up tickets
//! (phux-9gw.1.1 deferrals) widen the safe set further once the
//! reconcile path has miles on it.
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
//! `reconcile_terminal_output_per_cell` is the production path for
//! `TERMINAL_OUTPUT`. It reads each prediction's target cell from the
//! freshly rendered authoritative grid and classifies it as
//! **confirmed** (drop, the server already painted it), **pending**
//! (keep, server hasn't caught up — overlay stays alive), or
//! **contradicted** (drop the prediction *and* every prediction behind
//! it — the server diverged so the suffix is suspect). See the
//! `reconcile` module for the per-`PredictionKind` truth table.
//!
//! The wholesale-drain `reconcile_terminal_output` is retained for
//! `TERMINAL_SNAPSHOT` replays, where the entire viewport is stomped
//! and per-cell match would be redundant.

mod overlay;
mod reconcile;
mod state;

pub use overlay::Overlay;
pub use reconcile::{
    ReconcileStats, reconcile_terminal_output, reconcile_terminal_output_per_cell,
};
pub use state::{Prediction, PredictionKind, PredictionOutcome, PredictionState, PredictiveConfig};
