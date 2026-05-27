//! Server-to-client frame handling: dispatches `FrameKind` variants to
//! the right state mutations and rendering.
//!
//! Returns a [`FrameOutcome`] describing the follow-up the async driver
//! should take (e.g. exit on `DETACHED`, send `GET_METADATA` after
//! `ATTACHED`, repaint after a layout-replacing frame).
//!
//! This module's bodies will be populated by the `server_frame`
//! refactor agent.
