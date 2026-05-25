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
use crate::input::focus::FocusEvent;
use crate::input::key::KeyEvent;
use crate::input::mouse::MouseEvent;
use crate::input::paste::PasteEvent;

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
/// Discriminant for `ATTACH` (client to server, `SPEC.md` §7.1 / §13).
pub const TYPE_ATTACH: u8 = 0x02;
/// Discriminant for `DETACH` (client to server, `SPEC.md` §7.1 / §7.3).
pub const TYPE_DETACH: u8 = 0x03;
/// Discriminant for `INPUT_KEY` (client to server, `SPEC.md` §9.1).
pub const TYPE_INPUT_KEY: u8 = 0x10;
/// Discriminant for `INPUT_PASTE` (client to server, `SPEC.md` §9.4).
pub const TYPE_INPUT_PASTE: u8 = 0x11;
/// Discriminant for `INPUT_MOUSE` (client to server, `SPEC.md` §9.2).
pub const TYPE_INPUT_MOUSE: u8 = 0x12;
/// Discriminant for `INPUT_FOCUS` (client to server, `SPEC.md` §9.3).
pub const TYPE_INPUT_FOCUS: u8 = 0x14;
/// Discriminant for `PING` (client to server, `SPEC.md` §7.5).
pub const TYPE_PING: u8 = 0x7F;
/// Discriminant for `HELLO_OK` (server to client, `SPEC.md` §6.1). Reserved.
pub const TYPE_HELLO_OK: u8 = 0x80;
/// Discriminant for `ATTACHED` (server to client, `SPEC.md` §7.2 / §13).
pub const TYPE_ATTACHED: u8 = 0x81;
/// Discriminant for `DETACHED` (server to client, `SPEC.md` §7.2 / §7.3).
pub const TYPE_DETACHED: u8 = 0x82;
/// Discriminant for `BELL` (server to client, `SPEC.md` §7.6).
pub const TYPE_BELL: u8 = 0xB0;
/// Discriminant for `PONG` (server to client, `SPEC.md` §7.5). Reserved.
pub const TYPE_PONG: u8 = 0xFF;
/// Discriminant for `PANE_DIFF` (server to client, `SPEC.md` §7).
///
/// Picked from the §7 free range. v0.2+ may renumber when the `SessionId`
/// tagged-union routing lands; the discriminant is local to phux-6yl.5.
pub const TYPE_PANE_DIFF: u8 = 0x40;

// -----------------------------------------------------------------------------
// Auxiliary types used in `ATTACH` / `ATTACHED` bodies.
// -----------------------------------------------------------------------------

/// Client role at attach time.
///
/// **Phux-defined**, NOT yet codified in `SPEC.md` §13. Added in phux-4az to
/// give byc.6's deferred wire-integration tests something expressible. The
/// minimum useful split is `Primary` (the canonical input source — what
/// today's tmux-style attach implies) vs `Viewer` (read-only / mirror).
/// Future protocol versions may expand this to cover follower-with-cursor,
/// collaboration modes, etc. — when SPEC §13 grows a real role enum, this
/// type tracks it.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachRole {
    /// Canonical input/output participant. Forwards input, receives diffs.
    Primary = 0,
    /// Read-only mirror. Receives diffs; server discards any input frames.
    Viewer = 1,
}

/// On-wire tag for [`AttachRole::Primary`].
pub(crate) const ATTACH_ROLE_PRIMARY: u8 = 0;
/// On-wire tag for [`AttachRole::Viewer`].
pub(crate) const ATTACH_ROLE_VIEWER: u8 = 1;

/// Initial pane state delivered alongside `ATTACHED`.
///
/// **Phux-4az minimum:** grid dimensions plus an opening sequence of
/// [`DiffOp`]. `SPEC.md` §8.4 specifies more (cursor state, pane modes,
/// optional scrollback); those fields land in a follow-up when the
/// server-side replay path needs them. The encoding here treats `ops` as the
/// payload that, when applied to a freshly-initialised `cols×rows` grid,
/// reproduces the pane.
///
/// TODO(phux-byc.7+): extend with `cursor: CursorState`, `modes: PaneModes`,
/// and `scrollback: Option<Scrollback>` once the server-side bridge needs
/// them. Adding fields is additive on the wire (positional encoding is the
/// reason this remains a struct, not a tagged union).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshot {
    /// Grid width in cells.
    pub cols: u16,
    /// Grid height in cells.
    pub rows: u16,
    /// Diff operations that, applied to a blank `cols×rows` grid, reproduce
    /// the pane's current cell contents. See `SPEC.md` §8.4.
    pub ops: Vec<DiffOp>,
}

