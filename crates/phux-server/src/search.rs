//! Find-in-scrollback: a thin literal search over the server's existing
//! scrollback mirror.
//!
//! # Scope (`phux-3sy`)
//!
//! This is the *only* copy-mode-adjacent primitive phux owns. Per
//! CONTRIBUTING ("Things we will not accept" / "A homegrown selection
//! engine"), word/line/output **selection** and text **extraction**
//! delegate to the host terminal and to libghostty-vt's Selection +
//! Formatter APIs. phux builds nothing there. What phux *does* own is
//! locating a literal needle in the rows it already mirrors for
//! [`ScreenState`](phux_core::screen::ScreenState) — libghostty exposes no
//! search or regex, so this layer is genuinely ours to build.
//!
//! The search runs over the same text rows
//! [`SnapshotSynthesizer::screen_state_with_scrollback`] projects: the
//! history rows above the viewport (read side-effect-free via
//! `Point::History`) followed by the live viewport rows. It returns match
//! coordinates — a [`Region`] discriminant plus row/column spans in that
//! region's own coordinate space. Those coordinates are what a consumer
//! would hand to a libghostty `Selection` + `Formatter` to pull the matched
//! text, VT, or HTML back out. This module owns *finding* only; it does not
//! highlight, maintain a cursor, or (yet) extract — see the note below.
//!
//! ## Selection + extraction: the delegation boundary, and the pin gap
//!
//! Per CONTRIBUTING ("A homegrown selection engine"), word/line/output
//! *boundary* logic and text *extraction* belong to the host terminal and to
//! libghostty. phux owns exactly one copy-mode-adjacent primitive: the
//! literal find-in-scrollback below. The plan was to also ship a thin,
//! fully-safe extraction bridge — turn a [`Match`]'s coordinate span into a
//! [`libghostty_vt::screen::Selection`] and run a
//! [`libghostty_vt::fmt::Formatter`] over it.
//!
//! The *types* for that bridge are all public at pin `acc4b87`: `Selection`
//! has public [`GridRef`](libghostty_vt::screen::GridRef) endpoints,
//! [`Terminal::grid_ref`] yields them, and `Formatter` accepts an
//! `Option<Selection>`. The OSC-133-aware boundary helpers
//! (`ghostty_terminal_select_word` / `_line` / `_output`) are *not* wrapped
//! — they need the raw terminal handle, which the safe crate seals
//! `pub(crate)` — but that is the boundary engine we deliberately refuse to
//! reimplement, so its absence is correct, not a gap.
//!
//! The real blocker is narrower and is upstream: the safe crate's
//! `Formatter::new_inner` builds the FFI options with
//! `selection: match selection.map(Into::into) { Some(s) => &raw const s, .. }`,
//! taking the address of a match-arm-local copy of the `ffi::Selection`
//! (which is `Copy`). That temporary is dropped before
//! `ghostty_formatter_terminal_new` reads through the pointer, so the
//! `selection: Some(..)` path passes a dangling pointer and libghostty
//! returns `InvalidValue`. The no-selection (whole-screen) path is sound and
//! works. Until a libghostty-rs pin fixes that, there is no *sound* way to
//! format a selection through the safe API, and phux will not reach into
//! `-sys` to reconstruct the raw handle ourselves. The coordinate-producing
//! half — fully under our control — ships here; the extraction wrapper waits
//! on the upstream fix. `safe_api_selection_formatter_bridge_is_broken_at_pin`
//! pins the gap and will fail loudly when the pin moves.

use crate::grid::{SnapshotSynthesizer, SynthesisError};
use libghostty_vt::Terminal;
// `Selection`, `Formatter`, `Point`, and friends are imported only inside the
// test module that pins the upstream extraction-bridge gap; see
// `safe_api_selection_formatter_bridge_is_broken_at_pin`.

/// Which region of the mirrored screen a [`Match`] falls in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// A history row above the active viewport. `row` is the index into the
    /// projected scrollback rows, oldest-first (`row = 0` is the oldest
    /// retained row), matching
    /// [`ScreenState::scrollback`](phux_core::screen::ScreenState::scrollback).
    Scrollback,
    /// A live viewport row. `row` is the zero-based viewport row, top-first,
    /// matching [`ScreenState::lines`](phux_core::screen::ScreenState::lines).
    Viewport,
}

