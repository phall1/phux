use phux_core::screen::{CellColor, CellInfo, CellStyle, ScreenState, SemanticContent};

use libghostty_vt::{
    render::CellIteration,
    screen::CellSemanticContent,
    style::{RgbColor, Style, StyleColor},
};

use super::synthesizer::SynthesisError;

/// Project one viewport cell into a [`CellInfo`], or `None` when the cell
/// carries neither a non-default style nor an OSC-133 semantic mark
/// (`phux-8yl`).
///
/// Returning `None` for plain cells keeps the [`ScreenState::cells`] vec
/// sparse: a mostly-blank grid emits almost nothing, so the JSON stays
/// small while every styled or semantically-marked cell is still reported.
/// `row`/`col` are the viewport-relative, zero-based coordinates of the
/// cell's left edge (wide-cell tails are skipped by the caller).
pub(crate) fn collect_cell(
    cell: &CellIteration<'_, '_>,
    row: u16,
    col: u16,
) -> Result<Option<CellInfo>, SynthesisError> {
    let style = cell.style()?;
    // libghostty defaults every cell's semantic content to `Output`,
    // whether or not the shell emitted any OSC-133 marks — so `Output` is
    // the *absence* of a meaningful mark, not a signal. Collapse it to
    // `None` and surface only `Input` / `Prompt`, the marks an agent can
    // actually act on; this also keeps the cells projection sparse (a grid
    // with no shell integration emits no semantic field at all).
    let semantic = match cell.raw_cell()?.semantic_content()? {
        CellSemanticContent::Output => None,
        CellSemanticContent::Input => Some(SemanticContent::Input),
        CellSemanticContent::Prompt => Some(SemanticContent::Prompt),
    };

    // Resolve fg/bg via the iteration's color helpers (which apply the
    // palette/default), falling back to the raw `StyleColor` so a palette
    // index survives as a palette index in the projection.
    let fg = cell_color(cell.fg_color()?, style.fg_color);
    let bg = cell_color(cell.bg_color()?, style.bg_color);

    let cell_style = CellStyle {
        bold: style.bold,
        faint: style.faint,
        italic: style.italic,
        underline: !matches!(style.underline, libghostty_vt::style::Underline::None),
        blink: style.blink,
        inverse: style.inverse,
        invisible: style.invisible,
        strikethrough: style.strikethrough,
        overline: style.overline,
        fg,
        bg,
    };

    // Sparse: drop cells that carry nothing an agent could act on.
    if semantic.is_none() && cell_style == DEFAULT_CELL_STYLE {
        return Ok(None);
    }

    Ok(Some(CellInfo {
        col,
        row,
        semantic,
        style: cell_style,
    }))
}

/// The all-off, all-default [`CellStyle`] — the sentinel `collect_cell`
/// compares against to keep the cells projection sparse.
const DEFAULT_CELL_STYLE: CellStyle = CellStyle {
    bold: false,
    faint: false,
    italic: false,
    underline: false,
    blink: false,
    inverse: false,
    invisible: false,
    strikethrough: false,
    overline: false,
    fg: CellColor::Default,
    bg: CellColor::Default,
};

/// Project a cell color to [`CellColor`].
///
/// Prefers the cell's explicit per-cell [`StyleColor`] so a palette index
/// keeps its identity (`Palette { index }`) rather than collapsing to RGB.
/// When the cell sets no explicit color (`StyleColor::None`) but the
/// iteration still resolves a concrete RGB (`resolved` — e.g. a non-default
/// background inherited from the terminal palette), that RGB is surfaced;
/// otherwise the projection is [`CellColor::Default`].
pub(crate) fn cell_color(resolved: Option<RgbColor>, raw: StyleColor) -> CellColor {
    match raw {
        StyleColor::Palette(index) => CellColor::Palette { index: index.0 },
        StyleColor::Rgb(rgb) => CellColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        },
        StyleColor::None => resolved.map_or(CellColor::Default, |rgb| CellColor::Rgb {
            r: rgb.r,
            g: rgb.g,
            b: rgb.b,
        }),
    }
}
