//! Wire-codec round-trip and malformed-input tests.
//!
//! Proptest exercises the encoder and decoder on arbitrary `FrameKind`
//! values. Hand-rolled cases cover known-bad inputs and confirm the decoder
//! returns `DecodeError` rather than panicking.
//!
//! Under ADR-0013 the structured-diff codec is gone; the strategies here
//! cover `TerminalOutput` (raw VT bytes) and the new `TerminalSnapshot` (bytes
//! body) in place of the deleted `PaneDiff` strategies.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::ids::{ClientId, SessionId, TerminalId, WindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachTarget, ErrorCode, ViewportInfo};
use phux_protocol::wire::info::{
    LayoutNode, SessionInfo, SessionSnapshot, SplitDir, TerminalInfo, WindowInfo,
};
use phux_protocol::wire::{DecodeError, decode::Decoder, frame::FrameKind};
use proptest::prelude::*;

// -----------------------------------------------------------------------------
// Strategies
// -----------------------------------------------------------------------------

fn arb_attach_target() -> impl Strategy<Value = AttachTarget> {
    prop_oneof![
        Just(AttachTarget::Last),
        ".{0,64}".prop_map(AttachTarget::ByName),
        any::<u32>().prop_map(|id| AttachTarget::ById(SessionId::new(id))),
        (
            ".{0,32}",
            proptest::option::of(proptest::collection::vec(".{0,16}", 0..4)),
            proptest::option::of(".{0,32}"),
        )
            .prop_map(|(name, command, cwd)| AttachTarget::CreateIfMissing {
                name,
                command,
                cwd,
            }),
    ]
}

fn arb_viewport_info() -> impl Strategy<Value = ViewportInfo> {
    (
        any::<u16>(),
        any::<u16>(),
        proptest::option::of(any::<u16>()),
        proptest::option::of(any::<u16>()),
    )
        .prop_map(|(cols, rows, pixel_w, pixel_h)| {
            ViewportInfo::new(cols, rows).with_pixels(pixel_w, pixel_h)
        })
}

fn arb_split_dir() -> impl Strategy<Value = SplitDir> {
    prop_oneof![Just(SplitDir::Horizontal), Just(SplitDir::Vertical)]
}

/// Bounded recursion: at most depth 4 keeps prop-test work tractable while
/// still exercising recursive split-tree encoding/decoding.
fn arb_layout_node() -> impl Strategy<Value = LayoutNode> {
    let leaf = any::<u32>().prop_map(|id| LayoutNode::Leaf(TerminalId::local(id)));
    leaf.prop_recursive(4, 32, 2, |inner| {
        (arb_split_dir(), 0.0001f32..0.9999f32, inner.clone(), inner).prop_map(
            |(dir, ratio, left, right)| LayoutNode::Split {
                dir,
                ratio,
                left: Box::new(left),
                right: Box::new(right),
            },
        )
    })
}

fn arb_session_info() -> impl Strategy<Value = SessionInfo> {
    (
        any::<u32>(),
        ".{0,32}",
        proptest::option::of(any::<u32>()),
        any::<i64>(),
        any::<u16>(),
        any::<u16>(),
    )
        .prop_map(
            |(
                id,
                name,
                active_window,
                created_at_unix_secs,
                window_count,
                attached_client_count,
            )| {
                SessionInfo::new(SessionId::new(id), name)
                    .with_active_window(active_window.map(WindowId::new))
                    .with_created_at_unix_secs(created_at_unix_secs)
                    .with_window_count(window_count)
                    .with_attached_client_count(attached_client_count)
            },
        )
}

fn arb_window_info() -> impl Strategy<Value = WindowInfo> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u16>(),
        ".{0,32}",
        proptest::option::of(any::<u32>()),
        proptest::option::of(arb_layout_node()),
    )
        .prop_map(|(id, session_id, index, name, active_pane, layout)| {
            WindowInfo::new(WindowId::new(id), SessionId::new(session_id), name)
                .with_index(index)
                .with_active_pane(active_pane.map(TerminalId::local))
                .with_layout(layout)
        })
}

