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

use crate::diff::{CursorState, DiffOp, PaneModes};
use crate::ids::{ClientId, PaneId, SessionId};
use crate::input::focus::FocusEvent;
use crate::input::key::KeyEvent;
use crate::input::mouse::MouseEvent;
use crate::input::paste::PasteEvent;

use super::decode::Decoder;
use super::diff::{encode_cursor_state, encode_diff_ops, encode_pane_modes};
use super::encode::Encoder;
use super::error::DecodeError;
use super::info::{SessionSnapshot, encode_client_id, encode_session_snapshot};

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
/// Discriminant for `PANE_SNAPSHOT` (server to client, `SPEC.md` §7.2 / §8.4).
///
/// Required per SPEC §16 conformance. Separated from `ATTACHED` per SPEC §13's
/// attach sequence: `ATTACHED` → N×`PANE_SNAPSHOT` → `PANE_DIFF` stream.
pub const TYPE_PANE_SNAPSHOT: u8 = 0x91;

// -----------------------------------------------------------------------------
// AttachTarget tagged union — SPEC §13.
// -----------------------------------------------------------------------------

/// Wire tag for [`AttachTarget::Last`].
pub(crate) const ATTACH_TARGET_LAST: u8 = 0;
/// Wire tag for [`AttachTarget::ByName`].
pub(crate) const ATTACH_TARGET_BY_NAME: u8 = 1;
/// Wire tag for [`AttachTarget::ById`].
pub(crate) const ATTACH_TARGET_BY_ID: u8 = 2;
/// Wire tag for [`AttachTarget::CreateIfMissing`].
pub(crate) const ATTACH_TARGET_CREATE_IF_MISSING: u8 = 3;

/// Session the client wishes to attach to, per SPEC §13.
///
/// Tagged union; each variant maps to one of SPEC's four selection modes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AttachTarget {
    /// Most-recently-attached session known to the server. Implementations
    /// without prior-attach memory MAY return `SESSION_NOT_FOUND`.
    Last,
    /// Look up a session by its human-readable name.
    ByName(String),
    /// Look up a session by its server-assigned [`SessionId`].
    ById(SessionId),
    /// Look up a session by name; create one if no such session exists.
    CreateIfMissing {
        /// Name for the new session (also used to match an existing one).
        name: String,
        /// Initial command to run in the seed pane, if creation occurs.
        command: Option<Vec<String>>,
        /// Working directory for the seed pane, if creation occurs.
        cwd: Option<String>,
    },
}

/// Viewport metrics the client advertises at attach time.
///
/// SPEC §13: `{ cols, rows, pixel_w: optional<u16>, pixel_h: optional<u16> }`.
/// Pixel dimensions support sub-cell rendering and image protocols; cells are
/// the load-bearing axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportInfo {
    /// Viewport width in cells.
    pub cols: u16,
    /// Viewport height in cells.
    pub rows: u16,
    /// Optional viewport width in pixels.
    pub pixel_w: Option<u16>,
    /// Optional viewport height in pixels.
    pub pixel_h: Option<u16>,
}

// -----------------------------------------------------------------------------
// PaneSnapshotPayload — body of the PANE_SNAPSHOT frame.
// -----------------------------------------------------------------------------

