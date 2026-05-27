//! Chrome layer — status bar, dividers, pane borders.
//!
//! Split into submodules so wave-2 work can land in disjoint files:
//! - [`status_bar`] — bottom-row status widget (phux-5ke.2)
//! - [`dividers`] — pane separators and borders (phux-5ke.3)

pub mod dividers;
pub mod status_bar;