/// One literal-needle hit in the mirrored screen.
///
/// Coordinates are in the [`Region`]'s own row space. `col` and `len` are
/// measured in Unicode scalar values (`char`s) within the right-trimmed row
/// text, not grid columns or bytes — they index the same `String` rows
/// [`SnapshotSynthesizer::screen_state_with_scrollback`] projects, so a
/// consumer can slice the row directly or translate to grid columns via the
/// row's own walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Match {
    /// Region the match falls in.
    pub region: Region,
    /// Row index within `region`.
    pub row: usize,
    /// Start column of the match, in `char` offsets into the row.
    pub col: usize,
    /// Length of the match, in `char`s.
    pub len: usize,
}

/// How a [`search`] needle is matched against row text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SearchOptions {
    /// Match without regard to ASCII case. Non-ASCII characters are compared
    /// verbatim (a full Unicode case fold is out of scope for a literal
    /// find).
    pub case_insensitive: bool,
    /// Include the live viewport rows in the search. When `false`, only the
    /// scrollback history above the viewport is searched.
    pub include_viewport: bool,
}

/// How much scrollback history a [`search`] covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Search every retained history row (plus the viewport, per
    /// [`SearchOptions::include_viewport`]).
    AllHistory,
    /// Search the most-recent `n` history rows (those nearest the viewport),
    /// plus the viewport per [`SearchOptions::include_viewport`].
    RecentHistory(u32),
}

/// Find every occurrence of `needle` in `terminal`'s mirrored scrollback
/// (and optionally its viewport), oldest match first.
///
/// This is a literal substring search — libghostty exposes no regex, and we
/// add none here. An empty `needle` yields no matches. Overlapping matches
/// on a single row are not reported: after a hit the scan resumes past the
/// match, mirroring `str::match_indices` semantics.
///
/// The read is side-effect-free: it reuses
/// [`SnapshotSynthesizer::screen_state_with_scrollback`], which neither
/// scrolls the viewport nor mutates `terminal`, so it is safe to poll
/// against a live pane.
///
/// To extract the text under a returned [`Match`], hand its coordinates to a
/// libghostty `Selection` + `Formatter`; see the module-level note on why
/// that extraction bridge is blocked on an upstream fix at the current pin.
///
/// The synthesizer and terminal share the libghostty allocator lifetime
/// `'alloc`: the pooled render iterators inside `synth` borrow from the same
/// arena the `terminal` was built in (mirroring
/// [`SnapshotSynthesizer::screen_state_with_scrollback`]).
pub fn search<'alloc>(
    synth: &mut SnapshotSynthesizer<'alloc>,
    terminal: &Terminal<'alloc, '_>,
    needle: &str,
    scope: Scope,
    opts: SearchOptions,
) -> Result<Vec<Match>, SynthesisError> {
    if needle.is_empty() {
        return Ok(Vec::new());
    }

    let scrollback = match scope {
        Scope::AllHistory => Some(crate::grid::SCROLLBACK_ALL),
        Scope::RecentHistory(n) => Some(n),
    };

    // Reuse the existing structured projection so the wide-cell, history,
    // and right-trim handling stays in one place. `cells = false` keeps the
    // per-cell projection unallocated; we only want the text rows.
    let screen = synth.screen_state_with_scrollback(terminal, 0, scrollback, false)?;

    let mut matches = Vec::new();
    for (row, line) in screen.scrollback.iter().enumerate() {
        find_in_row(line, needle, Region::Scrollback, row, opts, &mut matches);
    }
    if opts.include_viewport {
        for (row, line) in screen.lines.iter().enumerate() {
            find_in_row(line, needle, Region::Viewport, row, opts, &mut matches);
        }
    }
    Ok(matches)
}

/// Convenience: drive [`search`] from a one-shot synthesizer. Per-pane hot
/// paths should reuse a [`SnapshotSynthesizer`] and call [`search`] directly.
pub fn search_oneshot(
    terminal: &Terminal<'_, '_>,
    needle: &str,
    scope: Scope,
    opts: SearchOptions,
) -> Result<Vec<Match>, SynthesisError> {
    let mut synth = SnapshotSynthesizer::new()?;
    search(&mut synth, terminal, needle, scope, opts)
}

/// Append every (non-overlapping) hit of `needle` in `line` to `out`.
///
/// Columns and lengths are reported in `char` offsets into `line`. The scan
/// is byte-based against the chosen casing, then the byte offset is mapped to
/// a `char` offset so the reported coordinates index the row's scalar values
/// regardless of multi-byte UTF-8.
fn find_in_row(
    line: &str,
    needle: &str,
    region: Region,
    row: usize,
    opts: SearchOptions,
    out: &mut Vec<Match>,
) {
    // `char` length of the needle is stable across casing for ASCII-only
    // folding (the only folding we do), so compute it once.
    let needle_chars = needle.chars().count();

    if opts.case_insensitive {
        let hay = line.to_ascii_lowercase();
        let pat = needle.to_ascii_lowercase();
        // `hay` is a fresh String with the same char boundaries as `line`
        // (ASCII lowercasing never changes byte length), so byte offsets map
        // back onto `line` unchanged.
        push_byte_hits(&hay, &pat, line, region, row, needle_chars, out);
    } else {
        push_byte_hits(line, needle, line, region, row, needle_chars, out);
    }
}

