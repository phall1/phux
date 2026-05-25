//! Snapshot tests for the SPEC §13-conformant wire frames.
//!
//! Each test encodes a representative fixture of an `ATTACH` / `ATTACHED` /
//! `PANE_SNAPSHOT` / `DETACH` / `DETACHED` / `INPUT_*` / `BELL` frame,
//! hex-dumps the bytes, and compares against a committed `.snap` file under
//! `tests/snapshots/`. The wire format is a cross-implementation contract —
//! any change MUST surface as a visible diff in pull-request review.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::diff::{Cell, Color, DiffOp, PaletteIndex};
use phux_protocol::ids::{ClientId, PaneId, SessionId, WindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, PaneSnapshotPayload, ViewportInfo};
use phux_protocol::wire::info::{
    LayoutNode, PaneInfo, SessionInfo, SessionSnapshot, SplitDir, WindowInfo,
};

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

// -----------------------------------------------------------------------------
// ATTACH — SPEC §13. The four AttachTarget variants plus viewport pixel-dim
// presence both ways.
// -----------------------------------------------------------------------------

const fn vp_no_pixels() -> ViewportInfo {
    ViewportInfo {
        cols: 80,
        rows: 24,
        pixel_w: None,
        pixel_h: None,
    }
}

const fn vp_with_pixels() -> ViewportInfo {
    ViewportInfo {
        cols: 80,
        rows: 24,
        pixel_w: Some(1280),
        pixel_h: Some(720),
    }
}