/// Decoded wire frame.
///
/// The phux-6yl.4 scaffold populated `Hello`, `Ping`, and `PaneDiff`. The
/// phux-4az pass adds the message-catalog variants needed for the attach
/// lifecycle: `Attach`/`Attached`/`Detach`/`Detached`, the four structured
/// input events from `SPEC.md` §9.1-§9.4, and `Bell` from §7.6. The
/// remaining SPEC §7 catalog (`Hello_Ok`, `Pong`, `OscEvent`, `Alert`,
/// resize/ack/command/error/etc.) lands in sibling tasks.
#[derive(Debug, Clone, PartialEq)]
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

    /// `ATTACH` — client requests to attach to a session (`SPEC.md` §7.1, §13).
    ///
    /// The phux-4az scaffold carries the session **name** plus a phux-defined
    /// [`AttachRole`]. SPEC §13 actually models the target as an
    /// `AttachTarget` tagged union (`LAST`, `BY_NAME`, `BY_ID`,
    /// `CREATE_IF_MISSING`); the union lands in a follow-up. `role` is NOT
    /// in SPEC §13 today — see [`AttachRole`] for rationale.
    Attach {
        /// Session name to attach to (UTF-8). Maps to `AttachTarget::BY_NAME`
        /// in SPEC §13's vocabulary.
        session_name: String,
        /// Client role for this attachment.
        role: AttachRole,
    },

    /// `DETACH` — client signals clean departure (`SPEC.md` §7.3).
    ///
    /// Carries no fields in the phux-4az scaffold; SPEC §7.3 also keeps it
    /// empty (the `DetachReason` is sent in `DETACHED` from the server).
    Detach,

    /// `INPUT_KEY` — client forwards a structured key event (`SPEC.md` §9.1).
    ///
    /// Wire shape: `u32` pane id followed by the encoded [`KeyEvent`].
    InputKey {
        /// Target pane.
        pane_id: u32,
        /// Structured key event; libghostty atoms inside.
        event: KeyEvent,
    },

    /// `INPUT_MOUSE` — client forwards a mouse event (`SPEC.md` §9.2).
    InputMouse {
        /// Target pane.
        pane_id: u32,
        /// Structured mouse event; coordinates are pane-local pixels.
        event: MouseEvent,
    },

    /// `INPUT_FOCUS` — client reports focus change on its host window
    /// (`SPEC.md` §9.3).
    InputFocus {
        /// Target pane.
        pane_id: u32,
        /// Whether the client window gained or lost focus.
        event: FocusEvent,
    },

    /// `INPUT_PASTE` — client forwards a paste payload (`SPEC.md` §9.4).
    InputPaste {
        /// Target pane.
        pane_id: u32,
        /// Paste payload plus trust classification.
        event: PasteEvent,
    },

    /// `ATTACHED` — server acknowledges attach with initial state
    /// (`SPEC.md` §7.2, §13).
    ///
    /// Phux-4az minimum: server-assigned `session_id`, focused
    /// `window_id`/`pane_id`, plus a [`PaneSnapshot`] of the focused pane.
    /// SPEC §13's full `SessionSnapshot` (lists of sessions/windows/panes)
    /// lands in a follow-up.
    Attached {
        /// Server-assigned session identifier.
        session_id: u32,
        /// Focused window at attach time.
        window_id: u32,
        /// Focused pane at attach time.
        pane_id: u32,
        /// Initial state of the focused pane.
        snapshot: PaneSnapshot,
    },

    /// `DETACHED` — server confirms detach and closes the transport
    /// (`SPEC.md` §7.3).
    ///
    /// Phux-4az scaffold carries no fields. SPEC §7.3 defines
    /// `{ reason: DetachReason, message: str }`; those land in a follow-up
    /// once the server actually distinguishes shutdown causes.
    Detached,

    /// `BELL` — pane received a bell character (`SPEC.md` §7.6).
    Bell {
        /// Pane that bell'd.
        pane_id: u32,
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
            Self::Attach { .. } => TYPE_ATTACH,
            Self::Detach => TYPE_DETACH,
            Self::InputKey { .. } => TYPE_INPUT_KEY,
            Self::InputMouse { .. } => TYPE_INPUT_MOUSE,
            Self::InputFocus { .. } => TYPE_INPUT_FOCUS,
            Self::InputPaste { .. } => TYPE_INPUT_PASTE,
            Self::Attached { .. } => TYPE_ATTACHED,
            Self::Detached => TYPE_DETACHED,
            Self::Bell { .. } => TYPE_BELL,
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
            Self::Attach { session_name, role } => {
                enc.write_str(session_name);
                enc.write_u8(encode_attach_role(*role));
            }
            // `Detach` and `Detached` are unit variants: just the type byte,
            // no payload. Merged to satisfy `clippy::match_same_arms`.
            Self::Detach | Self::Detached => {}
            Self::InputKey { pane_id, event } => {
                enc.write_u32_be(*pane_id);
                encode_key_event(event, &mut enc);
            }
            Self::InputMouse { pane_id, event } => {
                enc.write_u32_be(*pane_id);
                encode_mouse_event(event, &mut enc);
            }
            Self::InputFocus { pane_id, event } => {
                enc.write_u32_be(*pane_id);
                enc.write_u8(encode_focus_event(*event));
            }
            Self::InputPaste { pane_id, event } => {
                enc.write_u32_be(*pane_id);
                encode_paste_event(event, &mut enc);
            }
            Self::Attached {
                session_id,
                window_id,
                pane_id,
                snapshot,
            } => {
                enc.write_u32_be(*session_id);
                enc.write_u32_be(*window_id);
                enc.write_u32_be(*pane_id);
                encode_pane_snapshot(snapshot, &mut enc);
            }
            Self::Bell { pane_id } => {
                enc.write_u32_be(*pane_id);
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

// -----------------------------------------------------------------------------
// Helpers for the phux-4az catalog variants. Kept in this file so encoder and
// decoder share one source of truth for sub-record layout.
// -----------------------------------------------------------------------------

pub(super) const fn encode_attach_role(role: AttachRole) -> u8 {
    match role {
        AttachRole::Primary => ATTACH_ROLE_PRIMARY,
        AttachRole::Viewer => ATTACH_ROLE_VIEWER,
    }
}

pub(super) fn decode_attach_role(tag: u8) -> Result<AttachRole, DecodeError> {
    match tag {
        ATTACH_ROLE_PRIMARY => Ok(AttachRole::Primary),
        ATTACH_ROLE_VIEWER => Ok(AttachRole::Viewer),
        other => Err(DecodeError::UnknownEnumValue {
            field: "AttachRole",
            value: u32::from(other),
        }),
    }
}

pub(super) const fn encode_focus_event(event: FocusEvent) -> u8 {
    match event {
        FocusEvent::Gained => 0,
        FocusEvent::Lost => 1,
    }
}

pub(super) fn decode_focus_event(tag: u8) -> Result<FocusEvent, DecodeError> {
    match tag {
        0 => Ok(FocusEvent::Gained),
        1 => Ok(FocusEvent::Lost),
        other => Err(DecodeError::UnknownEnumValue {
            field: "FocusEvent",
            value: u32::from(other),
        }),
    }
}

pub(super) fn encode_key_event(event: &KeyEvent, enc: &mut Encoder<'_>) {
    // libghostty `Action` and `Key` are `#[repr(u32)]`; cast via `as u32` to
    // surface the discriminant. The decoder uses `TryFrom<u32>` (provided by
    // libghostty's `int_enum` derive) to round-trip.
    enc.write_u32_be(event.action as u32);
    enc.write_u32_be(event.key as u32);
    enc.write_u16_be(event.mods.bits());
    enc.write_u16_be(event.consumed_mods.bits());
    enc.write_u8(u8::from(event.composing));
    encode_optional_str(event.text.as_deref(), enc);
    encode_optional_u32(event.unshifted_codepoint, enc);
}

pub(super) fn decode_key_event(dec: &mut Decoder<'_>) -> Result<KeyEvent, DecodeError> {
    use crate::input::key::{KeyAction, ModSet, PhysicalKey};

    let action_raw = dec.read_u32_be()?;
    let action = KeyAction::try_from(action_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "KeyAction",
        value: action_raw,
    })?;
    let key_raw = dec.read_u32_be()?;
    let key = PhysicalKey::try_from(key_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "PhysicalKey",
        value: key_raw,
    })?;
    let mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let consumed_mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let composing = dec.read_u8()? != 0;
    let text = decode_optional_str(dec)?.map(str::to_owned);
    let unshifted_codepoint = decode_optional_u32(dec)?;
    Ok(KeyEvent {
        action,
        key,
        mods,
        consumed_mods,
        composing,
        text,
        unshifted_codepoint,
    })
}

