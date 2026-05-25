//! `diff_spike` — feed bytes into libghostty-vt, capture grids, compute diffs.
//!
//! Validates the architectural bet behind ADR-0002 and ADR-0004 with running
//! code, end-to-end:
//!
//! 1. Spin up a `libghostty_vt::Terminal`.
//! 2. Capture an initial blank grid.
//! 3. Feed it VT-encoded bytes.
//! 4. Capture a new grid.
//! 5. Compute `Vec<DiffOp>` and print a summary.
//!
//! No PTY, no IPC, no async. The point is to confirm the per-frame seam —
//! `Terminal → RenderState → phux_protocol::Grid → DiffOp[]` — works at
//! the level of correctness we need before any of the multiplexer code
//! lands.
//!
//! Run from inside the nix dev shell:
//!
//!     cd ~/workspace/phux && nix develop
//!     cargo run -p phux-server --example diff_spike

#![allow(clippy::print_stdout, reason = "spike binary — printing is the output")]
#![allow(clippy::expect_used, reason = "spike binary — fail loudly")]

use libghostty_vt::{Terminal, TerminalOptions};
use phux_protocol::{DiffOp, compute_diff};
use phux_server::grid;

fn main() {
    let mut terminal = Terminal::new(TerminalOptions {
        cols: 40,
        rows: 6,
        max_scrollback: 10_000,
    })
    .expect("Terminal::new");

    let snap0 = grid::capture(&terminal).expect("capture 0");
    println!("=== snapshot 0 (empty) ===");
    print_grid(&snap0);

    println!("\n>> feeding: \"hello, \\x1b[1;32mworld\\x1b[0m!\\r\\n\"");
    terminal.vt_write(b"hello, \x1b[1;32mworld\x1b[0m!\r\n");
    let snap1 = grid::capture(&terminal).expect("capture 1");
    println!("\n=== snapshot 1 (after hello) ===");
    print_grid(&snap1);

    println!("\n=== diff 0 -> 1 ===");
    let diff_0_1 = compute_diff(&snap0, &snap1);
    print_diff(&diff_0_1);

    println!("\n>> feeding: \"\\x1b[31msecond line\\x1b[0m\\r\\n\"");
    terminal.vt_write(b"\x1b[31msecond line\x1b[0m\r\n");
    let snap2 = grid::capture(&terminal).expect("capture 2");
    println!("\n=== snapshot 2 (after second line) ===");
    print_grid(&snap2);

    println!("\n=== diff 1 -> 2 ===");
    let diff_1_2 = compute_diff(&snap1, &snap2);
    print_diff(&diff_1_2);

    println!("\n>> feeding: clear screen + reposition");
    terminal.vt_write(b"\x1b[H\x1b[2J");
    let snap3 = grid::capture(&terminal).expect("capture 3");
    println!("\n=== snapshot 3 (cleared) ===");
    print_grid(&snap3);

    println!("\n=== diff 2 -> 3 ===");
    let diff_2_3 = compute_diff(&snap2, &snap3);
    print_diff(&diff_2_3);

    println!("\n=== summary ===");
    println!("0→1: {} ops", diff_0_1.len());
    println!("1→2: {} ops", diff_1_2.len());
    println!("2→3: {} ops", diff_2_3.len());

    // Sanity: applying 0→1 to snap0 should produce snap1 (cell-equivalence).
    let mut replay = snap0;
    apply_diff(&mut replay, &diff_0_1);
    assert_eq!(
        replay.cells, snap1.cells,
        "diff 0→1 did not reproduce snapshot 1; protocol invariant violated",
    );
    println!("\nreplay invariant: snap0 + diff(0→1) == snap1   ✓");
}

fn print_grid(g: &phux_protocol::Grid) {
    println!(
        "  dims: {}x{}  cursor: ({},{})",
        g.cols, g.rows, g.cursor.col, g.cursor.row
    );
    for (i, row) in g.cells.iter().enumerate() {
        let text: String = row
            .iter()
            .map(|c| c.text.first().copied().unwrap_or(' '))
            .collect::<String>()
            .trim_end()
            .to_string();
        println!("  {i:2} | {text}");
    }
}

fn print_diff(ops: &[DiffOp]) {
    if ops.is_empty() {
        println!("  (no changes)");
        return;
    }
    for op in ops {
        match op {
            DiffOp::CellRun { row, col, cells } => {
                let text: String = cells
                    .iter()
                    .map(|c| c.text.first().copied().unwrap_or(' '))
                    .collect();
                println!("  CellRun  ({row:2},{col:2}) [{}] \"{text}\"", cells.len());
            }
            DiffOp::Clear { row, col, count } => {
                println!("  Clear    ({row:2},{col:2}) count={count}");
            }
            DiffOp::CursorMove { row, col } => {
                println!("  CursorMove -> ({row},{col})");
            }
            DiffOp::CursorStyle {
                visible,
                shape,
                blink,
            } => {
                println!("  CursorStyle visible={visible} shape={shape:?} blink={blink}");
            }
        }
    }
}

/// Apply a diff in-place. Used to validate the round-trip invariant.
fn apply_diff(grid: &mut phux_protocol::Grid, ops: &[DiffOp]) {
    for op in ops {
        match op {
            DiffOp::CellRun { row, col, cells } => {
                let row = usize::from(*row);
                let col = usize::from(*col);
                for (i, cell) in cells.iter().enumerate() {
                    if let Some(target) = grid.cells.get_mut(row).and_then(|r| r.get_mut(col + i)) {
                        *target = cell.clone();
                    }
                }
            }
            DiffOp::Clear { row, col, count } => {
                let row = usize::from(*row);
                let col_start = usize::from(*col);
                if let Some(r) = grid.cells.get_mut(row) {
                    for c in r.iter_mut().skip(col_start).take(usize::from(*count)) {
                        *c = phux_protocol::Cell::blank();
                    }
                }
            }
            DiffOp::CursorMove { row, col } => {
                grid.cursor.row = *row;
                grid.cursor.col = *col;
            }
            DiffOp::CursorStyle {
                visible,
                shape,
                blink,
            } => {
                grid.cursor.visible = *visible;
                grid.cursor.shape = *shape;
                grid.cursor.blink = *blink;
            }
        }
    }
}
