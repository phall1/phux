//! Replay-invariant tests for `phux_client::mirror::DiffMirror`.
//!
//! Mirrors the server-side replay assertion from
//! `crates/phux-server/examples/diff_spike.rs`: applying the diff that
//! `compute_diff(G0, G1)` produces to a [`DiffMirror`] initialised at `G0`
//! yields a grid byte-identical to `G1`.

#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests assert via panic")]

use phux_client::DiffMirror;
use phux_protocol::{
    Cell, CellFlags, Color, CursorShape, CursorState, DiffOp, Grid, Underline, compute_diff,
    diff::{PaletteIndex, RgbColor},
};

fn cell_with_text(s: &str) -> Cell {
    Cell {
        text: s.chars().collect(),
        ..Cell::blank()
    }
}

fn cell_with_text_and_fg(s: &str, fg: Color) -> Cell {
    Cell {
        text: s.chars().collect(),
        fg,
        ..Cell::blank()
    }
}

#[test]
fn cell_run_writes_cells_at_offset() {
    let mut mirror = DiffMirror::new(3, 10);
    let cells = vec![cell_with_text("h"), cell_with_text("i")];
    mirror.apply(&[DiffOp::CellRun {
        row: 1,
        col: 3,
        cells: cells.clone(),
    }]);
    assert_eq!(mirror.grid.cells[1][3], cells[0]);
    assert_eq!(mirror.grid.cells[1][4], cells[1]);
    // Surrounding cells stay blank.
    assert!(mirror.grid.cells[1][2].is_blank());
    assert!(mirror.grid.cells[1][5].is_blank());
    assert!(mirror.grid.cells[0].iter().all(Cell::is_blank));
}

#[test]
fn clear_blanks_a_run_of_populated_cells() {
    let mut mirror = DiffMirror::new(1, 6);
    // Pre-populate via CellRun.
    let cells: Vec<Cell> = "abcdef"
        .chars()
        .map(|c| cell_with_text(&c.to_string()))
        .collect();
    mirror.apply(&[DiffOp::CellRun {
        row: 0,
        col: 0,
        cells,
    }]);
    assert_eq!(&mirror.grid.cells[0][2].text[..], &['c'][..]);

    mirror.apply(&[DiffOp::Clear {
        row: 0,
        col: 2,
        count: 3,
    }]);
    assert_eq!(&mirror.grid.cells[0][0].text[..], &['a'][..]);
    assert_eq!(&mirror.grid.cells[0][1].text[..], &['b'][..]);
    assert!(mirror.grid.cells[0][2].is_blank());
    assert!(mirror.grid.cells[0][3].is_blank());
    assert!(mirror.grid.cells[0][4].is_blank());
    assert_eq!(&mirror.grid.cells[0][5].text[..], &['f'][..]);
}

#[test]
fn cursor_move_updates_cursor_position() {
    let mut mirror = DiffMirror::new(5, 5);
    assert_eq!(mirror.cursor.row, 0);
    assert_eq!(mirror.cursor.col, 0);
    mirror.apply(&[DiffOp::CursorMove { row: 3, col: 4 }]);
    assert_eq!(mirror.cursor.row, 3);
    assert_eq!(mirror.cursor.col, 4);
    // Grid's embedded cursor mirrors the field.
    assert_eq!(mirror.grid.cursor.row, 3);
    assert_eq!(mirror.grid.cursor.col, 4);
}

#[test]
fn cursor_style_propagates_to_cursor_state() {
    let mut mirror = DiffMirror::new(2, 2);
    mirror.apply(&[DiffOp::CursorStyle {
        visible: false,
        shape: CursorShape::Bar,
        blink: false,
    }]);
    assert!(!mirror.cursor.visible);
    assert_eq!(mirror.cursor.shape, CursorShape::Bar);
    assert!(!mirror.cursor.blink);
    assert!(!mirror.grid.cursor.visible);
    assert_eq!(mirror.grid.cursor.shape, CursorShape::Bar);
    assert!(!mirror.grid.cursor.blink);
}