fn arb_pane_info() -> impl Strategy<Value = TerminalInfo> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u16>(),
        any::<u16>(),
        proptest::option::of(".{0,32}"),
        proptest::option::of(".{0,32}"),
    )
        .prop_map(|(id, window_id, cols, rows, title, cwd)| {
            TerminalInfo::new(TerminalId::local(id), WindowId::new(window_id), cols, rows)
                .with_title(title)
                .with_cwd(cwd)
        })
}

fn arb_session_snapshot() -> impl Strategy<Value = SessionSnapshot> {
    (
        proptest::collection::vec(arb_session_info(), 0..3),
        proptest::collection::vec(arb_window_info(), 0..4),
        proptest::collection::vec(arb_pane_info(), 0..5),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
    )
        .prop_map(|(sessions, windows, panes, fs, fw, fp)| {
            SessionSnapshot::new(SessionId::new(fs), WindowId::new(fw), TerminalId::new(fp))
                .with_sessions(sessions)
                .with_windows(windows)
                .with_panes(panes)
        })
}

/// Strategy producing one of the simple-payload `FrameKind` variants. The
/// structured variants (`ATTACH`, `ATTACHED`, `TERMINAL_SNAPSHOT`, `TERMINAL_OUTPUT`,
/// input frames) have dedicated proptests below.
fn arb_frame_kind() -> impl Strategy<Value = FrameKind> {
    prop_oneof![
        (".{0,128}", any::<u16>(), any::<u16>(), any::<u16>(),).prop_map(
            |(client_name, major, minor, patch)| FrameKind::Hello {
                client_name,
                protocol_major: major,
                protocol_minor: minor,
                protocol_patch: patch,
            },
        ),
        any::<u64>().prop_map(|nonce| FrameKind::Ping { nonce }),
        Just(FrameKind::Detach),
        Just(FrameKind::Detached),
        arb_terminal_id().prop_map(|terminal_id| FrameKind::Bell { terminal_id }),
    ]
}

/// Strategy producing both `Local` and `Satellite` variants of [`TerminalId`].
/// v0.1 servers only emit `Local`, but v0.1 decoders MUST round-trip both
/// shapes (the dispatch layer is what rejects `Satellite` ids with
/// `UnsupportedSatelliteRoute`).
fn arb_terminal_id() -> impl Strategy<Value = TerminalId> {
    prop_oneof![
        any::<u32>().prop_map(TerminalId::local),
        (".{0,32}", any::<u32>()).prop_map(|(host, id)| TerminalId::satellite(host, id)),
    ]
}

fn arb_focus_event() -> impl Strategy<Value = FocusEvent> {
    prop_oneof![Just(FocusEvent::Gained), Just(FocusEvent::Lost)]
}

fn arb_key_action() -> impl Strategy<Value = KeyAction> {
    prop_oneof![
        Just(KeyAction::Press),
        Just(KeyAction::Release),
        Just(KeyAction::Repeat),
    ]
}

fn arb_physical_key() -> impl Strategy<Value = PhysicalKey> {
    prop_oneof![
        Just(PhysicalKey::Unidentified),
        Just(PhysicalKey::A),
        Just(PhysicalKey::Enter),
        Just(PhysicalKey::Escape),
        Just(PhysicalKey::ArrowUp),
        Just(PhysicalKey::F1),
        Just(PhysicalKey::Numpad7),
    ]
}

fn arb_mod_set() -> impl Strategy<Value = ModSet> {
    any::<u16>().prop_map(|bits| ModSet::from_bits_truncate(bits & ModSet::all().bits()))
}

