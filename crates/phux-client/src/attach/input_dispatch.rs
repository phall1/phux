//! Input dispatcher: translates parser-emitted events into wire frames
//! or layout-action effects.
//!
//! Owns the resolver-intercept path (prefix chord → ResolvedAction →
//! mutate `LayoutState`), the predict overlay's keystroke feed, and the
//! parked-split bookkeeping (`PendingSplit`) that bridges a local
//! `split-pane` chord to its remote `SPAWN_TERMINAL` reply.
//!
//! This module's bodies will be populated by the `input_dispatch`
//! refactor agent.
