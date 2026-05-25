//! Wire-codec round-trip and malformed-input tests for phux-6yl.4.
//!
//! Proptest exercises the encoder and decoder on arbitrary `FrameKind`
//! values. Hand-rolled cases cover known-bad inputs and confirm the decoder
//! returns `DecodeError` rather than panicking.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::input::focus::FocusEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::input::mouse::{MouseAction, MouseButton, MouseEvent};
use phux_protocol::input::paste::{PasteEvent, PasteTrust};
use phux_protocol::wire::frame::{AttachRole, PaneSnapshot};
use phux_protocol::wire::{DecodeError, decode::Decoder, frame::FrameKind};
use proptest::prelude::*;

/// Strategy producing one of the implemented `FrameKind` variants. Bounds on
/// string and payload length keep the search space tractable while still
/// exercising edge cases (empty strings, non-ASCII, etc.). The `PaneDiff`,
/// `Attached`, `InputKey`, etc. variants have their own dedicated round-trip
/// tests below; this strategy focuses on the catalog-level dispatch and the
/// simple-payload variants.
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
        (".{0,64}", arb_attach_role())
            .prop_map(|(session_name, role)| FrameKind::Attach { session_name, role }),
        Just(FrameKind::Detach),
        Just(FrameKind::Detached),
        any::<u32>().prop_map(|pane_id| FrameKind::Bell { pane_id }),
    ]
}

fn arb_attach_role() -> impl Strategy<Value = AttachRole> {
    prop_oneof![Just(AttachRole::Primary), Just(AttachRole::Viewer)]
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
// phux-4az: message-catalog round-trip tests
// -----------------------------------------------------------------------------

proptest! {
    #[test]
    fn roundtrip_attach(
        session_name in ".{0,64}",
        role in arb_attach_role(),
    ) {
        let frame = FrameKind::Attach { session_name, role };
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

    #[test]
    fn roundtrip_attached(
        session_id in any::<u32>(),
        window_id in any::<u32>(),
        pane_id in any::<u32>(),
        cols in any::<u16>(),
        rows in any::<u16>(),
    ) {
        // `ops` left empty here; `wire::diff::tests` already exercises the
        // `Vec<DiffOp>` encoding. This test focuses on the outer envelope.
        let snapshot = PaneSnapshot { cols, rows, ops: Vec::new() };
        let frame = FrameKind::Attached { session_id, window_id, pane_id, snapshot };
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
fn attach_unknown_role_is_rejected() {
    // Hand-build an ATTACH frame whose role byte is 0xFF (unallocated).
    let mut body = vec![0x02u8]; // ATTACH type
    let name = b"default";
    body.extend_from_slice(&u32::try_from(name.len()).unwrap().to_be_bytes());
    body.extend_from_slice(name);
    body.push(0xFF);

    let mut bytes = vec![];
    bytes.extend_from_slice(&u32::try_from(body.len()).unwrap().to_be_bytes());
    bytes.extend_from_slice(&body);

    let err = FrameKind::decode(&bytes).unwrap_err();
    assert_eq!(
        err,
        DecodeError::UnknownEnumValue {
            field: "AttachRole",
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
