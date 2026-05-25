//! Wire-codec round-trip and malformed-input tests for phux-6yl.4.
//!
//! Proptest exercises the encoder and decoder on arbitrary `FrameKind`
//! values. Hand-rolled cases cover known-bad inputs and confirm the decoder
//! returns `DecodeError` rather than panicking.

#![allow(clippy::unwrap_used)]

use bytes::BytesMut;
use phux_protocol::wire::{DecodeError, decode::Decoder, frame::FrameKind};
use proptest::prelude::*;

/// Strategy producing one of the two `FrameKind` variants implemented in
/// phux-6yl.4. Bounds on string length and modal weights keep the search
/// space tractable while still exercising edge cases (empty strings,
/// non-ASCII, etc.).
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
    ]
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