fn arb_key_event() -> impl Strategy<Value = KeyEvent> {
    (
        arb_key_action(),
        arb_physical_key(),
        arb_mod_set(),
        arb_mod_set(),
        any::<bool>(),
        proptest::option::of(prop::string::string_regex("[a-zA-Z0-9 ]{0,8}").unwrap()),
        proptest::option::of(any::<u32>()),
    )
        .prop_map(
            |(action, key, mods, consumed_mods, composing, text, unshifted_codepoint)| KeyEvent {
                action,
                key,
                mods,
                consumed_mods,
                composing,
                text,
                unshifted_codepoint,
            },
        )
}

fn arb_mouse_action() -> impl Strategy<Value = MouseAction> {
    prop_oneof![
        Just(MouseAction::Press),
        Just(MouseAction::Release),
        Just(MouseAction::Motion),
    ]
}

fn arb_mouse_button() -> impl Strategy<Value = MouseButton> {
    prop_oneof![
        Just(MouseButton::Unknown),
        Just(MouseButton::Left),
        Just(MouseButton::Right),
        Just(MouseButton::Middle),
        Just(MouseButton::Four),
        Just(MouseButton::Eleven),
    ]
}

fn arb_mouse_event() -> impl Strategy<Value = MouseEvent> {
    (
        arb_mouse_action(),
        arb_mouse_button(),
        arb_mod_set(),
        prop::num::f64::NORMAL | prop::num::f64::ZERO | prop::num::f64::SUBNORMAL,
        prop::num::f64::NORMAL | prop::num::f64::ZERO | prop::num::f64::SUBNORMAL,
    )
        .prop_map(|(action, button, mods, x, y)| MouseEvent {
            action,
            button,
            mods,
            x,
            y,
        })
}

/// One of every `ErrorCode` known to SPEC §14. The decoder must round-trip
/// every wire value defined by the spec.
fn arb_error_code() -> impl Strategy<Value = ErrorCode> {
    prop_oneof![
        Just(ErrorCode::VersionIncompatible),
        Just(ErrorCode::UnknownMessageType),
        Just(ErrorCode::MalformedMessage),
        Just(ErrorCode::FrameTooLarge),
        Just(ErrorCode::NotAttached),
        Just(ErrorCode::AlreadyAttached),
        Just(ErrorCode::SessionNotFound),
        Just(ErrorCode::WindowNotFound),
        Just(ErrorCode::TerminalNotFound),
        Just(ErrorCode::ClientNotFound),
        Just(ErrorCode::UnsupportedSatelliteRoute),
        Just(ErrorCode::InvalidCommand),
        Just(ErrorCode::PermissionDenied),
        Just(ErrorCode::ResourceExhausted),
        Just(ErrorCode::InternalError),
    ]
}

fn arb_paste_event() -> impl Strategy<Value = PasteEvent> {
    (
        prop_oneof![Just(PasteTrust::Trusted), Just(PasteTrust::Untrusted)],
        proptest::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(|(trust, data)| PasteEvent { trust, data })
}

/// VT byte stream, capped at 4 KiB for test speed. Empty payloads are
/// legal — `TERMINAL_OUTPUT` carries whatever the PTY produced, including
/// zero bytes (which the rate-limiter just won't emit, but the codec
/// must round-trip).
fn arb_vt_bytes() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec(any::<u8>(), 0..4096)
}

