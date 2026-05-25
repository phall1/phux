//! Status-bar widget trait + registry + in-tree widget implementations.
//!
//! Filled in by `phux-nz4.4`. Note: [`crate::Widget`] is the *schema* enum
//! (a parsed `[[status.widgets]]` entry from TOML); the runtime `Widget`
//! trait will live here under a different name (e.g. `StatusWidget` or
//! `Renderable`) to avoid the collision.
