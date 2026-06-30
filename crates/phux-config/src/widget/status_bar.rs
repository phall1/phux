//! Status-bar composer: turns a [`StatusCfg`] plus a [`WidgetRegistry`]
//! into a runtime [`StatusBar`], and lays widget output into a single
//! row of [`Cell`]s.
//!
//! Owned by `phux-nz4.5`. The composer is host-agnostic — it does not
//! emit VT, does not pick a screen row, and does not own a clock; it
//! just produces a `width`-wide cell strip on demand. The TUI client
//! (`phux-client::attach::status_bar`) takes that strip, paints it at
//! the bottom of the outer terminal, and decides the refresh cadence.
//!
//! Layout: three slots from [`StatusCfg`] — `left`, `center`, `right` —
//! each a list of widgets rendered with no implicit separator (per
//! `docs/consumers/tui.md` §8.4). Slots are concatenated independently, then
//! placed:
//!
//! - `left` flush at column 0,
//! - `right` flush against the last column (`width - 1`),
//! - `center` centered in whatever gap remains between the two.
//!
//! Truncation is left-biased: when the three slots together overflow
//! `width`, the right slot is preserved first, then the left, and the
//! center yields. Within a slot we drop trailing cells once the slot's
//! budget runs out. The result vector is always exactly `width` cells
//! long, padded with blank cells where slots don't reach.

use std::collections::BTreeMap;

use crate::schema::{StatusCfg, Widget, WidgetSpec};
use crate::widget::{Cell, StatusWidget, WidgetCells, WidgetContext, WidgetError, WidgetRegistry};

/// One composed slot's worth of widgets.
struct Slot {
    widgets: Vec<Box<dyn StatusWidget>>,
}

impl Slot {
    fn build(specs: &[Widget], registry: &WidgetRegistry) -> Result<Self, WidgetError> {
        let mut widgets = Vec::with_capacity(specs.len());
        for entry in specs {
            let spec = match entry {
                Widget::Bare(kind) => WidgetSpec {
                    kind: kind.clone(),
                    opts: BTreeMap::new(),
                },
                Widget::Spec(s) => s.clone(),
            };
            widgets.push(registry.build(&spec)?);
        }
        Ok(Self { widgets })
    }

    fn render(&self, ctx: &WidgetContext<'_>) -> Vec<Cell> {
        let mut out: Vec<Cell> = Vec::new();
        for w in &self.widgets {
            let WidgetCells { cells } = w.render(ctx);
            out.extend(cells);
        }
        out
    }
}

/// The composed status bar.
///
/// Built once from a parsed [`StatusCfg`] and a populated
/// [`WidgetRegistry`]; rendered per-tick into a [`Vec<Cell>`] of caller-
/// supplied width.
pub struct StatusBar {
    left: Slot,
    center: Slot,
    right: Slot,
}

impl std::fmt::Debug for StatusBar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusBar")
            .field("left.len", &self.left.widgets.len())
            .field("center.len", &self.center.widgets.len())
            .field("right.len", &self.right.widgets.len())
            .finish()
    }
}

impl StatusBar {
    /// Build a [`StatusBar`] from parsed config + a populated widget
    /// registry.
    ///
    /// # Errors
    ///
    /// Forwards any [`WidgetError`] from the registry — most commonly
    /// `UnknownKind` (a widget kind in config that the registry has no
    /// factory for) or `InvalidOption` (a factory rejected its TOML
    /// options).
    pub fn build(cfg: &StatusCfg, registry: &WidgetRegistry) -> Result<Self, WidgetError> {
        Ok(Self {
            left: Slot::build(&cfg.left, registry)?,
            center: Slot::build(&cfg.center, registry)?,
            right: Slot::build(&cfg.right, registry)?,
        })
    }

    /// An empty bar: no widgets in any slot.
    ///
    /// phux-9vf: the TUI's error-line painter wraps an empty bar so the
    /// widget pipeline produces no output — the painter substitutes a
    /// fixed diagnostic row instead. Cheaper and clearer than threading
    /// an `Option<StatusBar>` through the painter.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            left: Slot {
                widgets: Vec::new(),
            },
            center: Slot {
                widgets: Vec::new(),
            },
            right: Slot {
                widgets: Vec::new(),
            },
        }
    }

    /// True if no slot carries any widgets — caller may then skip
    /// reserving a status row entirely.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.left.widgets.is_empty()
            && self.center.widgets.is_empty()
            && self.right.widgets.is_empty()
    }

    /// Render the bar at the supplied display width. Returns exactly
    /// `width` cells, padded with blanks where slots don't reach.
    ///
    /// Truncation policy on overflow: right wins first, then left,
    /// center yields last.
    #[must_use]
    pub fn render(&self, ctx: &WidgetContext<'_>, width: u16) -> Vec<Cell> {
        let width = usize::from(width);
        if width == 0 {
            return Vec::new();
        }

        let left = self.left.render(ctx);
        let mut center = self.center.render(ctx);
        let mut right = self.right.render(ctx);

        // Budget: right gets up to width; left gets whatever's left after
        // right; center gets whatever's left after both.
        let right_take = right.len().min(width);
        right.truncate(right_take);

        let mut left = left;
        let left_budget = width.saturating_sub(right_take);
        let left_take = left.len().min(left_budget);
        left.truncate(left_take);

        let center_budget = width.saturating_sub(left_take + right_take);
        let center_take = center.len().min(center_budget);
        center.truncate(center_take);

        // Compose into a fixed-width row.
        let mut row: Vec<Cell> = vec![Cell::default(); width];

        // Left: flush at column 0.
        for (i, c) in left.into_iter().enumerate() {
            row[i] = c;
        }

        // Right: flush at the last column.
        let right_start = width - right_take;
        for (i, c) in right.into_iter().enumerate() {
            row[right_start + i] = c;
        }

        // Center: centered within the gap between left and right.
        let gap_start = left_take;
        let gap_end = right_start;
        let gap_width = gap_end.saturating_sub(gap_start);
        let center_offset = gap_start + gap_width.saturating_sub(center_take) / 2;
        for (i, c) in center.into_iter().enumerate() {
            row[center_offset + i] = c;
        }

        row
    }
}

