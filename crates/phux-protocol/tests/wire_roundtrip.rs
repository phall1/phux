//! Wire-codec round-trip and malformed-input tests for phux-6yl.4 + phux-i58.
//!
//! Proptest exercises the encoder and decoder on arbitrary `FrameKind`
//! values. Hand-rolled cases cover known-bad inputs and confirm the decoder
//! returns `DecodeError` rather than panicking.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::ids::{ClientId, PaneId, SessionId, WindowId};
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachTarget, PaneSnapshotPayload, ViewportInfo};
use phux_protocol::wire::info::{
    LayoutNode, PaneInfo, SessionInfo, SessionSnapshot, SplitDir, WindowInfo,
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
        .prop_map(|(cols, rows, pixel_w, pixel_h)| ViewportInfo {
            cols,
            rows,
            pixel_w,
            pixel_h,
        })
}

fn arb_split_dir() -> impl Strategy<Value = SplitDir> {
    prop_oneof![Just(SplitDir::Horizontal), Just(SplitDir::Vertical)]
}

/// Bounded recursion: at most depth 4 keeps prop-test work tractable while
/// still exercising recursive split-tree encoding/decoding.
fn arb_layout_node() -> impl Strategy<Value = LayoutNode> {
    let leaf = any::<u32>().prop_map(|id| LayoutNode::Leaf(PaneId::new(id)));
    leaf.prop_recursive(4, 32, 2, |inner| {
        (
            arb_split_dir(),
            // Constrain ratio to the open interval so the decoder accepts it;
            // boundary and out-of-range cases are exercised by hand below.
            0.0001f32..0.9999f32,
            inner.clone(),
            inner,
        )
            .prop_map(|(dir, ratio, left, right)| LayoutNode::Split {
                dir,
                ratio,
                left: Box::new(left),
                right: Box::new(right),
            })
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
                SessionInfo {
                    id: SessionId::new(id),
                    name,
                    active_window: active_window.map(WindowId::new),
                    created_at_unix_secs,
                    window_count,
                    attached_client_count,
                }
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
        .prop_map(
            |(id, session_id, index, name, active_pane, layout)| WindowInfo {
                id: WindowId::new(id),
                session_id: SessionId::new(session_id),
                index,
                name,
                active_pane: active_pane.map(PaneId::new),
                layout,
            },
        )
}

fn arb_pane_info() -> impl Strategy<Value = PaneInfo> {
    (
        any::<u32>(),
        any::<u32>(),
        any::<u16>(),
        any::<u16>(),
        proptest::option::of(".{0,32}"),
        proptest::option::of(".{0,32}"),
    )
        .prop_map(|(id, window_id, cols, rows, title, cwd)| PaneInfo {
            id: PaneId::new(id),
            window_id: WindowId::new(window_id),
            cols,
            rows,
            title,
            cwd,
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
        .prop_map(|(sessions, windows, panes, fs, fw, fp)| SessionSnapshot {
            sessions,
            windows,
            panes,
            focused_session: SessionId::new(fs),
            focused_window: WindowId::new(fw),
            focused_pane: PaneId::new(fp),
        })
}

/// Strategy producing one of the simple-payload `FrameKind` variants. The
/// recursive/structured variants (`ATTACH`, `ATTACHED`, `PANE_SNAPSHOT`) have
/// their own dedicated round-trip proptests below.
fn arb_frame_kind() -> impl Strategy<Value = FrameKind> {
    prop_oneof![
        (
            // Bound the name to keep frames well under 16 MiB and tests fast.
            ".{0,128}",
            any::<u16>(),
            any::<u16>(),
            any::<u16>(),
        )
            .prop_map(|(client_name, major, minor, patch)| FrameKind::Hello {
                client_name,
                protocol_major: major,
                protocol_minor: minor,
                protocol_patch: patch,
            },),
        any::<u64>().prop_map(|nonce| FrameKind::Ping { nonce }),
        Just(FrameKind::Detach),
        Just(FrameKind::Detached),
        any::<u32>().prop_map(|pane_id| FrameKind::Bell { pane_id }),
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

/// Use a constrained subset of `PhysicalKey` to keep the strategy fast and
/// well within the libghostty enum's valid range. Any value the codec must
/// round-trip — not every possible u32 — is enough.
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
        // Bound text to keep tests fast. Exclude control characters per
        // SPEC §9.1.5; the codec does not enforce this but the property
        // checks identity, not validity.
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
        // Exclude NaNs — bit-identical NaN payloads round-trip, but
        // `PartialEq` on `f64::NAN` is `false`, which would fail
        // `prop_assert_eq!`. The wire format preserves NaN; the test just
        // sidesteps the equality comparison.
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

fn arb_paste_event() -> impl Strategy<Value = PasteEvent> {
    (
        prop_oneof![Just(PasteTrust::Trusted), Just(PasteTrust::Untrusted)],
        proptest::collection::vec(any::<u8>(), 0..64),
    )
        .prop_map(|(trust, data)| PasteEvent { trust, data })
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
fn truncated_length_header_is_eof() {
    // Three bytes: not enough for the u32 length.
    let bytes = [0u8, 0, 0];
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnexpectedEof);
}

#[test]
fn zero_length_is_rejected() {
    // Length header of 0 is invalid (must be >= 1 for the type byte).
    let bytes = [0u8, 0, 0, 0];
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::LengthOverflow);
}

#[test]
fn length_exceeds_protocol_cap() {
    // Length = 32 MiB, above the 16 MiB ceiling.
    let mut bytes = vec![];
    bytes.extend_from_slice(&0x0200_0000u32.to_be_bytes());
    bytes.push(0x7F); // PING tag, but length is bogus.
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::LengthOverflow);
}

#[test]
fn length_exceeds_buffer() {
    // Claims a 100-byte body, but only 1 byte (the type) is present.
    let mut bytes = vec![];
    bytes.extend_from_slice(&100u32.to_be_bytes());
    bytes.push(0x7F);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnexpectedEof);
}

#[test]
fn unknown_frame_kind_is_rejected() {
    // Length = 1, type = 0x42 (unallocated).
    let mut bytes = vec![];
    bytes.extend_from_slice(&1u32.to_be_bytes());
    bytes.push(0x42);
    let err = Decoder::new(&bytes).read_frame().unwrap_err();
    assert_eq!(err, DecodeError::UnknownFrameKind { tag: 0x42 });
}

#[test]
fn invalid_utf8_in_hello_client_name() {
    // Build a HELLO frame by hand with a non-UTF-8 client_name.
    let mut body = vec![0x01u8]; // HELLO type
    let bad_str = [0xFFu8, 0xFE, 0xFD]; // never valid UTF-8
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
    // Length claims a full PING (1 byte tag + 8 bytes nonce = 9) but body is
    // only the tag plus 3 nonce bytes.
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
    buf.extend_from_slice(&[0xAA, 0xBB, 0xCC]); // trailing junk

    let (decoded, tail) = FrameKind::decode(&buf).unwrap();
    assert_eq!(decoded, frame);
    assert_eq!(tail, &[0xAA, 0xBB, 0xCC]);
}

// -----------------------------------------------------------------------------
// phux-i58 SPEC §13 conformance: ATTACH / ATTACHED / PANE_SNAPSHOT and the
// SessionSnapshot/{SessionInfo,WindowInfo,PaneInfo,LayoutNode} types.
// -----------------------------------------------------------------------------

proptest! {
    #[test]
    fn roundtrip_attach_target(target in arb_attach_target()) {
        // AttachTarget on its own has no top-level FrameKind variant; wrap it
        // in a minimal ATTACH so the codec layer is exercised end-to-end.
        let frame = FrameKind::Attach {
            target,
            viewport: ViewportInfo { cols: 80, rows: 24, pixel_w: None, pixel_h: None },
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
    fn roundtrip_input_key(pane_id in any::<u32>(), event in arb_key_event()) {
        let frame = FrameKind::InputKey { pane_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_mouse(pane_id in any::<u32>(), event in arb_mouse_event()) {
        let frame = FrameKind::InputMouse { pane_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_focus(pane_id in any::<u32>(), event in arb_focus_event()) {
        let frame = FrameKind::InputFocus { pane_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_input_paste(pane_id in any::<u32>(), event in arb_paste_event()) {
        let frame = FrameKind::InputPaste { pane_id, event };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    /// `SessionInfo` round-trips through the snapshot codec — wrap it in a
    /// one-session SessionSnapshot to exercise the public entry points.
    #[test]
    fn roundtrip_session_info(info in arb_session_info()) {
        let snap = SessionSnapshot {
            sessions: vec![info.clone()],
            windows: Vec::new(),
            panes: Vec::new(),
            focused_session: info.id,
            focused_window: WindowId::new(0),
            focused_pane: PaneId::new(0),
        };
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_window_info(info in arb_window_info()) {
        let snap = SessionSnapshot {
            sessions: Vec::new(),
            windows: vec![info.clone()],
            panes: Vec::new(),
            focused_session: info.session_id,
            focused_window: info.id,
            focused_pane: PaneId::new(0),
        };
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_pane_info(info in arb_pane_info()) {
        let snap = SessionSnapshot {
            sessions: Vec::new(),
            windows: Vec::new(),
            panes: vec![info.clone()],
            focused_session: SessionId::new(0),
            focused_window: info.window_id,
            focused_pane: info.id,
        };
        let frame = FrameKind::Attached { snapshot: snap, initial_client_id: ClientId::new(0) };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    /// `LayoutNode` recursion is exercised via the WindowInfo it lives inside.
    /// Depth bounded by `arb_layout_node` to keep prop-test work tractable.
    #[test]
    fn roundtrip_layout_node(layout in arb_layout_node()) {
        let win = WindowInfo {
            id: WindowId::new(1),
            session_id: SessionId::new(1),
            index: 0,
            name: "w".to_owned(),
            active_pane: None,
            layout: Some(layout),
        };
        let snap = SessionSnapshot {
            sessions: Vec::new(),
            windows: vec![win],
            panes: Vec::new(),
            focused_session: SessionId::new(1),
            focused_window: WindowId::new(1),
            focused_pane: PaneId::new(0),
        };
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
    fn roundtrip_pane_snapshot_frame(
        pane_id in any::<u32>(),
        cols in any::<u16>(),
        rows in any::<u16>(),
    ) {
        // `ops` covered by `wire::diff::tests`; this test focuses on the
        // outer PANE_SNAPSHOT envelope.
        let frame = FrameKind::PaneSnapshot {
            pane_id: PaneId::new(pane_id),
            snapshot: PaneSnapshotPayload { cols, rows, ops: Vec::new() },
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn roundtrip_bell(pane_id in any::<u32>()) {
        let frame = FrameKind::Bell { pane_id };
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
    // Hand-build an ATTACH frame whose AttachTarget tag is 0xFF (unallocated).
    let mut body = vec![0x02u8]; // ATTACH type
    body.push(0xFF); // bogus AttachTarget tag

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
    let mut body = vec![0x14u8]; // INPUT_FOCUS
    body.extend_from_slice(&0u32.to_be_bytes()); // pane_id
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
fn bell_round_trip() {
    let frame = FrameKind::Bell {
        pane_id: 0x1234_5678,
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

/// Encode a single-split layout with an arbitrary `ratio` so we can exercise
/// the decoder's validation path independently of the encoder's good-citizen
/// behaviour (the encoder happily writes any f32; the decoder is the gate).
fn encode_split_with_ratio(ratio: f32) -> Vec<u8> {
    // We wrap the LayoutNode in a one-window snapshot, then an ATTACHED frame.
    // Hand-construct the LayoutNode bytes so the ratio is exactly what we want.
    let mut body = vec![0x81u8]; // ATTACHED type byte

    // SessionSnapshot { sessions: [], windows: [WindowInfo{...}], panes: [],
    //                   focused_*: 0 }
    body.extend_from_slice(&0u32.to_be_bytes()); // sessions len = 0
    body.extend_from_slice(&1u32.to_be_bytes()); // windows len = 1

    // WindowInfo {
    //   id: 1, session_id: 1, index: 0, name: "w",
    //   active_pane: None,
    //   layout: Some(Split { dir: H, ratio: <ratio>, left: Leaf(1), right: Leaf(2) })
    // }
    body.extend_from_slice(&1u32.to_be_bytes()); // window id
    body.extend_from_slice(&1u32.to_be_bytes()); // session id
    body.extend_from_slice(&0u16.to_be_bytes()); // index
    body.extend_from_slice(&1u32.to_be_bytes()); // name len = 1
    body.push(b'w');
    body.push(0); // active_pane: None
    body.push(1); // layout: Some(_)
    body.push(1); // LayoutNode::Split tag
    body.push(0); // SplitDir::Horizontal
    body.extend_from_slice(&ratio.to_be_bytes()); // ratio
    body.push(0); // left: Leaf tag
    body.extend_from_slice(&1u32.to_be_bytes()); // pane id 1
    body.push(0); // right: Leaf tag
    body.extend_from_slice(&2u32.to_be_bytes()); // pane id 2

    body.extend_from_slice(&0u32.to_be_bytes()); // panes len = 0
    body.extend_from_slice(&0u32.to_be_bytes()); // focused_session
    body.extend_from_slice(&1u32.to_be_bytes()); // focused_window
    body.extend_from_slice(&0u32.to_be_bytes()); // focused_pane

    body.extend_from_slice(&0u32.to_be_bytes()); // initial_client_id

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);
    bytes
}

#[test]
fn layout_ratio_nan_is_rejected() {
    let bytes = encode_split_with_ratio(f32::NAN);
    let err = FrameKind::decode(&bytes).unwrap_err();
    // PartialEq on the NaN-bearing variant is false; assert by pattern.
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
#[allow(clippy::float_cmp)] // exact bit-pattern is the assertion here
fn layout_ratio_zero_is_accepted() {
    // 0.0 is in [0.0, 1.0] inclusive — accepted.
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
#[allow(clippy::float_cmp)] // exact bit-pattern is the assertion here
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

// Suppress unused warning on SplitDir — the import is needed for the
// `arb_layout_node` strategy's `Just` paths.
#[allow(dead_code)]
const _SPLIT_DIR_TYPE_CHECK: fn() -> SplitDir = || SplitDir::Horizontal;

// -----------------------------------------------------------------------------
// phux-429: PANE_DIFF carries cursor + modes + base_frame_id + revision as
// struct fields per SPEC §8.1/§8.5 (not as DiffOps). The tests below exercise
// the new field layout end-to-end through encode/decode. These are appended at
// the END of the file so parallel agents can append their own sections without
// interleaving.
// -----------------------------------------------------------------------------

use phux_protocol::diff::{CursorShape, CursorState, DiffOp, PaneModes};

fn cursor_field_arb_cursor_shape() -> impl Strategy<Value = CursorShape> {
    prop_oneof![
        Just(CursorShape::Block),
        Just(CursorShape::Bar),
        Just(CursorShape::Underline),
        Just(CursorShape::BlockHollow),
    ]
}

fn cursor_field_arb_cursor_state() -> impl Strategy<Value = CursorState> {
    (
        any::<u16>(),
        any::<u16>(),
        any::<bool>(),
        cursor_field_arb_cursor_shape(),
        any::<bool>(),
    )
        .prop_map(|(row, col, visible, shape, blink)| CursorState {
            row,
            col,
            visible,
            shape,
            blink,
        })
}

fn cursor_field_arb_pane_modes() -> impl Strategy<Value = PaneModes> {
    any::<u16>().prop_map(PaneModes::from_bits)
}

proptest! {
    #[test]
    fn cursor_field_roundtrip_pane_diff(
        pane_id in any::<u32>(),
        frame_id in any::<u64>(),
        base_frame_id in any::<u64>(),
        cursor in cursor_field_arb_cursor_state(),
        modes in cursor_field_arb_pane_modes(),
        revision in any::<u8>(),
    ) {
        let frame = FrameKind::PaneDiff {
            pane_id,
            frame_id,
            base_frame_id,
            ops: Vec::new(),
            cursor,
            modes,
            revision,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }

    #[test]
    fn cursor_field_roundtrip_pane_diff_with_ops(
        pane_id in any::<u32>(),
        frame_id in any::<u64>(),
        base_frame_id in any::<u64>(),
        cursor in cursor_field_arb_cursor_state(),
        modes in cursor_field_arb_pane_modes(),
        revision in any::<u8>(),
        // A single Clear op is enough to confirm the ops field still travels
        // alongside the new fields. Op-stream details are exercised in
        // wire::diff::tests.
        clear_row in any::<u16>(),
        clear_col in any::<u16>(),
        clear_count in any::<u16>(),
    ) {
        let frame = FrameKind::PaneDiff {
            pane_id,
            frame_id,
            base_frame_id,
            ops: vec![DiffOp::Clear {
                row: clear_row,
                col: clear_col,
                count: clear_count,
            }],
            cursor,
            modes,
            revision,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, tail) = FrameKind::decode(&buf).unwrap();
        prop_assert_eq!(decoded, frame);
        prop_assert!(tail.is_empty());
    }
}

#[test]
fn cursor_field_pane_diff_invalid_cursor_shape_rejected() {
    // Hand-build a PANE_DIFF body whose embedded CursorShape tag is 0xFF.
    // Body layout: type | pane_id u32 | frame_id u64 | base_frame_id u64 |
    //              ops_count u32 (0) | cursor (row u16 | col u16 | visible u8
    //              | shape u8 | blink u8) | modes u16 | revision u8.
    let mut body = vec![0x40u8]; // TYPE_PANE_DIFF
    body.extend_from_slice(&0u32.to_be_bytes()); // pane_id
    body.extend_from_slice(&0u64.to_be_bytes()); // frame_id
    body.extend_from_slice(&0u64.to_be_bytes()); // base_frame_id
    body.extend_from_slice(&0u32.to_be_bytes()); // ops count = 0
    body.extend_from_slice(&0u16.to_be_bytes()); // cursor.row
    body.extend_from_slice(&0u16.to_be_bytes()); // cursor.col
    body.push(1); // cursor.visible
    body.push(0xFF); // cursor.shape — INVALID
    body.push(1); // cursor.blink
    body.extend_from_slice(&0u16.to_be_bytes()); // modes
    body.push(0); // revision

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnknownEnumValue {
            field: "CursorShape",
            value: 0xFF,
        }
    );
}

#[test]
fn cursor_field_pane_diff_reserved_mode_bits_round_trip() {
    // Per SPEC §16 ("tolerate unknown trailing fields"), reserved mode bits
    // travel through encode/decode unchanged so additive minor-version
    // protocol changes remain backward compatible.
    let frame = FrameKind::PaneDiff {
        pane_id: 1,
        frame_id: 2,
        base_frame_id: 1,
        ops: Vec::new(),
        cursor: CursorState::default(),
        modes: PaneModes::from_bits(0x4000), // currently unallocated bit
        revision: 0,
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, _) = FrameKind::decode(&buf).unwrap();
    if let FrameKind::PaneDiff { modes, .. } = decoded {
        assert_eq!(modes.bits(), 0x4000);
    } else {
        panic!("expected PaneDiff");
    }
}

#[test]
fn cursor_field_pane_diff_mouse_protocol_encoding_packed_fields() {
    // The 4-bit mouse_protocol (0x00F0) and 4-bit mouse_encoding (0x0F00)
    // packed nibbles survive a round-trip.
    let modes = PaneModes::EMPTY
        .with_mouse_protocol(0xA)
        .with_mouse_encoding(0x3)
        .insert(PaneModes::FOCUS_REPORTING);
    let frame = FrameKind::PaneDiff {
        pane_id: 0,
        frame_id: 0,
        base_frame_id: 0,
        ops: Vec::new(),
        cursor: CursorState::default(),
        modes,
        revision: 0,
    };
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    let (decoded, _) = FrameKind::decode(&buf).unwrap();
    if let FrameKind::PaneDiff { modes, .. } = decoded {
        assert_eq!(modes.mouse_protocol(), 0xA);
        assert_eq!(modes.mouse_encoding(), 0x3);
        assert!(modes.contains(PaneModes::FOCUS_REPORTING));
    } else {
        panic!("expected PaneDiff");
    }
}
