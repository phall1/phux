//! Selection extraction: turn a [`Match`] coordinate span into the text under
//! it, via libghostty's sound one-shot selection-format API.
//!
//! # Why this exists (`phux-3sy` / `phux-97w`, the unblocked bridge)
//!
//! [`search`](crate::search) locates a literal needle in the rows phux
//! already mirrors and returns a [`Match`]: a [`Region`] plus a `row` and a
//! `col`/`len` span measured in `char` offsets into the *right-trimmed
//! projected text row* (the exact `String` rows
//! [`screen_state_with_scrollback`](crate::grid::SnapshotSynthesizer::screen_state_with_scrollback)
//! builds). That is the search layer's coordinate space, not libghostty's.
//!
//! This module is the copy/extract primitive the selection/copy-mode epic
//! (`phux-abi`) needs: it converts that `(Region, row, char-col, len)` span
//! into a [`libghostty_vt::selection::Selection`] over the terminal's grid
//! and formats just that range back to plain text via the sound one-shot
//! [`Terminal::format_selection_alloc`] path. It is deliberately *only* the
//! find-coords -> selection -> text extraction primitive: no copy-mode UI,
//! no cursor, no highlight.
//!
//! # The wide-glyph translation (the crux, 3sy's caveat)
//!
//! A `char` offset into the trimmed text row diverges from a grid column
//! whenever the row holds wide glyphs (CJK / emoji) or multi-`char` grapheme
//! clusters (a base codepoint plus combining marks). The projection (see
//! [`screen_state_with_scrollback`](crate::grid::SnapshotSynthesizer::screen_state_with_scrollback))
//! builds each row by
//! walking cells in grid order, skipping [`CellWide::SpacerTail`] cells (the
//! right half of a wide glyph emits no column), pushing each cell's whole
//! grapheme cluster as one-or-more `char`s, and advancing the grid column by
//! the cell's display width (2 for [`CellWide::Wide`], else 1). For example,
//! the row `你好xy` occupies grid columns 0 (wide `你`), 1 (its skipped
//! tail), 2 (wide `好`), 3 (its tail), 4 (`x`), 5 (`y`); its trimmed text is
//! the 4-`char` string `你好xy` whose char offsets 0,1,2,3 map to grid
//! columns 0,2,4,5.
//!
//! The private `char_col_to_grid_x` re-walks the same cells in the same order
//! to invert that mapping: given a target `char` offset it returns the grid column of
//! the cell that *contains* that offset. Mapping the cell's *left edge* is
//! correct for a wide glyph — a selection endpoint on the wide base covers
//! the whole glyph, and libghostty's `SpacerTail` is not independently
//! selectable.
//!
//! # Point space
//!
//! A [`Region::Viewport`] match maps to [`Point::Active`] coordinates (the
//! viewport walk projects active-area rows top-first, `y = row`), so the
//! viewport/active case is implemented fully and tested. The
//! [`Region::Scrollback`] (history) case is reported precisely in the
//! structured handoff rather than shipped, because the history row index the
//! search layer reports (oldest-first, bounded by a possible `RecentHistory`
//! window) does not have an unambiguous one-to-one mapping onto a
//! [`Point::History`] `y` from this module's inputs alone — see
//! [`extract_match`]'s error path and the module-level note in the report.

use libghostty_vt::{
    Terminal,
    fmt::Format,
    screen::CellWide,
    selection::{FormatOptions, Selection},
    terminal::{Point, PointCoordinate},
};

use crate::grid::{GRAPHEME_INLINE, SynthesisError};
use crate::search::{Match, Region};

/// Errors from translating a [`Match`] to a selection and extracting its text.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// Surfaced from the libghostty render/selection path.
    #[error("libghostty: {0}")]
    Synthesis(#[from] SynthesisError),
    /// Surfaced directly from a libghostty call (`grid_ref`, format).
    #[error("libghostty: {0}")]
    Ghostty(#[from] libghostty_vt::Error),
    /// The match's `char` column could not be resolved to a grid column —
    /// the row is shorter (in non-tail cells) than the reported `col`. This
    /// indicates the terminal mutated between [`search`](crate::search) and
    /// extraction, or a stale [`Match`].
    #[error("match column {col} is past the end of row {row} ({region:?})")]
    ColumnOutOfRange {
        /// The region the out-of-range match referenced.
        region: Region,
        /// The row index within that region.
        row: usize,
        /// The `char` column that fell past the row's last non-tail cell.
        col: usize,
    },
    /// Extraction for a [`Region::Scrollback`] match is not implemented: the
    /// search layer's history row index has no unambiguous mapping onto a
    /// [`Point::History`] coordinate from this primitive's inputs alone (see
    /// the module-level note). Callers wanting scrollback extraction must
    /// resolve the absolute history `y` themselves and call
    /// [`extract_active_span`].
    #[error(
        "scrollback (history) extraction is not implemented; resolve the absolute history y and use extract_active_span"
    )]
    ScrollbackUnsupported,
}