/// Convenience: collect the printable text of a rendered row into a
/// `String`. Blank cells become spaces. Useful for tests and for the
/// minimal "render to bytes" path the TUI client uses.
#[must_use]
pub fn row_to_string(row: &[Cell]) -> String {
    let mut s = String::with_capacity(row.len());
    for cell in row {
        match cell.text.first() {
            Some(ch) => s.push(*ch),
            None => s.push(' '),
        }
    }
    s
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::schema::{StatusCfg, Widget, WidgetSpec};
    use std::time::{Duration, UNIX_EPOCH};

    fn ctx_with(session: &str) -> WidgetContext<'_> {
        WidgetContext {
            now: UNIX_EPOCH + Duration::from_secs(0),
            session_name: session,
            prefix: "C-a",
            windows: &[],
        }
    }

    fn spec(kind: &str, opts: &[(&str, toml::Value)]) -> Widget {
        Widget::Spec(WidgetSpec {
            kind: kind.to_owned(),
            opts: opts
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
        })
    }

    #[test]
    fn empty_config_is_empty() {
        let cfg = StatusCfg::default();
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        assert!(bar.is_empty());
        let row = bar.render(&ctx_with(""), 10);
        assert_eq!(row.len(), 10);
        assert!(row.iter().all(|c| c.text.is_empty()));
    }

    #[test]
    fn left_slot_flushes_left() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        let row = bar.render(&ctx_with("alpha"), 20);
        let s = row_to_string(&row);
        assert_eq!(s, "alpha               ");
    }

    #[test]
    fn right_slot_flushes_right() {
        let cfg = StatusCfg {
            right: vec![Widget::Bare("session-name".into())],
            ..Default::default()
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        let row = bar.render(&ctx_with("beta"), 10);
        let s = row_to_string(&row);
        assert_eq!(s, "      beta");
    }

    #[test]
    fn three_slots_compose() {
        // left=session, center=session, right=session — distinct names
        // are hard without a second widget; reuse the same widget with
        // different prefixes via spec form.
        let cfg = StatusCfg {
            left: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("L:".into()))],
            )],
            center: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("C:".into()))],
            )],
            right: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("R:".into()))],
            )],
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        let row = bar.render(&ctx_with("x"), 20);
        let s = row_to_string(&row);
        // 3 cells per slot: L:x (3) … C:x (3) centered … R:x (3) flush right
        // Gap = 20 - 3 - 3 = 14; center starts at 3 + (14-3)/2 = 3+5 = 8.
        assert_eq!(s, "L:x     C:x      R:x");
        assert_eq!(s.len(), 20);
    }

    #[test]
    fn truncation_preserves_right_then_left_then_center() {
        let cfg = StatusCfg {
            left: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("LEFT".into()))],
            )],
            center: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("CENTER".into()))],
            )],
            right: vec![spec(
                "session-name",
                &[("prefix", toml::Value::String("RIGHT".into()))],
            )],
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        // Total raw: LEFT(4) + CENTER(6) + RIGHT(5) = 15. Width 10 means
        // right (5) wins, left gets 5 (LEFT + 'C' from CENTER session?
        // No — left's render is "LEFT" + ""=session="" → "LEFT", 4 cells).
        // Left fits in budget(5). Center budget = 10 - 4 - 5 = 1, so center
        // truncates to its first cell 'C'.
        let row = bar.render(&ctx_with(""), 10);
        let s = row_to_string(&row);
        assert_eq!(s, "LEFTCRIGHT");
        assert_eq!(s.len(), 10);
    }

    #[test]
    fn zero_width_returns_empty() {
        let cfg = StatusCfg::default();
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        let row = bar.render(&ctx_with(""), 0);
        assert!(row.is_empty());
    }

    #[test]
    fn unknown_widget_kind_propagates_error() {
        let cfg = StatusCfg {
            left: vec![Widget::Bare("not-a-real-widget".into())],
            ..Default::default()
        };
        let reg = WidgetRegistry::with_builtins();
        match StatusBar::build(&cfg, &reg) {
            Err(WidgetError::UnknownKind(k)) => assert_eq!(k, "not-a-real-widget"),
            other => panic!("expected UnknownKind, got {other:?}"),
        }
    }

    #[test]
    fn time_and_session_render_together() {
        // Mirrors the integration target: bar with both built-in widgets.
        let cfg = StatusCfg {
            left: vec![Widget::Bare("session-name".into())],
            right: vec![spec(
                "time",
                &[("format", toml::Value::String("YEAR".into()))],
            )],
            ..Default::default()
        };
        let reg = WidgetRegistry::with_builtins();
        let bar = StatusBar::build(&cfg, &reg).unwrap();
        // "YEAR" is a literal (no `%` escapes) so the time widget renders
        // it verbatim regardless of clock — deterministic snapshot.
        let row = bar.render(&ctx_with("main"), 20);
        let s = row_to_string(&row);
        assert_eq!(s, "main            YEAR");
    }
}
