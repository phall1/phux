//! Snapshot tests for the SPEC §13-conformant wire frames.
//!
//! Each test encodes a representative fixture of an `ATTACH` / `ATTACHED` /
//! `PANE_SNAPSHOT` / `PANE_OUTPUT` / `DETACH` / `DETACHED` / `INPUT_*` /
//! `BELL` frame, hex-dumps the bytes, and compares against a committed
//! `.snap` file under `tests/snapshots/`. The wire format is a
//! cross-implementation contract — any change MUST surface as a visible
//! diff in pull-request review.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::ids::{ClientId, PaneId, SessionId, WindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};
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
    ViewportInfo::new(80, 24)
}

const fn vp_with_pixels() -> ViewportInfo {
    ViewportInfo::new(80, 24).with_pixels(Some(1280), Some(720))
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
// DETACH / DETACHED — unit messages.
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
// INPUT_*.
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
    let snapshot = SessionSnapshot::new(SessionId::new(1), WindowId::new(0), PaneId::new(0))
        .with_sessions(vec![
            SessionInfo::new(SessionId::new(1), "default")
                .with_created_at_unix_secs(1_700_000_000)
                .with_attached_client_count(1),
        ]);
    let frame = FrameKind::Attached {
        snapshot,
        initial_client_id: ClientId::new(42),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_attached_realistic_graph() {
    let sessions = vec![
        SessionInfo::new(SessionId::new(1), "work")
            .with_active_window(Some(WindowId::new(10)))
            .with_created_at_unix_secs(1_700_000_000)
            .with_window_count(2)
            .with_attached_client_count(1),
        SessionInfo::new(SessionId::new(2), "personal")
            .with_active_window(Some(WindowId::new(30)))
            .with_created_at_unix_secs(1_700_000_500)
            .with_window_count(1),
    ];

    let windows = vec![
        WindowInfo::new(WindowId::new(10), SessionId::new(1), "code")
            .with_active_pane(Some(PaneId::new(100)))
            .with_layout(Some(LayoutNode::Split {
                dir: SplitDir::Horizontal,
                ratio: 0.5,
                left: Box::new(LayoutNode::Leaf(PaneId::new(100))),
                right: Box::new(LayoutNode::Leaf(PaneId::new(101))),
            })),
        WindowInfo::new(WindowId::new(20), SessionId::new(1), "logs")
            .with_index(1)
            .with_active_pane(Some(PaneId::new(102)))
            .with_layout(Some(LayoutNode::Leaf(PaneId::new(102)))),
        WindowInfo::new(WindowId::new(30), SessionId::new(2), "scratch")
            .with_active_pane(Some(PaneId::new(103)))
            .with_layout(Some(LayoutNode::Leaf(PaneId::new(103)))),
    ];

    let panes = vec![
        PaneInfo::new(PaneId::new(100), WindowId::new(10), 80, 24)
            .with_title(Some("editor".to_owned()))
            .with_cwd(Some("/home/u/src".to_owned())),
        PaneInfo::new(PaneId::new(101), WindowId::new(10), 80, 24)
            .with_cwd(Some("/home/u/src".to_owned())),
        PaneInfo::new(PaneId::new(102), WindowId::new(20), 160, 48),
        PaneInfo::new(PaneId::new(103), WindowId::new(30), 80, 24)
            .with_cwd(Some("/home/u".to_owned())),
    ];

    let snapshot = SessionSnapshot::new(SessionId::new(1), WindowId::new(10), PaneId::new(100))
        .with_sessions(sessions)
        .with_windows(windows)
        .with_panes(panes);
    let frame = FrameKind::Attached {
        snapshot,
        initial_client_id: ClientId::new(1),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

// -----------------------------------------------------------------------------
// PANE_OUTPUT (SPEC §8.1, ADR-0013) — hot-path bytes-on-wire.
// -----------------------------------------------------------------------------

#[test]
fn snap_pane_output_hello_world() {
    // A representative PANE_OUTPUT carrying ASCII bytes: "hello world\r\n".
    let frame = FrameKind::PaneOutput {
        pane_id: 1,
        seq: 0,
        bytes: b"hello world\r\n".to_vec(),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_pane_output_empty_bytes() {
    let frame = FrameKind::PaneOutput {
        pane_id: 0x0000_002A,
        seq: 1,
        bytes: Vec::new(),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_pane_output_with_sgr() {
    // A short bold-red sequence: validates the wire envelope is bytes-
    // transparent — the SGR is opaque to the protocol.
    let frame = FrameKind::PaneOutput {
        pane_id: 7,
        seq: 42,
        bytes: b"\x1b[1;31mERR\x1b[0m".to_vec(),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

// -----------------------------------------------------------------------------
// PANE_SNAPSHOT — SPEC §8.4, ADR-0013. vt_replay_bytes body shape.
// -----------------------------------------------------------------------------

#[test]
fn snap_pane_snapshot_empty_vt() {
    let frame = FrameKind::PaneSnapshot {
        pane_id: PaneId::new(100),
        cols: 80,
        rows: 24,
        vt_replay_bytes: Vec::new(),
        scrollback_bytes: None,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_pane_snapshot_minimal_replay() {
    // Reset + CUP home + a single ASCII char + cursor placement.
    let frame = FrameKind::PaneSnapshot {
        pane_id: PaneId::new(100),
        cols: 80,
        rows: 24,
        vt_replay_bytes: b"\x1b[!p\x1b[2J\x1b[HH\x1b[1;2H".to_vec(),
        scrollback_bytes: None,
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_pane_snapshot_with_scrollback() {
    let frame = FrameKind::PaneSnapshot {
        pane_id: PaneId::new(100),
        cols: 80,
        rows: 24,
        vt_replay_bytes: b"\x1b[!p\x1b[2J\x1b[H".to_vec(),
        scrollback_bytes: Some(b"prior line one\r\nprior line two\r\n".to_vec()),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_bell() {
    insta::assert_snapshot!(dump_frame(&FrameKind::Bell {
        pane_id: 0x0000_00BE,
    }));
}

// -----------------------------------------------------------------------------
// VIEWPORT_RESIZE — SPEC §10.5. Cell-only and pixel-augmented viewports.
// -----------------------------------------------------------------------------

#[test]
fn snap_viewport_resize_cells_only() {
    let frame = FrameKind::ViewportResize {
        viewport: ViewportInfo::new(120, 40),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}

#[test]
fn snap_viewport_resize_with_pixels() {
    let frame = FrameKind::ViewportResize {
        viewport: ViewportInfo::new(120, 40).with_pixels(Some(1920), Some(1080)),
    };
    insta::assert_snapshot!(dump_frame(&frame));
}
