//! In-tree status-bar widget implementations.
//!
//! Each submodule defines a `Widget`-implementing struct plus a
//! `pub(crate) fn factory` that the [`super::WidgetRegistry`] calls.

pub(super) mod session_name;
pub(super) mod time;
