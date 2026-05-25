//! Snapshot tests for the diff wire codec (phux-6yl.5).
//!
//! Each test encodes a fixture, hex-dumps the bytes, and compares against a
//! committed `.snap` file under `tests/snapshots/`. The wire format is a
//! cross-implementation contract: any change MUST surface as a visible diff
//! in pull-request review.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::diff::{Cell, CellFlags, Color, CursorShape, DiffOp, Underline};
use phux_protocol::wire::diff::encode_diff_ops;
use phux_protocol::wire::encode::Encoder;
use phux_protocol::wire::frame::FrameKind;

/// Render `bytes` in a `xxd`-style hex dump: 16 columns per row, formatted as
/// `OFFSET | HEX HEX HEX ... | ASCII`.
fn hex_dump(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::new();
    if bytes.is_empty() {
        out.push_str("(empty)\n");
        return out;
    }

    for (chunk_idx, chunk) in bytes.chunks(16).enumerate() {
        let offset = chunk_idx * 16;
        let _ = write!(out, "{offset:08x} |");
        for (i, b) in chunk.iter().enumerate() {
            if i == 8 {
                out.push(' ');
            }
            let _ = write!(out, " {b:02x}");
        }
        // Pad short last row so the ASCII column lines up.
        let pad_cells = 16 - chunk.len();
        for i in 0..pad_cells {
            if chunk.len() + i == 8 {
                out.push(' ');
            }
            out.push_str("   ");
        }
        out.push_str(" |");
        for b in chunk {
            let c = if (0x20..=0x7e).contains(b) {
                *b as char
            } else {
                '.'
            };
            out.push(c);
        }
        out.push('\n');
    }
    out
}

/// Encode `ops` with [`encode_diff_ops`] and return its hex dump.
fn dump_ops(ops: &[DiffOp]) -> String {
    let mut buf = BytesMut::new();
    {
        let mut enc = Encoder::new(&mut buf);
        encode_diff_ops(ops, &mut enc);
    }
    hex_dump(&buf)
}

#[test]
fn snap_empty_ops() {
    insta::assert_snapshot!(dump_ops(&[]));
}

#[test]
fn snap_single_ascii_cell() {
    let ops = vec![DiffOp::CellRun {
        row: 0,
        col: 0,
        cells: vec![Cell {
            text: vec!['A'],
            ..Cell::blank()
        }],
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_mixed_color_cells() {
    let mk = |fg, bg, ch: char| Cell {
        text: vec![ch],
        fg,
        bg,
        ..Cell::blank()
    };
    let ops = vec![DiffOp::CellRun {
        row: 3,
        col: 7,
        cells: vec![
            mk(Color::Default, Color::Default, 'a'),
            mk(Color::Indexed(7), Color::Default, 'b'),
            mk(Color::Default, Color::Indexed(4), 'c'),
            mk(Color::Rgb(0xff, 0x80, 0x00), Color::Default, 'd'),
            mk(Color::Rgb(0x12, 0x34, 0x56), Color::Indexed(2), 'e'),
        ],
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_bold_italic_flags() {
    let ops = vec![DiffOp::CellRun {
        row: 0,
        col: 0,
        cells: vec![Cell {
            text: vec!['X'],
            flags: CellFlags::BOLD | CellFlags::ITALIC,
            ..Cell::blank()
        }],
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_curly_rgb_underline() {
    let ops = vec![DiffOp::CellRun {
        row: 1,
        col: 2,
        cells: vec![Cell {
            text: vec!['u'],
            underline: Underline::Curly,
            underline_color: Color::Rgb(0xaa, 0xbb, 0xcc),
            ..Cell::blank()
        }],
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_clear_midrow() {
    let ops = vec![DiffOp::Clear {
        row: 5,
        col: 20,
        count: 10,
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_cursor_move() {
    let ops = vec![DiffOp::CursorMove { row: 9, col: 41 }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_cursor_style_block_no_blink() {
    let ops = vec![DiffOp::CursorStyle {
        visible: true,
        shape: CursorShape::Block,
        blink: false,
    }];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_mixed_op_sequence() {
    let ops = vec![
        DiffOp::CellRun {
            row: 0,
            col: 0,
            cells: vec![
                Cell {
                    text: vec!['h'],
                    ..Cell::blank()
                },
                Cell {
                    text: vec!['i'],
                    ..Cell::blank()
                },
            ],
        },
        DiffOp::CursorMove { row: 0, col: 2 },
        DiffOp::Clear {
            row: 1,
            col: 0,
            count: 80,
        },
    ];
    insta::assert_snapshot!(dump_ops(&ops));
}

#[test]
fn snap_full_pane_diff_frame() {
    let frame = FrameKind::PaneDiff {
        pane_id: 0x0000_0042,
        frame_id: 0x0000_0000_DEAD_BEEF,
        ops: vec![
            DiffOp::CellRun {
                row: 0,
                col: 0,
                cells: vec![Cell {
                    text: vec!['O', 'K'].into_iter().take(1).collect(),
                    fg: Color::Indexed(2),
                    ..Cell::blank()
                }],
            },
            DiffOp::CursorMove { row: 0, col: 1 },
        ],
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    insta::assert_snapshot!(hex_dump(&buf));
}
