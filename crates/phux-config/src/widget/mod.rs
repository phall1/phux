//! Status-bar widget trait + registry + in-tree widget implementations.
//!
//! Owned by `phux-nz4.4`. Note that [`crate::Widget`] is the *schema* enum
//! (a parsed `[[status.widgets]]` entry from TOML); the runtime trait here
//! is called [`StatusWidget`] to avoid the name collision.
//!
//! ## Why a local `Cell` here
//!
//! The status bar is a *frontend* concern: phux-config composes cells, the
//! TUI client lays them out, the wire never sees them. Under ADR-0013 the
//! protocol crate no longer exposes a `Cell` type (pane content moved to
//! VT bytes on the wire); the status bar accordingly carries its own
//! lightweight cell shape — just a grapheme cluster — and renders it via
//! the same SGR emission code path the renderer uses for live panes. If a
//! richer widget style surface ever lands (foreground color, bold flag,
//! etc.), grow this struct here — the wire doesn't care.

use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, SystemTime};

use smallvec::SmallVec;

use crate::schema::WidgetSpec;

mod status_bar;
mod widgets;

pub use status_bar::{StatusBar, row_to_string};
pub use widgets::session_name::SessionNameWidget;
pub use widgets::time::TimeWidget;

/// A single status-bar cell.
///
/// Local to phux-config — see the module-level doc for rationale. Grapheme
/// storage matches the inline-then-spill pattern phux-protocol once used
/// for its (now-deleted) `Cell::text` field.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    /// Grapheme cluster occupying this cell. May be empty for a blank cell.
    /// First element is the base codepoint; remaining elements are combining
    /// codepoints in source order.
    pub text: SmallVec<[char; 2]>,
}

/// Context passed to a [`StatusWidget`] at render time.
///
/// Kept intentionally narrow for nz4.4: clock time + session name. Future
/// waves may add active-pane id, cwd, etc.
#[derive(Debug, Clone, Copy)]
pub struct WidgetContext<'a> {
    /// Wall-clock time the status bar is rendering at. Passed in (rather
    /// than read inside the widget) so render is a pure function of
    /// context — that's what makes deterministic snapshot tests possible.
    pub now: SystemTime,
    /// Current session name (`""` if not in a session).
    pub session_name: &'a str,
}

/// A horizontal strip of cells produced by a widget for one render pass.
///
/// `Cell` is the local widget cell type — see the module-level doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetCells {
    /// Cells in left-to-right display order.
    pub cells: Vec<Cell>,
}

impl WidgetCells {
    /// Build a [`WidgetCells`] from a plain string. Each `char` becomes a
    /// single-cell grapheme; styling is left at default. This is the
    /// simplest possible cell-builder and is what both built-in widgets
    /// use today.
    #[must_use]
    pub fn from_text(s: &str) -> Self {
        let cells = s
            .chars()
            .map(|c| Cell {
                text: smallvec::smallvec![c],
            })
            .collect();
        Self { cells }
    }

    /// True if this strip carries no cells.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Number of cells in the strip.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.cells.len()
    }
}

/// A status-bar widget.
///
/// The trait is **not** named `Widget` because [`crate::schema::Widget`]
/// already occupies that name on the parsed-TOML side.
pub trait StatusWidget: Send + Sync + fmt::Debug + 'static {
    /// Render the widget for the current [`WidgetContext`]. Returns a
    /// horizontal cell strip.
    fn render(&self, ctx: &WidgetContext<'_>) -> WidgetCells;

    /// Optional poll interval. `None` ⇒ this widget needs no time-based
    /// repaint and is redrawn only when the status bar repaints for
    /// other reasons (session-name change, layout change, …). `Some(d)`
    /// ⇒ the status bar schedules a repaint at this cadence.
    fn poll_interval(&self) -> Option<Duration> {
        None
    }
}

/// Factory function: builds a widget from a TOML `opts` map.
pub type WidgetFactory =
    fn(&BTreeMap<String, toml::Value>) -> Result<Box<dyn StatusWidget>, WidgetError>;

/// Registry of widget kinds → factories.
///
/// [`WidgetRegistry::with_builtins`] pre-populates `time` and `session-name`;
/// callers may register additional kinds via [`Self::register`] before
/// the first [`Self::build`] call.
pub struct WidgetRegistry {
    factories: BTreeMap<&'static str, WidgetFactory>,
}

impl fmt::Debug for WidgetRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WidgetRegistry")
            .field("kinds", &self.factories.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl WidgetRegistry {
    /// Empty registry — register kinds explicitly via [`Self::register`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            factories: BTreeMap::new(),
        }
    }

    /// Registry pre-populated with the in-tree widgets: `time` and
    /// `session-name`.
    #[must_use]
    pub fn with_builtins() -> Self {
        let mut r = Self::new();
        r.register("time", widgets::time::factory);
        r.register("session-name", widgets::session_name::factory);
        r
    }

    /// Register a factory under `kind`. Later registrations overwrite
    /// earlier ones for the same `kind`.
    pub fn register(&mut self, kind: &'static str, factory: WidgetFactory) {
        self.factories.insert(kind, factory);
    }

    /// Look up a kind and invoke its factory.
    ///
    /// # Errors
    ///
    /// Returns [`WidgetError::UnknownKind`] if `spec.kind` is not
    /// registered, or forwards [`WidgetError::InvalidOption`] from the
    /// factory.
    pub fn build(&self, spec: &WidgetSpec) -> Result<Box<dyn StatusWidget>, WidgetError> {
        let factory = self
            .factories
            .get(spec.kind.as_str())
            .ok_or_else(|| WidgetError::UnknownKind(spec.kind.clone()))?;
        factory(&spec.opts)
    }

    /// Registered widget kinds, in ASCII order (handy for tests).
    #[must_use]
    pub fn kinds(&self) -> Vec<&'static str> {
        self.factories.keys().copied().collect()
    }
}

impl Default for WidgetRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

/// Failures from widget construction.
#[derive(Debug, thiserror::Error)]
pub enum WidgetError {
    /// `spec.kind` was not in the registry.
    #[error("unknown widget kind: {0}")]
    UnknownKind(String),
    /// A factory rejected one of its options.
    #[error("invalid option for widget {kind}: {message}")]
    InvalidOption {
        /// The widget kind that rejected the option.
        kind: String,
        /// Human-readable explanation.
        message: String,
    },
}
