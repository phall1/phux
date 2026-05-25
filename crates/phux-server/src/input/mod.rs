//! Server-side input plumbing: wire events → libghostty-vt events → PTY bytes.
//!
//! Per ADR-0008, the wire's input atoms (`KeyAction`, `PhysicalKey`,
//! `ModSet`, `MouseAction`, `MouseButton`, `FocusEvent`) ARE libghostty's
//! types. The only work this layer does is:
//!
//! 1. Compose libghostty's allocator-bound `Event` types from the wire's
//!    plain field shapes (`KeyEvent`, `MouseEvent`).
//! 2. Gate emission on the pane's terminal state (focus mode 1004,
//!    bracketed-paste mode 2004).
//! 3. Apply per-pane policy for untrusted paste payloads.
//! 4. Own a `PerPane*Encoder` so encoder state stays private to a single
//!    pane (ADR-0006 §"Encoder options stay server-local").
//!
//! See SPEC.md §9 and ADR-0006 + ADR-0008.

pub mod focus;
pub mod key;
pub mod mouse;
pub mod paste;

pub use focus::PerPaneFocusEncoder;
pub use key::PerPaneKeyEncoder;
pub use mouse::PerPaneMouseEncoder;
pub use paste::{PasteOutcome, PerPanePasteEncoder};