proptest! {
    /// Encoding then decoding any supported `FrameKind` is the identity.
    #[test]
    fn roundtrip_frame_kind(frame in arb_frame_kind()) {
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = Decoder::new(&buf).read_frame().unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    /// Decoding never panics on arbitrary byte input. The result is either
    /// a successful parse (rare but possible by luck) or a `DecodeError`.
    #[test]
    fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = Decoder::new(&bytes).read_frame();
    }

    #[test]
    fn roundtrip_pane_output(
        terminal_id in arb_terminal_id(),
        seq in any::<u64>(),
        bytes in arb_vt_bytes(),
    ) {
        let frame = FrameKind::TerminalOutput { terminal_id, seq, bytes };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_pane_snapshot(
        terminal_id in arb_terminal_id(),
        cols in any::<u16>(),
        rows in any::<u16>(),
        vt_replay_bytes in arb_vt_bytes(),
        scrollback_bytes in proptest::option::of(arb_vt_bytes()),
    ) {
        let frame = FrameKind::TerminalSnapshot {
            terminal_id,
            cols,
            rows,
            vt_replay_bytes,
            scrollback_bytes,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }
}

#[test]
fn hello_round_trip_minimal() {
    let frame = FrameKind::Hello {
        client_name: "phux-client".to_owned(),
        protocol_major: 0,
        protocol_minor: 1,
        protocol_patch: 0,
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn ping_round_trip() {
    let frame = FrameKind::Ping {
        nonce: 0xDEAD_BEEF_CAFE_F00D,
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, _) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
}

#[test]
fn pane_output_round_trip_hello_world() {
    let frame = FrameKind::TerminalOutput {
        terminal_id: TerminalId::local(1),
        seq: 0,
        bytes: b"hello world\r\n".to_vec(),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn pane_snapshot_round_trip_minimal() {
    let frame = FrameKind::TerminalSnapshot {
        terminal_id: TerminalId::new(100),
        cols: 80,
        rows: 24,
        vt_replay_bytes: b"\x1b[!p\x1b[2J\x1b[H".to_vec(),
        scrollback_bytes: None,
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn pane_snapshot_round_trip_with_scrollback() {
    let frame = FrameKind::TerminalSnapshot {
        terminal_id: TerminalId::new(100),
        cols: 80,
        rows: 24,
        vt_replay_bytes: b"vt".to_vec(),
        scrollback_bytes: Some(b"sb".to_vec()),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn truncated_length_header_is_eof() {
    let bytes = [0u8, 0, 0];
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnexpectedEof);
}

#[test]
fn zero_length_is_rejected() {
    let bytes = [0u8, 0, 0, 0];
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::LengthOverflow);
}

#[test]
fn length_exceeds_protocol_cap() {
    let mut bytes = vec![];
    bytes.extend_from_slice(&0x0200_0000u32.to_be_bytes());
    bytes.push(0x7F);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::LengthOverflow);
}

#[test]
fn length_exceeds_buffer() {
    let mut bytes = vec![];
    bytes.extend_from_slice(&100u32.to_be_bytes());
    bytes.push(0x7F);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnexpectedEof);
}

#[test]
fn unknown_frame_kind_is_rejected() {
    let mut bytes = vec![];
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.push(0x42);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnknownFrameKind { tag: 0x42 });
}

#[test]
fn retired_pane_diff_discriminant_is_rejected() {
    // The pre-ADR-0013 `PANE_DIFF` discriminant (0x40) is no longer
    // recognised. A frame carrying it must surface as UnknownFrameKind.
    let mut body = vec![0x40u8];
    // Pad some plausible-looking diff bytes; doesn't matter, decoder
    // refuses on the type byte.
    body.extend_from_slice(&[0u8; 8]);
    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);
    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(err, DecodeError::UnknownFrameKind { tag: 0x40 });
}

#[test]
fn invalid_utf8_in_hello_client_name() {
    let mut body = vec![0x01u8];
    let bad_str = [0xFFu8, 0xFE, 0xFD];
    body.extend_from_slice(&u32::try_from(bad_str.len()).unwrap().to_be_bytes());
    body.extend_from_slice(&bad_str);
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());
    body.extend_from_slice(&0u16.to_be_bytes());

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::InvalidUtf8);
}

#[test]
fn truncated_ping_body() {
    let mut bytes = vec![];
    bytes.extend_from_slice(&9u32.to_be_bytes());
    bytes.push(0x7F);
    bytes.extend_from_slice(&[0, 0, 0]);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnexpectedEof);
}

#[test]
fn tail_is_returned_after_single_frame() {
    let frame = FrameKind::Ping { nonce: 7 };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]);

    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(tail, &[0xAA, 0xBB, 0xCC]);
}