/// Scan `hay` for `pat`, translating each byte offset into a `char` offset
/// in `origin` (the original, un-cased row) before recording the [`Match`].
fn push_byte_hits(
    hay: &str,
    pat: &str,
    origin: &str,
    region: Region,
    row: usize,
    needle_chars: usize,
    out: &mut Vec<Match>,
) {
    let mut from = 0usize;
    while let Some(rel) = hay[from..].find(pat) {
        let byte = from + rel;
        let col = origin[..byte].chars().count();
        out.push(Match {
            region,
            row,
            col,
            len: needle_chars,
        });
        // Advance past this match so hits don't overlap. `pat` is non-empty
        // (callers reject an empty needle), so `from` strictly increases.
        from = byte + pat.len();
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use libghostty_vt::{
        Terminal, TerminalOptions,
        fmt::{Format, Formatter, FormatterOptions},
        screen::Selection,
        terminal::{Point, PointCoordinate},
    };

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
    fn empty_needle_yields_no_matches() {
        let mut t = fresh(20, 3);
        t.vt_write(b"hello world");
        let hits = search_oneshot(&t, "", Scope::AllHistory, vp()).expect("search");
        assert!(hits.is_empty(), "an empty needle must match nothing");
    }

    #[test]
    fn finds_needle_in_viewport() {
        let mut t = fresh(40, 3);
        t.vt_write(b"the quick brown fox");
        let hits = search_oneshot(&t, "brown", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let m = hits[0];
        assert_eq!(m.region, Region::Viewport);
        assert_eq!(m.row, 0);
        assert_eq!(m.col, 10, "'brown' starts at char 10 of the row");
        assert_eq!(m.len, 5);
    }

    #[test]
    fn finds_needle_in_scrollback_history() {
        // 5 lines, 2-row viewport -> 3 history rows (line1..line3).
        let mut t = fresh(20, 2);
        t.vt_write(b"alpha\r\nbravo\r\ncharlie\r\ndelta\r\necho");
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 3);

        let hits = search_oneshot(&t, "bravo", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        let m = hits[0];
        assert_eq!(m.region, Region::Scrollback);
        // History rows are oldest-first: alpha(0), bravo(1), charlie(2).
        assert_eq!(m.row, 1);
        assert_eq!((m.col, m.len), (0, 5));
    }

    #[test]
    fn viewport_excluded_when_not_requested() {
        let mut t = fresh(20, 2);
        t.vt_write(b"hit-1\r\nhit-2\r\nhit-3\r\nhit-4");
        // 4 lines, 2-row viewport -> hit-1, hit-2 in scrollback; hit-3, hit-4
        // in the viewport.
        let opts = SearchOptions {
            case_insensitive: false,
            include_viewport: false,
        };
        let hits = search_oneshot(&t, "hit", Scope::AllHistory, opts).expect("search");
        assert!(
            hits.iter().all(|m| m.region == Region::Scrollback),
            "include_viewport = false must not surface viewport rows, got {hits:?}",
        );
        assert_eq!(hits.len(), 2, "two history rows carry the needle");
    }

    #[test]
    fn recent_history_bounds_the_scan() {
        let mut t = fresh(20, 2);
        // 5 lines, 2-row viewport -> 3 history rows (mark1..mark3). Bound to
        // the most-recent 1 history row (mark3) and exclude the viewport.
        t.vt_write(b"mark1\r\nmark2\r\nmark3\r\nplain4\r\nplain5");
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 3);

        let opts = SearchOptions {
            case_insensitive: false,
            include_viewport: false,
        };
        let hits = search_oneshot(&t, "mark", Scope::RecentHistory(1), opts).expect("search");
        // Only the most-recent history row (mark3) is in scope.
        assert_eq!(hits.len(), 1, "only the last history row is searched");
        assert_eq!(hits[0].region, Region::Scrollback);
    }

    #[test]
    fn case_insensitive_matches_mixed_case() {
        let mut t = fresh(40, 3);
        t.vt_write(b"ERROR: Disk Full");
        let sensitive = search_oneshot(&t, "error", Scope::AllHistory, vp()).expect("search");
        assert!(
            sensitive.is_empty(),
            "case-sensitive 'error' must not hit 'ERROR'"
        );

        let opts = SearchOptions {
            case_insensitive: true,
            include_viewport: true,
        };
        let insensitive = search_oneshot(&t, "error", Scope::AllHistory, opts).expect("search");
        assert_eq!(insensitive.len(), 1);
        assert_eq!((insensitive[0].col, insensitive[0].len), (0, 5));
    }

    #[test]
    fn reports_multiple_non_overlapping_hits_per_row() {
        let mut t = fresh(40, 2);
        t.vt_write(b"aabaab");
        let hits = search_oneshot(&t, "aa", Scope::AllHistory, vp()).expect("search");
        // "aabaab": matches at char 0 and char 3; the scan resumes past each
        // hit, so the overlapping 'a' at index 1 is not double-counted.
        let cols: Vec<usize> = hits.iter().map(|m| m.col).collect();
        assert_eq!(cols, vec![0, 3], "non-overlapping hits, got {hits:?}");
    }

    #[test]
    fn column_offsets_count_scalars_past_multibyte() {
        // A multibyte glyph before the needle must not inflate the reported
        // column: coordinates are char offsets, not byte offsets.
        let mut t = fresh(40, 2);
        // "é" is two UTF-8 bytes but one char; "TODO" then starts at char 2.
        t.vt_write("éxTODO".as_bytes());
        let hits = search_oneshot(&t, "TODO", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].col, 2, "char offset, not byte offset, got {hits:?}");
    }

    #[test]
    fn no_history_searches_viewport_only() {
        let mut t = fresh(40, 5);
        t.vt_write(b"single line, no scrollback");
        assert_eq!(t.scrollback_rows().expect("scrollback_rows"), 0);
        let hits = search_oneshot(&t, "scrollback", Scope::AllHistory, vp()).expect("search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].region, Region::Viewport);
    }

    #[test]
    fn safe_api_selection_formatter_bridge_is_broken_at_pin() {
        // Pins the upstream gap that blocks the extraction wrapper
        // (phux-3sy). The safe `libghostty_vt` crate at pin acc4b87 *does*
        // expose every type the bridge needs — `screen::Selection` has
        // public `GridRef` endpoints, `Terminal::grid_ref` yields them, and
        // `fmt::Formatter` accepts an `Option<Selection>`. But
        // `Formatter::new_inner` takes the address of a match-arm-local copy
        // of the `ffi::Selection` (`&raw const s`) that is dropped before the
        // FFI call reads through it, so the `selection: Some(..)` path passes
        // a dangling pointer and libghostty rejects it.
        //
        // This test asserts both halves: the no-selection (whole-screen)
        // path works, and the selection path fails. When a future
        // libghostty-rs pin fixes `new_inner`, the second assertion flips and
        // this test fails loudly — the signal to wire up the extraction
        // bridge (see the module note and `phux-3sy` follow-up).
        let mut t = fresh(40, 3);
        t.vt_write(b"the quick brown fox");

        // Whole-screen format: works.
        let mut whole = Formatter::new(
            &t,
            FormatterOptions {
                format: Format::Plain,
                trim: true,
                unwrap: true,
                selection: None,
            },
        )
        .expect("whole-screen formatter constructs");
        let bytes = whole.format_alloc(None).expect("whole-screen format");
        assert!(
            String::from_utf8_lossy(&bytes).contains("brown"),
            "the no-selection formatter path is functional",
        );

        // Selection-restricted format: broken at this pin. Build a linear
        // selection over "brown" (cols 10..=14 of the only viewport row).
        let selection = Selection {
            start: t
                .grid_ref(Point::Viewport(PointCoordinate { x: 10, y: 0 }))
                .expect("start grid_ref"),
            end: t
                .grid_ref(Point::Viewport(PointCoordinate { x: 14, y: 0 }))
                .expect("end grid_ref"),
            rectangle: false,
        };
        let restricted = Formatter::new(
            &t,
            FormatterOptions {
                format: Format::Plain,
                trim: true,
                unwrap: true,
                selection: Some(selection),
            },
        )
        .and_then(|mut f| f.format_alloc(None).map(|b| b.to_vec()));
        assert!(
            restricted.is_err(),
            "selection-restricted formatting is expected to fail at pin \
             acc4b87 (dangling selection pointer in Formatter::new_inner); \
             if this now succeeds, wire up the extraction bridge, got {:?}",
            restricted.map(|b| String::from_utf8_lossy(&b).into_owned()),
        );
    }
}