/// Extract the text under a [`Match`] as a plain-text `String`.
///
/// Only [`Region::Viewport`] matches are supported here, mapping the match's
/// `row` directly onto [`Point::Active`] `y`. A [`Region::Scrollback`] match
/// returns [`ExtractError::ScrollbackUnsupported`] — see the module docs for
/// why the history `y` is ambiguous from a [`Match`] alone.
///
/// The match's `char`-offset `col`/`len` are translated to grid columns via
/// the private `char_col_to_grid_x` (handling wide glyphs and multi-`char`
/// clusters), then a linear [`Selection`] is built over `[start_x, end_x]` of row `y`
/// and formatted with the sound one-shot
/// [`Terminal::format_selection_alloc`] API.
pub fn extract_match(terminal: &Terminal<'_, '_>, m: Match) -> Result<String, ExtractError> {
    match m.region {
        Region::Viewport => {
            let y = u32::try_from(m.row).unwrap_or(u32::MAX);
            extract_active_span(terminal, y, m.col, m.len).map_err(|e| match e {
                // Re-stamp the region/row so the error names the viewport
                // match the caller passed, not the raw active-space inputs.
                ExtractError::ColumnOutOfRange { col, .. } => ExtractError::ColumnOutOfRange {
                    region: Region::Viewport,
                    row: m.row,
                    col,
                },
                other => other,
            })
        }
        Region::Scrollback => Err(ExtractError::ScrollbackUnsupported),
    }
}

/// Extract the text spanning `len` `char`s starting at `char` column
/// `char_col` of active-area row `y`, as plain text.
///
/// This is the point-space-explicit primitive [`extract_match`] delegates to
/// for the viewport case, and the entry a caller with a resolved history `y`
/// would use after translating the [`Point::History`] coordinate into
/// active space themselves. `char_col`/`len` are `char` offsets into the same
/// right-trimmed projected text row the search layer reports; they are
/// translated to grid columns here.
///
/// A zero `len` extracts nothing and returns an empty string without touching
/// libghostty's selection path.
pub fn extract_active_span(
    terminal: &Terminal<'_, '_>,
    y: u32,
    char_col: usize,
    len: usize,
) -> Result<String, ExtractError> {
    if len == 0 {
        return Ok(String::new());
    }

    let out_of_range = |col: usize| ExtractError::ColumnOutOfRange {
        region: Region::Viewport,
        row: usize::try_from(y).unwrap_or(usize::MAX),
        col,
    };

    // Translate the start and the (inclusive) last char of the span to grid
    // columns. The selection endpoints are inclusive (see `Selection::new`),
    // so the end maps the last char, `char_col + len - 1`.
    let start_x =
        char_col_to_grid_x(terminal, y, char_col)?.ok_or_else(|| out_of_range(char_col))?;
    let last_char = char_col + len - 1;
    let end_x =
        char_col_to_grid_x(terminal, y, last_char)?.ok_or_else(|| out_of_range(last_char))?;

    let start = terminal.grid_ref(Point::Active(PointCoordinate { x: start_x, y }))?;
    let end = terminal.grid_ref(Point::Active(PointCoordinate { x: end_x, y }))?;
    let selection = Selection::new(start, end, false);

    let bytes = terminal
        .format_selection_alloc(
            None,
            FormatOptions::new()
                .with_emit_format(Format::Plain)
                .with_trim(true)
                .with_unwrap(true)
                .with_selection(&selection),
        )?
        // A non-empty selection over real cells yields Some(bytes); None
        // means libghostty saw nothing selectable in the range, which we
        // surface as the empty string rather than an error.
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    Ok(bytes)
}