// -----------------------------------------------------------------------------
// SPEC §13 conformance: ATTACH / ATTACHED / TERMINAL_SNAPSHOT envelope.
// -----------------------------------------------------------------------------

proptest! {
    #[test]
    fn roundtrip_attach_target(target in arb_attach_target()) {
        let frame = FrameKind::Attach {
            target,
            viewport: ViewportInfo::new(80, 24),
            request_scrollback: false,
            scrollback_limit_lines: 0,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_attach_full(
        target in arb_attach_target(),
        viewport in arb_viewport_info(),
        request_scrollback in any::<bool>(),
        scrollback_limit_lines in any::<u32>(),
    ) {
        let frame = FrameKind::Attach {
            target,
            viewport,
            request_scrollback,
            scrollback_limit_lines,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_key(terminal_id in arb_terminal_id(), event in arb_key_event()) {
        let frame = FrameKind::InputKey { terminal_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_mouse(terminal_id in arb_terminal_id(), event in arb_mouse_event()) {
        let frame = FrameKind::InputMouse { terminal_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_focus(terminal_id in arb_terminal_id(), event in arb_focus_event()) {
        let frame = FrameKind::InputFocus { terminal_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_paste(terminal_id in arb_terminal_id(), event in arb_paste_event()) {
        let frame = FrameKind::InputPaste { terminal_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_session_info(info in arb_session_info()) {
        let snap = SessionSnapshot::new(info.id, WindowId::new(0), TerminalId::new(0))
            .with_sessions(vec![info]);
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_window_info(info in arb_window_info()) {
        let snap = SessionSnapshot::new(info.session_id, info.id, TerminalId::new(0))
            .with_windows(vec![info]);
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_pane_info(info in arb_pane_info()) {
        let snap = SessionSnapshot::new(SessionId::new(0), info.window_id, info.id.clone())
            .with_panes(vec![info]);
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_layout_node(layout in arb_layout_node()) {
        let win = WindowInfo::new(WindowId::new(1), SessionId::new(1), "w")
            .with_layout(Some(layout));
        let snap = SessionSnapshot::new(SessionId::new(1), WindowId::new(1), TerminalId::new(0))
            .with_windows(vec![win]);
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_attached(
        snapshot in arb_session_snapshot(),
        client_id in any::<u32>(),
    ) {
        let frame = FrameKind::Attached {
            snapshot,
            initial_client_id: ClientId::new(client_id),
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_bell(terminal_id in arb_terminal_id()) {
        let frame = FrameKind::Bell { terminal_id };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_error(
        request_id in proptest::option::of(any::<u32>()),
        code in arb_error_code(),
        message in ".{0,256}",
    ) {
        let frame = FrameKind::Error { request_id, code, message };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_viewport_resize(viewport in arb_viewport_info()) {
        let frame = FrameKind::ViewportResize { viewport };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_frame_ack(terminal_id in arb_terminal_id(), seq in any::<u64>()) {
        let frame = FrameKind::FrameAck { terminal_id, seq };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }
}

#[test]
fn detach_round_trip() {
    let frame = FrameKind::Detach;
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn detached_round_trip() {
    let frame = FrameKind::Detached;
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn attach_unknown_target_tag_is_rejected() {
    let mut body = vec![0x02u8];
    body.push(0xFF);
    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);
    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnknownEnumValue {
            field: "AttachTarget",
            value: 0xFF,
        }
    );
}

#[test]
fn input_focus_unknown_kind_is_rejected() {
    let mut body = vec![0x14u8];
    // TerminalId::Local { id: 0 } — tag byte 0x00 followed by the u32 id.
    body.push(0x00);
    body.extend_from_slice(&0u32.to_be_bytes());
    body.push(0xAB);

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnknownEnumValue {
            field: "FocusEvent",
            value: 0xAB,
        }
    );
}

#[test]
fn error_round_trip_session_not_found() {
    // The canonical case unblocking phux-byc.6.6: the server replies to an
    // ATTACH targeting an unknown session with ERROR { SESSION_NOT_FOUND }.
    let frame = FrameKind::Error {
        request_id: None,
        code: ErrorCode::SessionNotFound,
        message: "no such session: 'work'".to_owned(),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn error_round_trip_with_request_id() {
    let frame = FrameKind::Error {
        request_id: Some(42),
        code: ErrorCode::InvalidCommand,
        message: "missing field: terminal_id".to_owned(),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

#[test]
fn error_unknown_code_is_rejected() {
    // Hand-roll a TYPE_ERROR (0xC1) frame with a code that the v0.1 decoder
    // does not recognise. The decoder MUST surface UnknownEnumValue rather
    // than silently mapping to a placeholder variant.
    let mut body = vec![0xC1u8];
    body.push(0); // request_id: None tag
    body.extend_from_slice(&0x9999u16.to_be_bytes()); // unknown code
    body.extend_from_slice(&0u32.to_be_bytes()); // empty message

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnknownEnumValue {
            field: "ErrorCode",
            value: 0x9999,
        }
    );
}

#[test]
fn error_code_wire_values_match_spec() {
    // SPEC §14 names these wire values; lock them in so a refactor cannot
    // silently renumber the enum.
    assert_eq!(ErrorCode::VersionIncompatible.as_wire(), 1);
    assert_eq!(ErrorCode::UnknownMessageType.as_wire(), 2);
    assert_eq!(ErrorCode::MalformedMessage.as_wire(), 3);
    assert_eq!(ErrorCode::FrameTooLarge.as_wire(), 4);
    assert_eq!(ErrorCode::NotAttached.as_wire(), 100);
    assert_eq!(ErrorCode::AlreadyAttached.as_wire(), 101);
    assert_eq!(ErrorCode::SessionNotFound.as_wire(), 102);
    assert_eq!(ErrorCode::WindowNotFound.as_wire(), 103);
    assert_eq!(ErrorCode::TerminalNotFound.as_wire(), 104);
    assert_eq!(ErrorCode::ClientNotFound.as_wire(), 105);
    assert_eq!(ErrorCode::UnsupportedSatelliteRoute.as_wire(), 106);
    assert_eq!(ErrorCode::InvalidCommand.as_wire(), 200);
    assert_eq!(ErrorCode::PermissionDenied.as_wire(), 201);
    assert_eq!(ErrorCode::ResourceExhausted.as_wire(), 202);
    assert_eq!(ErrorCode::InternalError.as_wire(), 65535);
}

#[test]
fn bell_round_trip() {
    let frame = FrameKind::Bell {
        terminal_id: TerminalId::local(0x1234_5678),
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert!(tail.is_empty());
}

// -----------------------------------------------------------------------------
// Layout ratio validation — SPEC §13 leaves bounds implicit; phux rejects
// NaN, infinite, and out-of-range values on decode.
// -----------------------------------------------------------------------------

fn encode_split_with_ratio(ratio: f32) -> Vec<u8> {
    // Hand-roll an ATTACHED frame whose single WindowInfo carries a Split
    // node with `ratio`. The shape mirrors `info::encode_session_snapshot`
    // exactly — keep this in sync when the wire shape changes.
    let mut body = vec![0x81u8]; // TYPE_ATTACHED

    // sessions: empty list
    body.extend_from_slice(&0u32.to_be_bytes());
    // windows: one item
    body.extend_from_slice(&1u32.to_be_bytes());

    // WindowInfo
    body.extend_from_slice(&1u32.to_be_bytes()); // id
    body.extend_from_slice(&1u32.to_be_bytes()); // session_id
    body.extend_from_slice(&0u16.to_be_bytes()); // index
    body.extend_from_slice(&1u32.to_be_bytes()); // name length
    body.push(b'w'); // name bytes
    body.push(0); // active_pane: None
    body.push(1); // layout: Some
    body.push(1); // LayoutNode::Split
    body.push(0); // SplitDir::Horizontal
    body.extend_from_slice(&ratio.to_be_bytes());
    // Left leaf: LAYOUT_TAG_LEAF=0, then TerminalId::Local { id: 1 }
    body.push(0);
    body.push(0); // TERMINAL_ID_TAG_LOCAL
    body.extend_from_slice(&1u32.to_be_bytes());
    // Right leaf: LAYOUT_TAG_LEAF=0, then TerminalId::Local { id: 2 }
    body.push(0);
    body.push(0); // TERMINAL_ID_TAG_LOCAL
    body.extend_from_slice(&2u32.to_be_bytes());

    // panes: empty list
    body.extend_from_slice(&0u32.to_be_bytes());
    // focused_session, focused_window, focused_pane (tagged TerminalId)
    body.extend_from_slice(&0u32.to_be_bytes()); // focused_session
    body.extend_from_slice(&0u32.to_be_bytes()); // focused_window
    body.push(0); // TERMINAL_ID_TAG_LOCAL
    body.extend_from_slice(&1u32.to_be_bytes()); // focused_pane id

    // initial_client_id
    body.extend_from_slice(&0u32.to_be_bytes());

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

#[test]
fn layout_ratio_nan_is_rejected() {
    let bytes = encode_split_with_ratio(f32::NAN);
    let err = FrameKind::decode(&bytes).unwrap_err();
    match err {
        DecodeError::MalformedLayoutRatio { ratio } => assert!(ratio.is_nan()),
        other => panic!("expected MalformedLayoutRatio, got {other:?}"),
    }
}

#[test]
fn layout_ratio_above_one_is_rejected() {
    let bytes = encode_split_with_ratio(1.5);
    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(err, DecodeError::MalformedLayoutRatio { ratio: 1.5 });
}

#[test]
fn layout_ratio_negative_is_rejected() {
    let bytes = encode_split_with_ratio(-0.1);
    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(err, DecodeError::MalformedLayoutRatio { ratio: -0.1 });
}

#[test]
fn layout_ratio_infinite_is_rejected() {
    let bytes = encode_split_with_ratio(f32::INFINITY);
    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::MalformedLayoutRatio {
            ratio: f32::INFINITY
        }
    );
}

#[test]
#[allow(clippy::float_cmp)]
fn layout_ratio_zero_is_accepted() {
    let bytes = encode_split_with_ratio(0.0);
    let (decoded, _tail) = FrameKind::decode(&bytes).unwrap();
    if let FrameKind::Attached { snapshot, .. } = decoded {
        let win = &snapshot.windows[0];
        match win.layout.as_ref().unwrap() {
            LayoutNode::Split { ratio, .. } => assert_eq!(*ratio, 0.0),
            other => panic!("expected Split, got {other:?}"),
        }
    } else {
        panic!("expected Attached frame");
    }
}

#[test]
#[allow(clippy::float_cmp)]
fn layout_ratio_one_is_accepted() {
    let bytes = encode_split_with_ratio(1.0);
    let (decoded, _tail) = FrameKind::decode(&bytes).unwrap();
    if let FrameKind::Attached { snapshot, .. } = decoded {
        let win = &snapshot.windows[0];
        match win.layout.as_ref().unwrap() {
            LayoutNode::Split { ratio, .. } => assert_eq!(*ratio, 1.0),
            other => panic!("expected Split, got {other:?}"),
        }
    } else {
        panic!("expected Attached frame");
    }
}

#[allow(dead_code)]
const _SPLIT_DIR_TYPE_CHECK: fn() -> SplitDir = || SplitDir::Horizontal;
