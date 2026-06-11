//! `windows` widget — the tmux-style tab bar.
//!
//! Renders one styled segment per window from [`WidgetContext::windows`],
//! the active one in the `active` style and the rest in `inactive`,
//! joined by `separator`. Each segment's text comes from `format` with
//! `{index}` (0-based position, the `select-window` selector) and
//! `{name}` (the editable label) substituted.

use std::collections::BTreeMap;

use crate::widget::{Cell, CellStyle, StatusWidget, WidgetCells, WidgetContext, WidgetError};

/// Widget kind, used in error messages.
const KIND: &str = "windows";

/// `windows` (tab-bar) widget.
#[derive(Debug, Clone)]
pub struct WindowsWidget {
    /// Style applied to the active window's segment.
    pub active: CellStyle,
    /// Style applied to inactive windows' segments.
    pub inactive: CellStyle,
    /// Literal text placed between segments.
    pub separator: String,
    /// Per-segment template; `{index}` and `{name}` are substituted.
    pub format: String,
}

impl Default for WindowsWidget {
    fn default() -> Self {
        Self {
            // Theme-agnostic, eye-catching default: the active tab is
            // bold reverse-video; inactive tabs are dimmed.
            active: CellStyle {
                bold: true,
                reverse: true,
                ..CellStyle::default()
            },
            inactive: CellStyle {
                dim: true,
                ..CellStyle::default()
            },
            separator: " ".to_owned(),
            format: "{index}:{name}".to_owned(),
        }
    }
}

impl WindowsWidget {
    #[allow(
        clippy::literal_string_with_formatting_args,
        reason = "`{index}`/`{name}` are this widget's own template placeholders, not std format args"
    )]
    fn segment_text(&self, index: usize, name: &str) -> String {
        self.format
            .replace("{index}", &index.to_string())
            .replace("{name}", name)
    }
}

impl StatusWidget for WindowsWidget {
    fn render(&self, ctx: &WidgetContext<'_>) -> WidgetCells {
        let mut cells: Vec<Cell> = Vec::new();
        for (i, w) in ctx.windows.iter().enumerate() {
            if i > 0 && !self.separator.is_empty() {
                cells.extend(WidgetCells::from_styled(&self.separator, None).cells);
            }
            // phux-x2hm: a zoomed active window gets tmux's `Z` marker.
            let mut text = self.segment_text(i, &w.name);
            if w.zoomed {
                text.push_str(" Z");
            }
            let style = if w.active {
                self.active.clone()
            } else {
                self.inactive.clone()
            };
            let style = if style.is_plain() { None } else { Some(style) };
            cells.extend(WidgetCells::from_styled(&text, style).cells);
        }
        WidgetCells { cells }
    }

    // No `poll_interval` — the tab bar repaints when the layout changes,
    // which the client drives via the status-bar repaint path.
}

/// Factory: builds a [`WindowsWidget`] from a TOML `opts` map.
///
/// Accepted keys (all optional; omitted keys keep the default preset):
/// - `active` / `inactive` (inline table) — a [`CellStyle`]:
///   `fg`/`bg` (color strings), `bold`/`dim`/`italic`/`underline`/`reverse`
///   (bools).
/// - `separator` (string) — text between segments (default `" "`).
/// - `format` (string) — segment template with `{index}`/`{name}`
///   (default `"{index}:{name}"`).
///
/// # Errors
///
/// Returns [`WidgetError::InvalidOption`] if a value is the wrong type or
/// a style table has an unknown field.
pub(in crate::widget) fn factory(
    opts: &BTreeMap<String, toml::Value>,
) -> Result<Box<dyn StatusWidget>, WidgetError> {
    let defaults = WindowsWidget::default();
    let active = style_opt(opts, "active")?.unwrap_or(defaults.active);
    let inactive = style_opt(opts, "inactive")?.unwrap_or(defaults.inactive);
    let separator = string_opt(opts, "separator")?.unwrap_or(defaults.separator);
    let format = string_opt(opts, "format")?.unwrap_or(defaults.format);
    Ok(Box::new(WindowsWidget {
        active,
        inactive,
        separator,
        format,
    }))
}

/// Parse an optional [`CellStyle`] from an inline-table option.
fn style_opt(
    opts: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<Option<CellStyle>, WidgetError> {
    opts.get(key).map_or(Ok(None), |value| {
        value
            .clone()
            .try_into::<CellStyle>()
            .map(Some)
            .map_err(|e| WidgetError::InvalidOption {
                kind: KIND.to_owned(),
                message: format!("`{key}` must be a style table: {e}"),
            })
    })
}

/// Parse an optional string option.
fn string_opt(
    opts: &BTreeMap<String, toml::Value>,
    key: &str,
) -> Result<Option<String>, WidgetError> {
    match opts.get(key) {
        None => Ok(None),
        Some(toml::Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(WidgetError::InvalidOption {
            kind: KIND.to_owned(),
            message: format!("`{key}` must be a string, got {}", other.type_str()),
        }),
    }
}