/// Map a `char` offset into active-area row `y`'s right-trimmed projected
/// text to the grid column of the cell that contains it.
///
/// Walks the row's cells in grid order exactly as the text projection does:
/// skip [`CellWide::SpacerTail`] cells (they emit no `char` and own no
/// column), count each visited cell's grapheme-cluster `char`s, and advance
/// the grid column by the cell's display width (2 for [`CellWide::Wide`],
/// else 1). The first cell whose accumulated `char` range covers `target` is
/// the answer, and its *left* grid column is returned (a wide glyph's base
/// column, not its skipped tail).
///
/// Returns `Ok(None)` when `target` lands at or past the row's last non-tail
/// cell — the row's projected text is shorter than `target + 1` `char`s.
///
/// This mirrors the projection's right-trim only implicitly: trailing blank
/// cells contribute a single space `char` each here, but a `target` inside
/// the *trimmed* text always falls on a real (non-trailing-blank) cell, so
/// the trim never changes the answer for an in-range `target`. An empty
/// (blank) cell emits one space `char` and one column, matching the
/// projection's `buf.push(' ')`.
fn char_col_to_grid_x(
    terminal: &Terminal<'_, '_>,
    y: u32,
    target: usize,
) -> Result<Option<u16>, ExtractError> {
    let cols = terminal.cols()?;
    let mut char_acc = 0usize;
    let mut grid_x = 0u16;

    while grid_x < cols {
        let point = Point::Active(PointCoordinate { x: grid_x, y });
        let grid_ref = terminal.grid_ref(point)?;
        let wide = grid_ref.cell()?.wide()?;
        if matches!(wide, CellWide::SpacerTail) {
            // Right half of a wide glyph: emits no char and owns no column in
            // the projection. Advance one grid column without counting chars.
            grid_x += 1;
            continue;
        }

        // How many `char`s this cell contributes to the projected text. A
        // blank cell contributes exactly one (a space).
        let cluster_chars = grapheme_char_count(&grid_ref)?.max(1);
        if target < char_acc + cluster_chars {
            // `target` falls within this cell's grapheme cluster; the cell's
            // left grid column is the selection endpoint.
            return Ok(Some(grid_x));
        }
        char_acc += cluster_chars;

        grid_x = grid_x.saturating_add(if matches!(wide, CellWide::Wide) { 2 } else { 1 });
    }

    Ok(None)
}

