//! Paint composition for the attach driver.
//!
//! Two paint paths:
//! * [`paint_full_frame`] — clear viewport, render every pane, dividers,
//!   status bar. Use after layout mutations, viewport resize, or attach.
//! * [`paint_focused_pane`] + [`paint_bar_after_pane`] — incremental
//!   path for `TERMINAL_OUTPUT` arrivals where only the focused pane
//!   changed.
//!
//! [`pane_viewport`] reserves the bottom row for the status bar so pane
//! Rects never spill into it.
//!
//! This module's bodies will be populated by the `paint` refactor agent.
