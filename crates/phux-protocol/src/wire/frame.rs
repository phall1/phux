//! Frame header and `FrameKind` enum.
//!
//! Owned by phux-6yl.4. See `SPEC.md` §5 (framing) and §7 (message catalog).
//!
//! Wire layout (per `SPEC.md` §5):
//!
//! ```text
//! +-------------------------+
//! | length: u32 big-endian  |   number of bytes that follow
//! +-------------------------+
//! | type:   u8              |   message discriminant from §7
//! +-------------------------+
//! | payload: length-1 bytes |
//! +-------------------------+
//! ```
//!
//! `length` is at least `1` (the type byte) and at most `MAX_FRAME_LEN`.

use bytes::BytesMut;

use crate::diff::DiffOp;

use super::decode::Decoder;
use super::diff::encode_diff_ops;
use super::encode::Encoder;
use super::error::DecodeError;

/// Maximum permitted value of the wire-frame `length` field, per `SPEC.md` §5
/// ("at most `16_777_216` (16 MiB)").
pub const MAX_FRAME_LEN: u32 = 16 * 1024 * 1024;

// -----------------------------------------------------------------------------
// Message discriminants from SPEC §7. Only the variants implemented in this
// scaffold are exposed via `FrameKind`; the remaining IDs are recorded here so
// sibling tasks can wire them up without re-deriving the catalog.
// -----------------------------------------------------------------------------

/// Discriminant for `HELLO` (client to server, `SPEC.md` §6.1).
pub const TYPE_HELLO: u8 = 0x01;
/// Discriminant for `PING` (client to server, `SPEC.md` §7.5).
pub const TYPE_PING: u8 = 0x7F;
/// Discriminant for `HELLO_OK` (server to client, `SPEC.md` §6.1). Reserved.
pub const TYPE_HELLO_OK: u8 = 0x80;
/// Discriminant for `PONG` (server to client, `SPEC.md` §7.5). Reserved.
pub const TYPE_PONG: u8 = 0xFF;
/// Discriminant for `PANE_DIFF` (server to client, `SPEC.md` §7).
///
/// Picked from the §7 free range. v0.2+ may renumber when the `SessionId`
/// tagged-union routing lands; the discriminant is local to phux-6yl.5.
pub const TYPE_PANE_DIFF: u8 = 0x40;

/// Decoded wire frame.
///
/// Only the `Hello` and `Ping` variants are populated in the phux-6yl.4
/// scaffold; sibling tasks extend this enum with the remaining catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FrameKind {
    /// `HELLO` — client to server handshake (`SPEC.md` §6.1).
    ///
    /// The full message carries `versions: list<VersionRange>` and
    /// `client_caps`. This scaffold keeps the on-wire encoding minimal: a
    /// length-prefixed UTF-8 `client_name` string plus a `(major, minor,
    /// patch)` triple, sufficient to exercise the codec end-to-end. Sibling
    /// work fleshes out the real field set.
    Hello {
        /// Free-form client identifier (e.g. `"phux-client 0.1.0"`).
        client_name: String,
        /// Highest protocol major version the client supports.
        protocol_major: u16,
        /// Highest protocol minor version the client supports.
        protocol_minor: u16,
        /// Highest protocol patch version the client supports.
        protocol_patch: u16,
    },

    /// `PING` — liveness probe (`SPEC.md` §7.5). The peer MUST echo `nonce`
    /// back in a `PONG` frame.
    Ping {
        /// Opaque nonce echoed by the peer in `PONG`.
        nonce: u64,
    },

    /// `PANE_DIFF` — server-to-client incremental pane update (`SPEC.md` §8.3).
    ///
    /// The body carries a `u32` pane id, a `u64` frame id, then a `u32`-prefixed
    /// list of [`DiffOp`]. The `pane_id` is a plain `u32` for now; the
    /// `SessionId` tagged-union from ADR-0007 §3 will replace it once
    /// satellite routing lands.
    PaneDiff {
        /// Target pane.
        pane_id: u32,
        /// Monotonic frame counter for this pane.
        frame_id: u64,
        /// Diff operations to apply, in order.
        ops: Vec<DiffOp>,
    },
}

impl FrameKind {
    /// Type discriminant from `SPEC.md` §7.
    #[must_use]
    pub const fn type_byte(&self) -> u8 {
        match self {
            Self::Hello { .. } => TYPE_HELLO,
            Self::Ping { .. } => TYPE_PING,
            Self::PaneDiff { .. } => TYPE_PANE_DIFF,
        }
    }

    /// Encode `self` as a complete length-prefixed frame.
    ///
    /// Writes the four-byte big-endian length header, the type byte, and the
    /// payload. The caller owns the `BytesMut` lifecycle.
    pub fn encode(&self, out: &mut BytesMut) {
        // Reserve four bytes for the length header; backfill once we know how
        // many bytes the type + payload consumed.
        let header_pos = out.len();
        out.extend_from_slice(&[0u8; 4]);

        let body_start = out.len();
        let mut enc = Encoder::new(out);
        enc.write_u8(self.type_byte());

        match self {
            Self::Hello {
                client_name,
                protocol_major,
                protocol_minor,
                protocol_patch,
            } => {
                enc.write_str(client_name);
                enc.write_u16_be(*protocol_major);
                enc.write_u16_be(*protocol_minor);
                enc.write_u16_be(*protocol_patch);
            }
            Self::Ping { nonce } => {
                enc.write_u64_be(*nonce);
            }
            Self::PaneDiff {
                pane_id,
                frame_id,
                ops,
            } => {
                enc.write_u32_be(*pane_id);
                enc.write_u64_be(*frame_id);
                encode_diff_ops(ops, &mut enc);
            }
        }

        // Backfill the length header. The length value excludes the four
        // header bytes themselves but includes the type byte and payload, per
        // SPEC §5.
        let body_len = out.len() - body_start;
        debug_assert!(
            u32::try_from(body_len).is_ok_and(|n| n <= MAX_FRAME_LEN),
            "encoded frame exceeds protocol cap",
        );
        let len_u32 = u32::try_from(body_len).unwrap_or(u32::MAX);
        out[header_pos..header_pos + 4].copy_from_slice(&len_u32.to_be_bytes());
    }

    /// Decode a single frame from `input`. Returns the decoded frame and the
    /// unconsumed tail of `input`.
    pub fn decode(input: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        Decoder::new(input).read_frame()
    }
}