/// Count the `char`s in the grapheme cluster at `grid_ref`, with a stack
/// buffer fast path and a heap retry for deep clusters — mirroring the
/// scrollback projection's read.
fn grapheme_char_count(
    grid_ref: &libghostty_vt::screen::GridRef<'_>,
) -> Result<usize, ExtractError> {
    let mut inline = [char::from(0u8); GRAPHEME_INLINE];
    match grid_ref.graphemes(&mut inline) {
        Ok(n) => Ok(n),
        Err(libghostty_vt::Error::OutOfSpace { required }) => {
            let mut heap = vec![char::from(0u8); required];
            Ok(grid_ref.graphemes(&mut heap)?)
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use crate::search::{Scope, SearchOptions, search_oneshot};
    use libghostty_vt::{Terminal, TerminalOptions};

    fn fresh(cols: u16, rows: u16) -> Terminal<'static, 'static> {
        Terminal::new(TerminalOptions {
            cols,
            rows,
            max_scrollback: 100,
        })
        .expect("Terminal::new")
    }

    fn vp() -> SearchOptions {
        SearchOptions {
            case_insensitive: false,
            include_viewport: true,
        }
    }

    #[test]
    fn extracts_plain_ascii_match_text() {
        // (a) A plain-ASCII match round-trips its exact text: search finds
        // the needle, extraction pulls back the same bytes.
        let mut t = fresh(40, 3);
        t.vt_write(b"the quick brown fox");
        let hits = search_oneshot(&t, "brown", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let text = extract_match(&t, hits[0]).expect("extract");
        assert_eq!(text, "brown", "the extracted text matches the needle");
    }

    #[test]
    fn extracts_whole_word_with_neighbours() {
        // A longer span (two words) round-trips, proving the inclusive
        // end-column math (char_col + len - 1) covers the full range.
        let mut t = fresh(40, 3);
        t.vt_write(b"alpha bravo charlie delta");
        let hits = search_oneshot(&t, "bravo charlie", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let text = extract_match(&t, hits[0]).expect("extract");
        assert_eq!(text, "bravo charlie");
    }

    #[test]
    fn wide_glyph_before_match_shifts_grid_column() {
        // (b) A wide glyph BEFORE the match: the char offset and the grid
        // column diverge, and extraction must use the grid column.
        //
        // Row "你好xTODO": grid columns 0(你 wide),1(tail),2(好 wide),3(tail),
        // 4(x),5(T),6(O),7(D),8(O). Projected text is "你好xTODO" — char
        // offsets 0(你),1(好),2(x),3(T)... So "TODO" is at CHAR offset 3 but
        // GRID column 5. If extraction used the char offset (3) as the grid
        // x, the selection would start on 好's tail / x and the text would be
        // wrong. The grid mapping must put the start at grid column 5.
        let mut t = fresh(40, 3);
        t.vt_write("你好xTODO".as_bytes());

        let hits = search_oneshot(&t, "TODO", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let m = hits[0];
        assert_eq!(m.col, 3, "char offset of TODO is 3 (你, 好, x precede it)");

        // Direct grid-column translation: char offset 3 -> grid column 5.
        let start_x = char_col_to_grid_x(&t, 0, 3)
            .expect("translate")
            .expect("in range");
        assert_eq!(
            start_x, 5,
            "char offset 3 maps to grid column 5, not 3 (two wide glyphs precede it)"
        );

        let text = extract_match(&t, m).expect("extract");
        assert_eq!(
            text, "TODO",
            "the wide-glyph-aware grid mapping extracts the right text"
        );
    }

    #[test]
    fn extracts_match_that_includes_a_wide_glyph() {
        // The needle itself spans a wide glyph: "好x" is char offsets 1..=2
        // but grid columns 2 and 4 (好 is wide). The inclusive end must land
        // on grid column 4 (x), so the selection covers 好 (cols 2-3) and x.
        let mut t = fresh(40, 3);
        t.vt_write("你好x".as_bytes());
        let hits = search_oneshot(&t, "好x", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let text = extract_match(&t, hits[0]).expect("extract");
        assert_eq!(text, "好x");
    }

    #[test]
    fn multi_row_region_extracts_each_row() {
        // (c) A multi-row selection: extract a span on a second viewport row,
        // proving the row->y mapping is per-row (y = match.row).
        let mut t = fresh(40, 4);
        t.vt_write(b"first line here\r\nsecond MARKER line");
        let hits = search_oneshot(&t, "MARKER", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let m = hits[0];
        assert_eq!(m.region, Region::Viewport);
        assert_eq!(m.row, 1, "MARKER is on the second viewport row");
        let text = extract_match(&t, m).expect("extract");
        assert_eq!(text, "MARKER");
    }

    #[test]
    fn match_to_extract_round_trips_searched_content() {
        // The evolved 97w pin-test: a real Match -> extract round-trip over
        // content the search actually found, not a hardcoded sub-range. Every
        // hit's extracted text equals the searched needle.
        let mut t = fresh(40, 3);
        t.vt_write(b"needle once, needle twice, needle thrice");
        let hits = search_oneshot(&t, "needle", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 3, "three hits on the row");
        for m in hits {
            let text = extract_match(&t, m).expect("extract");
            assert_eq!(text, "needle", "each searched hit extracts its needle");
        }
    }

    #[test]
    fn scrollback_match_reports_unsupported() {
        // A history match returns the precise "not implemented" error rather
        // than a wrong mapping (the deliberate scope boundary).
        let mut t = fresh(20, 2);
        t.vt_write(b"alpha\r\nbravo\r\ncharlie\r\ndelta\r\necho");
        let hits = search_oneshot(&t, "bravo", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].region, Region::Scrollback);
        let err = extract_match(&t, hits[0]).expect_err("scrollback is unsupported");
        assert!(matches!(err, ExtractError::ScrollbackUnsupported));
    }

    #[test]
    fn out_of_range_column_errors() {
        // A col past the row's last non-tail cell surfaces ColumnOutOfRange
        // rather than panicking or silently truncating.
        let t = fresh(40, 3);
        // Empty viewport row 0: every cell is blank (one space char each), so
        // char offset 0 maps to grid column 0, but offset 100 (past the 40
        // columns) is out of range.
        let err = extract_active_span(&t, 0, 100, 1).expect_err("past row end");
        assert!(matches!(
            err,
            ExtractError::ColumnOutOfRange { col: 100, .. }
        ));
    }
}