#[test]
fn ingest_snapshot_replaces_full_state() {
    let mut mirror = DiffMirror::new(2, 3);
    mirror.apply(&[DiffOp::CellRun {
        row: 0,
        col: 0,
        cells: vec![cell_with_text("x")],
    }]);

    let mut snap = Grid::blank(4, 5);
    snap.cells[2][1] = cell_with_text("y");
    snap.cursor = CursorState {
        row: 1,
        col: 2,
        visible: false,
        shape: CursorShape::Underline,
        blink: false,
    };
    mirror.ingest_snapshot(&snap, 42);
    assert_eq!(mirror.grid, snap);
    assert_eq!(mirror.cursor, snap.cursor);
    assert_eq!(mirror.frame_id, 42);
}

#[test]
fn replay_invariant_hand_coded() {
    // Build G1 by hand: row 0 = "hello, world!" with the "world" run styled
    // bold + green RGB; row 1 = "second line" red; cursor at row 2 col 0.
    // This mirrors what diff_spike feeds into libghostty but stays purely
    // protocol-side so the test doesn't need a Terminal.
    let mut g1 = Grid::blank(6, 40);

    let plain = |c: char| cell_with_text(&c.to_string());
    let styled = |c: char, fg: Color, bold: bool| {
        let mut cell = cell_with_text(&c.to_string());
        cell.fg = fg;
        if bold {
            cell.flags |= CellFlags::BOLD;
        }
        cell
    };

    let green = Color::Rgb(RgbColor { r: 0, g: 255, b: 0 });
    let red = Color::Rgb(RgbColor { r: 255, g: 0, b: 0 });

    // "hello, " — plain
    for (i, ch) in "hello, ".chars().enumerate() {
        g1.cells[0][i] = plain(ch);
    }
    // "world" — bold green
    for (i, ch) in "world".chars().enumerate() {
        g1.cells[0][7 + i] = styled(ch, green, true);
    }
    // "!" — plain
    g1.cells[0][12] = plain('!');

    // Row 1: "second line" red
    for (i, ch) in "second line".chars().enumerate() {
        g1.cells[1][i] = styled(ch, red, false);
    }

    g1.cursor = CursorState {
        row: 2,
        col: 0,
        visible: true,
        shape: CursorShape::Block,
        blink: true,
    };

    let g0 = Grid::blank(6, 40);
    let ops = compute_diff(&g0, &g1);

    // Sanity: the diff is non-empty and contains at least a CellRun and a
    // CursorMove, matching the diff_spike shape.
    assert!(!ops.is_empty());
    assert!(
        ops.iter().any(|op| matches!(op, DiffOp::CellRun { .. })),
        "expected at least one CellRun in {ops:?}",
    );
    assert!(
        ops.iter().any(|op| matches!(op, DiffOp::CursorMove { .. })),
        "expected a CursorMove in {ops:?}",
    );

    let mut mirror = DiffMirror::new(6, 40);
    mirror.apply(&ops);

    // Byte-identical reproduction — this is the protocol invariant.
    assert_eq!(mirror.grid, g1, "client mirror did not reproduce G1");
}

#[test]
fn replay_invariant_with_clear_op() {
    // G0 has a populated row, G1 has it blanked — exercises the Clear path
    // in the round trip.
    let mut g0 = Grid::blank(2, 8);
    for i in 0..8 {
        g0.cells[0][i] = cell_with_text_and_fg(
            "x",
            Color::Rgb(RgbColor {
                r: 10,
                g: 20,
                b: 30,
            }),
        );
    }
    let g1 = Grid::blank(2, 8);

    let ops = compute_diff(&g0, &g1);
    assert!(
        ops.iter().any(|op| matches!(op, DiffOp::Clear { .. })),
        "expected a Clear op in {ops:?}",
    );

    let mut mirror = DiffMirror::new(2, 8);
    mirror.ingest_snapshot(&g0, 0);
    mirror.apply(&ops);
    assert_eq!(mirror.grid, g1);
}

#[test]
fn underline_and_flags_roundtrip_through_replay() {
    let mut g1 = Grid::blank(1, 4);
    g1.cells[0][0] = Cell {
        text: smallvec::smallvec!['u'],
        fg: Color::None,
        bg: Color::Rgb(RgbColor { r: 9, g: 9, b: 9 }),
        underline: Underline::Curly,
        underline_color: Color::Palette(PaletteIndex(7)),
        flags: CellFlags::ITALIC | CellFlags::STRIKETHROUGH,
    };

    let g0 = Grid::blank(1, 4);
    let ops = compute_diff(&g0, &g1);

    let mut mirror = DiffMirror::new(1, 4);
    mirror.apply(&ops);
    assert_eq!(mirror.grid, g1);
}