#[test]
fn snap_attach_target_last() {
    let frame = FrameKind::Attach {
        target: AttachTarget::Last,
        viewport: vp_no_pixels(),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_target_by_name() {
    let frame = FrameKind::Attach {
        target: AttachTarget::ByName("default".to_owned()),
        viewport: vp_no_pixels(),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_target_by_id() {
    let frame = FrameKind::Attach {
        target: AttachTarget::ById(SessionId::new(7)),
        viewport: vp_no_pixels(),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_target_create_if_missing_minimal() {
    let frame = FrameKind::Attach {
        target: AttachTarget::CreateIfMissing {
            name: "dev".to_owned(),
            command: None,
            cwd: None,
        },
        viewport: vp_no_pixels(),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_target_create_if_missing_full() {
    let frame = FrameKind::Attach {
        target: AttachTarget::CreateIfMissing {
            name: "dev".to_owned(),
            command: Some(vec!["zsh".to_owned()]),
            cwd: Some("/tmp".to_owned()),
        },
        viewport: vp_no_pixels(),
        request_scrollback: true,
        scrollback_limit_lines: 10_000,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attach_viewport_with_pixels() {
    let frame = FrameKind::Attach {
        target: AttachTarget::ByName("default".to_owned()),
        viewport: vp_with_pixels(),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

// -----------------------------------------------------------------------------
// DETACH / DETACHED — unit messages; unchanged from phux-4az.
// -----------------------------------------------------------------------------

#[test]
fn snap_detach() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Detach));
}

#[test]
fn snap_detached() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Detached));
}

// -----------------------------------------------------------------------------
// INPUT_* — unchanged from phux-4az.
// -----------------------------------------------------------------------------

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

// -----------------------------------------------------------------------------
// ATTACHED — SPEC §13 full SessionSnapshot, with a non-trivial layout tree.
// -----------------------------------------------------------------------------

#[test]
fn snap_attached_empty_graph() {
    // Single session, no windows or panes — the smallest legal ATTACHED.
    let snapshot = SessionSnapshot {
        sessions: vec![SessionInfo {
            id: SessionId::new(1),
            name: "default".to_owned(),
            active_window: None,
            created_at_unix_secs: 1_700_000_000,
            window_count: 0,
            attached_client_count: 1,
        }],
        windows: Vec::new(),
        panes: Vec::new(),
        focused_session: SessionId::new(1),
        focused_window: WindowId::new(0),
        focused_pane: PaneId::new(0),
    };
    let frame = FrameKind::Attached {
        snapshot,
        initial_client_id: ClientId::new(42),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attached_realistic_graph() {
    // 2 sessions, 3 windows, 4 panes, non-trivial layout including a Split.
    let sessions = vec![
        SessionInfo {
            id: SessionId::new(1),
            name: "work".to_owned(),
            active_window: Some(WindowId::new(10)),
            created_at_unix_secs: 1_700_000_000,
            window_count: 2,
            attached_client_count: 1,
        },
        SessionInfo {
            id: SessionId::new(2),
            name: "personal".to_owned(),
            active_window: Some(WindowId::new(30)),
            created_at_unix_secs: 1_700_000_500,
            window_count: 1,
            attached_client_count: 0,
        },
    ];

    let windows = vec![
        WindowInfo {
            id: WindowId::new(10),
            session_id: SessionId::new(1),
            index: 0,
            name: "code".to_owned(),
            active_pane: Some(PaneId::new(100)),
            layout: Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                left: Box::new(LayoutNode::Leaf(PaneId::new(100))),
                right: Box::new(LayoutNode::Leaf(PaneId::new(101))),
            }),
        },
        WindowInfo {
            id: WindowId::new(20),
            session_id: SessionId::new(1),
            index: 1,
            name: "logs".to_owned(),
            active_pane: Some(PaneId::new(102)),
            layout: Some(LayoutNode::Leaf(PaneId::new(102))),
        },
        WindowInfo {
            id: WindowId::new(30),
            session_id: SessionId::new(2),
            index: 0,
            name: "scratch".to_owned(),
            active_pane: Some(PaneId::new(103)),
            layout: Some(LayoutNode::Leaf(PaneId::new(103))),
        },
    ];

    let panes = vec![
        PaneInfo {
            id: PaneId::new(100),
            window_id: WindowId::new(10),
            cols: 80,
            rows: 24,
            title: Some("editor".to_owned()),
            cwd: Some("/home/u/src".to_owned()),
        },
        PaneInfo {
            id: PaneId::new(101),
            window_id: WindowId::new(10),
            cols: 80,
            rows: 24,
            title: None,
            cwd: Some("/home/u/src".to_owned()),
        },
        PaneInfo {
            id: PaneId::new(102),
            window_id: WindowId::new(20),
            cols: 160,
            rows: 48,
            title: None,
            cwd: None,
        },
        PaneInfo {
            id: PaneId::new(103),
            window_id: WindowId::new(30),
            cols: 80,
            rows: 24,
            title: None,
            cwd: Some("/home/u".to_owned()),
        },
    ];

    let snapshot = SessionSnapshot {
        sessions,
        windows,
        panes,
        focused_session: SessionId::new(1),
        focused_window: WindowId::new(10),
        focused_pane: PaneId::new(100),
    };
    let frame = FrameKind::Attached {
        snapshot,
        initial_client_id: ClientId::new(1),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

// -----------------------------------------------------------------------------
// PANE_SNAPSHOT — separate frame per SPEC §13 attach sequence + §16.
// -----------------------------------------------------------------------------

#[test]
fn snap_pane_snapshot_empty() {
    let frame = FrameKind::PaneSnapshot {
        pane_id: PaneId::new(100),
        snapshot: PaneSnapshotPayload {
            cols: 80,
            rows: 24,
            ops: Vec::new(),
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_pane_snapshot_with_initial_diff() {
    let frame = FrameKind::PaneSnapshot {
        pane_id: PaneId::new(100),
        snapshot: PaneSnapshotPayload {
            cols: 4,
            rows: 1,
            ops: vec![DiffOp::CellRun {
                row: 0,
                col: 0,
                cells: vec![Cell {
                    text: smallvec::smallvec!['H'],
                    fg: Color::Palette(PaletteIndex(2)),
                    ..Cell::blank()
                }],
            }],
        },
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_bell() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Bell {
        pane_id: 0x0000_00BE,
    }));
}
