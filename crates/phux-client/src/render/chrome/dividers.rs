//! Pane dividers and borders. Filled in by phux-5ke.3.
//!
//! Replaces `attach/multi_pane::paint_dividers` with a ratatui-based composer
//! that uses `Cell::skip` to carve out pane interior rects for libghostty.
