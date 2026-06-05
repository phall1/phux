//! Synthesize a `TERMINAL_SNAPSHOT` `vt_replay_bytes` blob from a
//! `libghostty_vt::Terminal`.
//!
//! See [`synthesizer::SnapshotSynthesizer`] for the main entry point.

pub mod reference;
pub mod synthesizer;

pub use synthesizer::{
    SnapshotBytes, SnapshotSynthesizer, SynthesisError, synthesize, SCROLLBACK_ALL,
    GRAPHEME_INLINE,
};
pub use reference::ConsumerReference;