/// Initial state of a single pane, delivered as a `PANE_SNAPSHOT` frame.
///
/// **Phux-4az → phux-i58 minimum:** grid dimensions plus an opening sequence
/// of [`DiffOp`] that, applied to a blank `cols × rows` grid, reproduces the
/// pane. SPEC §8.4 also specifies cursor state, pane modes, and optional
/// scrollback; those fields land when the server-side replay path needs them
/// (see TODO below).
///
/// Renamed from `PaneSnapshot` (the type) → `PaneSnapshotPayload` in the
/// phux-i58 SPEC §13 conformance pass so the name `FrameKind::PaneSnapshot`
/// (the frame variant) is available.
///
/// Extending this struct is a breaking wire change with the current
/// positional encoder. SPEC.md Appendix A mandates TLV
/// (`{field_id: varint, wire_type: u8, value}`), under which extension
/// becomes additive; migration is tracked separately — see
/// `bd show phux-i58`.
///
/// TODO(post-phux-i58): extend with `cursor: CursorState`, `modes: PaneModes`,
/// and `scrollback: Option<Scrollback>` per SPEC §8.4 once the server-side
/// bridge needs them. The cursor/modes types are not yet modeled in
/// `phux_core`; do not invent them here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneSnapshotPayload {
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
/// phux-4az pass added the message-catalog variants needed for the attach
/// lifecycle. The phux-i58 SPEC §13 conformance pass conforms ATTACH/ATTACHED
/// to spec and splits out `PANE_SNAPSHOT` per SPEC §16. The remaining SPEC §7
/// catalog (`Hello_Ok`, `Pong`, `OscEvent`, `Alert`, resize/ack/command/etc.)
/// lands in sibling tasks.
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

    /// `PANE_DIFF` — server-to-client incremental pane update (`SPEC.md` §8.1).
    ///
    /// The body shape conforms to SPEC §8.1's `PANE_DIFF { pane_id, frame_id,
    /// base_frame_id, ops, cursor, modes, revision }`. Per SPEC §8.5 the
    /// `cursor` and `modes` fields ride along with every diff rather than as
    /// separate frames — pulling them into the op stream would increase
    /// wire chatter without benefit.
    ///
    /// `revision` is `0` today; SPEC §8.1 reserves it for future
    /// compression schemes (e.g. per-frame LZ4). The `pane_id` is a plain
    /// `u32` for now; the `SessionId` tagged-union from ADR-0007 §3 will
    /// replace it once satellite routing lands.
    PaneDiff {
        /// Target pane.
        pane_id: u32,
        /// Monotonic frame counter for this pane — the frame this produces.
        frame_id: u64,
        /// Frame counter this diff applies on top of. `0` means "the empty
        /// grid at pane creation"; see SPEC §8.1.
        base_frame_id: u64,
        /// Diff operations to apply, in order.
        ops: Vec<DiffOp>,
        /// Cursor state at the end of this frame (SPEC §8.5).
        cursor: CursorState,
        /// Pane-wide modes at the end of this frame (SPEC §8.5).
        modes: PaneModes,
        /// Revision tag; `0` today, reserved for SPEC §8.1 compression
        /// schemes.
        revision: u8,
    },

    /// `ATTACH` — client requests to attach to a session (`SPEC.md` §13).
    ///
    /// Conforms to SPEC §13 as of phux-i58: `target` tagged union plus
    /// viewport metrics plus scrollback negotiation.
    Attach {
        /// Which session to attach to. Tagged union with four variants.
        target: AttachTarget,
        /// Client viewport dimensions at attach time.
        viewport: ViewportInfo,
        /// Whether the client wants the server to send scrollback as part of
        /// the attach sequence.
        request_scrollback: bool,
        /// Upper bound on scrollback lines the client will accept.
        ///
        /// The server caps its own retention at `min(server_cap, this)`.
        scrollback_limit_lines: u32,
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
    /// (`SPEC.md` §13).
    ///
    /// Conforms to SPEC §13 as of phux-i58: full `SessionSnapshot` plus the
    /// server-allocated `ClientId` identifying this attachment. The per-pane
    /// initial state arrives separately via `PANE_SNAPSHOT` frames per the
    /// SPEC §13 attach sequence.
    Attached {
        /// Full graph of sessions/windows/panes plus the attaching client's
        /// initial focus triple.
        snapshot: SessionSnapshot,
        /// Server-allocated client identifier for this attachment.
        initial_client_id: ClientId,
    },

    /// `DETACHED` — server confirms detach and closes the transport
    /// (`SPEC.md` §7.3).
    ///
    /// Phux-4az scaffold carries no fields. SPEC §7.3 defines
    /// `{ reason: DetachReason, message: str }`; those land in a follow-up
    /// once the server actually distinguishes shutdown causes.
    Detached,

    /// `PANE_SNAPSHOT` — initial state of a single pane (`SPEC.md` §8.4).
    ///
    /// REQUIRED per SPEC §16 conformance. Sent after `ATTACHED` for each pane
    /// the client needs initialised; subsequent updates flow as `PANE_DIFF`.
    /// The server MAY also emit `PANE_SNAPSHOT` mid-stream as a flow-control
    /// catch-up (SPEC §12.2).
    PaneSnapshot {
        /// Target pane.
        pane_id: PaneId,
        /// Initial grid state and (eventually) cursor/modes/scrollback.
        snapshot: PaneSnapshotPayload,
    },

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
            Self::PaneSnapshot { .. } => TYPE_PANE_SNAPSHOT,
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
                base_frame_id,
                ops,
                cursor,
                modes,
                revision,
            } => {
                enc.write_u32_be(*pane_id);
                enc.write_u64_be(*frame_id);
                enc.write_u64_be(*base_frame_id);
                encode_diff_ops(ops, &mut enc);
                encode_cursor_state(*cursor, &mut enc);
                encode_pane_modes(*modes, &mut enc);
                enc.write_u8(*revision);
            }
            Self::Attach {
                target,
                viewport,
                request_scrollback,
                scrollback_limit_lines,
            } => {
                encode_attach_target(target, &mut enc);
                encode_viewport_info(viewport, &mut enc);
                enc.write_u8(u8::from(*request_scrollback));
                enc.write_u32_be(*scrollback_limit_lines);
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
                snapshot,
                initial_client_id,
            } => {
                encode_session_snapshot(snapshot, &mut enc);
                encode_client_id(*initial_client_id, &mut enc);
            }
            Self::PaneSnapshot { pane_id, snapshot } => {
                enc.write_u32_be(pane_id.get());
                encode_pane_snapshot_payload(snapshot, &mut enc);
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
// Helpers for the message-catalog variants. Kept in this file so encoder and
// decoder share one source of truth for sub-record layout.
// -----------------------------------------------------------------------------

pub(super) fn encode_attach_target(target: &AttachTarget, enc: &mut Encoder<'_>) {
    match target {
        AttachTarget::Last => {
            enc.write_u8(ATTACH_TARGET_LAST);
        }
        AttachTarget::ByName(name) => {
            enc.write_u8(ATTACH_TARGET_BY_NAME);
            enc.write_str(name);
        }
        AttachTarget::ById(id) => {
            enc.write_u8(ATTACH_TARGET_BY_ID);
            enc.write_u32_be(id.get());
        }
        AttachTarget::CreateIfMissing { name, command, cwd } => {
            enc.write_u8(ATTACH_TARGET_CREATE_IF_MISSING);
            enc.write_str(name);
            encode_optional_string_list(command.as_deref(), enc);
            encode_optional_str(cwd.as_deref(), enc);
        }
    }
}

pub(super) fn decode_attach_target(dec: &mut Decoder<'_>) -> Result<AttachTarget, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        ATTACH_TARGET_LAST => Ok(AttachTarget::Last),
        ATTACH_TARGET_BY_NAME => Ok(AttachTarget::ByName(dec.read_str()?.to_owned())),
        ATTACH_TARGET_BY_ID => Ok(AttachTarget::ById(SessionId::new(dec.read_u32_be()?))),
        ATTACH_TARGET_CREATE_IF_MISSING => {
            let name = dec.read_str()?.to_owned();
            let command = decode_optional_string_list(dec)?;
            let cwd = decode_optional_str(dec)?.map(str::to_owned);
            Ok(AttachTarget::CreateIfMissing { name, command, cwd })
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "AttachTarget",
            value: u32::from(other),
        }),
    }
}