pub(super) fn encode_mouse_event(event: &MouseEvent, enc: &mut Encoder<'_>) {
    enc.write_u32_be(event.action as u32);
    enc.write_u32_be(event.button as u32);
    enc.write_u16_be(event.mods.bits());
    enc.write_f64_be(event.x);
    enc.write_f64_be(event.y);
}

pub(super) fn decode_mouse_event(dec: &mut Decoder<'_>) -> Result<MouseEvent, DecodeError> {
    use crate::input::key::ModSet;
    use crate::input::mouse::{MouseAction, MouseButton};

    let action_raw = dec.read_u32_be()?;
    let action = MouseAction::try_from(action_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "MouseAction",
        value: action_raw,
    })?;
    let button_raw = dec.read_u32_be()?;
    let button = MouseButton::try_from(button_raw).map_err(|_| DecodeError::UnknownEnumValue {
        field: "MouseButton",
        value: button_raw,
    })?;
    let mods = ModSet::from_bits_truncate(dec.read_u16_be()?);
    let x = dec.read_f64_be()?;
    let y = dec.read_f64_be()?;
    Ok(MouseEvent {
        action,
        button,
        mods,
        x,
        y,
    })
}

pub(super) fn encode_paste_event(event: &PasteEvent, enc: &mut Encoder<'_>) {
    enc.write_u8(event.trust as u8);
    enc.write_bytes(&event.data);
}

