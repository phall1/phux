//! Server-side input plumbing: wire events → libghostty-vt events → PTY bytes.
//!
//! ADR-0006 makes this layer a one-to-one shape mirror: each
//! `phux_protocol::input::*Event` constructs the corresponding
//! `libghostty_vt::*::Event` via a field-for-field copy (numeric
//! discriminants already match), and a `PerPane*Encoder` wraps the
//! libghostty encoder + a reusable byte buffer so per-pane state stays
//! private to the server.
//!
//! See [`SPEC.md`] §9 and [`ADR/0006-input-mirrors-libghostty.md`].

pub mod focus;
pub mod key;
pub mod mouse;
pub mod paste;

pub use focus::PerPaneFocusEncoder;
pub use key::PerPaneKeyEncoder;
pub use mouse::PerPaneMouseEncoder;
pub use paste::{PasteOutcome, PerPanePasteEncoder};