pub(super) fn encode_viewport_info(v: &ViewportInfo, enc: &mut Encoder<'_>) {
    enc.write_u16_be(v.cols);
    enc.write_u16_be(v.rows);
    encode_optional_u16(v.pixel_w, enc);
    encode_optional_u16(v.pixel_h, enc);
}

pub(super) fn decode_viewport_info(dec: &mut Decoder<'_>) -> Result<ViewportInfo, DecodeError> {
    let cols = dec.read_u16_be()?;
    let rows = dec.read_u16_be()?;
    let pixel_w = decode_optional_u16(dec)?;
    let pixel_h = decode_optional_u16(dec)?;
    Ok(ViewportInfo {
        cols,
        rows,
        pixel_w,
        pixel_h,
    })
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

pub(super) fn encode_pane_snapshot_payload(snap: &PaneSnapshotPayload, enc: &mut Encoder<'_>) {
    enc.write_u16_be(snap.cols);
    enc.write_u16_be(snap.rows);
    encode_diff_ops(&snap.ops, enc);
}

pub(super) fn decode_pane_snapshot_payload(
    dec: &mut Decoder<'_>,
) -> Result<PaneSnapshotPayload, DecodeError> {
    use super::diff::decode_diff_ops;
    let cols = dec.read_u16_be()?;
    let rows = dec.read_u16_be()?;
    let ops = decode_diff_ops(dec)?;
    Ok(PaneSnapshotPayload { cols, rows, ops })
}

// -----------------------------------------------------------------------------
// Small option-of-primitive helpers. Local to this module — `info.rs` has its
// own parallel set tuned to its types (id newtypes, layout nodes).
// -----------------------------------------------------------------------------

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

fn encode_optional_u16(value: Option<u16>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(n) => {
            enc.write_u8(1);
            enc.write_u16_be(n);
        }
    }
}

fn decode_optional_u16(dec: &mut Decoder<'_>) -> Result<Option<u16>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => Ok(Some(dec.read_u16_be()?)),
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<u16> tag",
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

fn encode_optional_string_list(value: Option<&[String]>, enc: &mut Encoder<'_>) {
    match value {
        None => enc.write_u8(0),
        Some(list) => {
            enc.write_u8(1);
            debug_assert!(
                u32::try_from(list.len()).is_ok(),
                "string list length exceeds u32",
            );
            let len = u32::try_from(list.len()).unwrap_or(u32::MAX);
            enc.write_u32_be(len);
            for s in list {
                enc.write_str(s);
            }
        }
    }
}

fn decode_optional_string_list(dec: &mut Decoder<'_>) -> Result<Option<Vec<String>>, DecodeError> {
    let tag = dec.read_u8()?;
    match tag {
        0 => Ok(None),
        1 => {
            let len = dec.read_u32_be()?;
            let len_usize = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
            let mut out = Vec::with_capacity(len_usize);
            for _ in 0..len_usize {
                out.push(dec.read_str()?.to_owned());
            }
            Ok(Some(out))
        }
        other => Err(DecodeError::UnknownEnumValue {
            field: "Option<list<str>> tag",
            value: u32::from(other),
        }),
    }
}