pub(super) fn decode_paste_event(dec: &mut Decoder<'_>) -> Result<PasteEvent, DecodeError> {
    use crate::input::paste::PasteTrust;
    let trust_tag = dec.read_u8()?;
    let trust = match trust_tag {
        0 => PasteTrust::Trusted,
        1 => PasteTrust::Untrusted,
        other => {
            return Err(DecodeError::UnknownEnumValue {
                field: "PasteTrust",
                value: u32::from(other),
            });
        }
    };
    let data = dec.read_bytes()?.to_vec();
    Ok(PasteEvent { trust, data })
}

pub(super) fn encode_pane_snapshot(snap: &PaneSnapshot, enc: &mut Encoder<'_>) {
    enc.write_u16_be(snap.cols);
    enc.write_u16_be(snap.rows);
    encode_diff_ops(&snap.ops, enc);
}

pub(super) fn decode_pane_snapshot(dec: &mut Decoder<'_>) -> Result<PaneSnapshot, DecodeError> {
    use super::diff::decode_diff_ops;
    let cols = dec.read_u16_be()?;
    let rows = dec.read_u16_be()?;
    let ops = decode_diff_ops(dec)?;
    Ok(PaneSnapshot { cols, rows, ops })
}

fn encode_optional_str(value: Option<&str>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(s) => {
            enc.write_u8(1);
            enc.write_str(s);
        }
    }
}

fn decode_optional_str<'a>(dec: &mut Decoder<'a>) -> Result<Option<&'a str>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_str()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<str> tag",
            value: u32::from(other),
        }),
    }
}

fn encode_optional_u32(value: Option<u32>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(n) => {
            enc.write_u8(1);
            enc.write_u32_be(n);
        }
    }
}

fn decode_optional_u32(dec: &mut Decoder<'_>) -> Result<Option<u32>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_u32_be()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<u32> tag",
            value: u32::from(other),
        }),
    }
}
