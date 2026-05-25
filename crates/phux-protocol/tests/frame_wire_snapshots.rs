//! Snapshot tests for the phux-4az message-catalog wire frames.
//!
//! Each test encodes a representative fixture of an `ATTACH` / `ATTACHED` /
//! `DETACH` / `DETACHED` / `INPUT_*` / `BELL` frame, hex-dumps the bytes,
//! and compares against a committed `.snap` file under `tests/snapshots/`.
//! The wire format is a cross-implementation contract — any change MUST
//! surface as a visible diff in pull-request review.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::diff::{Cell, Color, DiffOp, PaletteIndex};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachRole, FrameKind, PaneSnapshot};

/// Render `bytes` as an `xxd`-style hex dump: 16 cols per row,
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

fn dump_frame(frame: &FrameKind) -> String {
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    hex_dump(&buf)
}

#[test]
fn snap_attach_primary() {
    let frame = FrameKind::Attach {
        session_name: "default".to_owned(),
        role: AttachRole::Primary,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_viewer_empty_name() {
    let frame = FrameKind::Attach {
        session_name: String::new(),
        role: AttachRole::Viewer,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_detach() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Detach));
}

#[test]
fn snap_input_key_letter_a_press() {
    let frame = FrameKind::InputKey {
        pane_id: 0x0000_0007,
        event: KeyEvent {
            action: KeyAction::Press,
            key: PhysicalKey::A,
            mods: ModSet::empty(),
            consumed_mods: ModSet::empty(),
            composing: false,
            text: Some("a".to_owned()),
            unshifted_codepoint: Some(u32::from('a')),
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_input_key_no_text() {
    let frame = FrameKind::InputKey {
        pane_id: 0x0000_0001,
        event: KeyEvent {
            action: KeyAction::Release,
            key: PhysicalKey::Escape,
            mods: ModSet::CTRL | ModSet::SHIFT,
            consumed_mods: ModSet::empty(),
            composing: false,
            text: None,
            unshifted_codepoint: None,
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_input_mouse_left_click() {
    let frame = FrameKind::InputMouse {
        pane_id: 0x0000_0042,
        event: MouseEvent {
            action: MouseAction::Press,
            button: MouseButton::Left,
            mods: ModSet::empty(),
            x: 120.0,
            y: 40.5,
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_input_focus_gained() {
    let frame = FrameKind::InputFocus {
        pane_id: 0x0000_0003,
        event: FocusEvent::Gained,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_input_focus_lost() {
    let frame = FrameKind::InputFocus {
        pane_id: 0x0000_0003,
        event: FocusEvent::Lost,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_input_paste_trusted_ascii() {
    let frame = FrameKind::InputPaste {
        pane_id: 0x0000_0005,
        event: PasteEvent {
            trust: PasteTrust::Trusted,
            data: b"hello world".to_vec(),
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attached_empty_snapshot() {
    let frame = FrameKind::Attached {
        session_id: 0x0000_0011,
        window_id: 0x0000_0022,
        pane_id: 0x0000_0033,
        snapshot: PaneSnapshot {
            cols: 80,
            rows: 24,
            ops: Vec::new(),
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attached_with_initial_diff() {
    let frame = FrameKind::Attached {
        session_id: 1,
        window_id: 1,
        pane_id: 1,
        snapshot: PaneSnapshot {
            cols: 4,
            rows: 1,
            ops: vec![DiffOp::CellRun {
                row: 0,
                col: 0,
                cells: vec![Cell {
                    text: vec!['H'],
                    fg: Color::Palette(PaletteIndex(2)),
                    ..Cell::blank()
                }],
            }],
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_detached() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Detached));
}

#[test]
fn snap_bell() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Bell {
        pane_id: 0x0000_00BE,
    }));
}
