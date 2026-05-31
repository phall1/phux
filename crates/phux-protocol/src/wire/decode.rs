//! Wire-frame decoder. Bounds-checked; never panics on malformed input.
//!
//! Owned by phux-6yl.4. See `docs/spec/proto.md` §5 (framing) and Appendix A
//! (primitives). Every decode method returns `Result` and refuses to read
//! past the end of the borrowed slice.

use super::error::DecodeError;
use super::frame::{
    ErrorCode, FrameKind, MAX_FRAME_LEN, TYPE_ATTACH, TYPE_ATTACHED, TYPE_BELL, TYPE_COMMAND,
    TYPE_COMMAND_RESULT, TYPE_DELETE_METADATA, TYPE_DETACH, TYPE_DETACHED, TYPE_ERROR, TYPE_EVENT,
    TYPE_FRAME_ACK, TYPE_GET_METADATA, TYPE_HELLO, TYPE_INPUT_FOCUS, TYPE_INPUT_KEY,
    TYPE_INPUT_MOUSE, TYPE_INPUT_PASTE, TYPE_LIST_METADATA, TYPE_METADATA_CHANGED,
    TYPE_METADATA_KEYS, TYPE_METADATA_VALUE, TYPE_PING, TYPE_SET_METADATA, TYPE_SPAWN_TERMINAL,
    TYPE_SUBSCRIBE_EVENTS, TYPE_SUBSCRIBE_METADATA, TYPE_TERMINAL_CLOSED, TYPE_TERMINAL_OUTPUT,
    TYPE_TERMINAL_RESIZE, TYPE_TERMINAL_SNAPSHOT, TYPE_TERMINAL_SPAWNED, TYPE_VIEWPORT_RESIZE,
    decode_agent_event, decode_attach_target, decode_command, decode_command_result,
    decode_focus_event, decode_key_event, decode_mouse_event, decode_optional_bytes,
    decode_optional_env, decode_optional_i32, decode_optional_str, decode_optional_string_list,
    decode_optional_terminal_id, decode_optional_u32, decode_paste_event, decode_scope,
    decode_spawn_result, decode_terminal_id, decode_viewport_info,
};
use super::info::{decode_client_id, decode_session_snapshot};
use crate::ids::CollectionId;

/// Cursor-style decoder over an immutable byte slice.
///
/// The decoder borrows its input; `read_*` methods advance an internal
/// position. None of them panic on truncated or otherwise malformed input;
/// they return [`DecodeError`] instead.
#[derive(Debug)]
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
    /// End offset of the current frame body, set by [`Self::read_frame`]
    /// once the length header is parsed. Lets variant decoders distinguish
    /// "ran out of this frame's body" (a backward-compat trailing field is
    /// absent) from "ran out of the whole buffer", without conflating a
    /// following frame's bytes with this frame's optional tail. `None`
    /// outside a framed decode (the boundary is unknown), in which case
    /// [`Self::at_body_end`] falls back to the input end.
    body_end: Option<usize>,
}

impl<'a> Decoder<'a> {
    /// Wrap `input` for primitive reads.
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self {
            input,
            pos: 0,
            body_end: None,
        }
    }

    /// Current read offset within the input.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Whether the cursor is at (or past) the end of the current frame
    /// body. A variant decoder consults this to decide whether an additive
    /// trailing field is present: `true` means the producer encoded a body
    /// that ended before this field, so the field defaults.
    ///
    /// Outside a framed decode (`body_end` unset) this falls back to the
    /// end of the borrowed input.
    #[must_use]
    pub fn at_body_end(&self) -> bool {
        self.pos >= self.body_end.unwrap_or(self.input.len())
    }

    /// Remaining (unread) bytes.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.input[self.pos..]
    }

    /// Count of bytes remaining before the current frame-body boundary (or
    /// the input end when decoding outside a framed context).
    ///
    /// Used to bound pre-allocation: a length-prefixed list cannot contain
    /// more elements than there are remaining bytes, because every element
    /// occupies at least one byte on the wire. Reserving capacity larger than
    /// this is always wasted — and lets an attacker drive an unbounded
    /// `Vec::with_capacity` from a tiny frame (a decode-path denial of
    /// service). Callers
    /// clamp their declared element count to this value before reserving;
    /// the read loop still errors with [`DecodeError::UnexpectedEof`] if the
    /// declared count overshoots the bytes actually present.
    #[must_use]
    pub fn remaining_in_body(&self) -> usize {
        self.body_end
            .unwrap_or(self.input.len())
            .saturating_sub(self.pos)
    }

    /// Reserve capacity for a length-prefixed collection without trusting the
    /// declared `count` past what the remaining frame bytes could justify.
    ///
    /// Returns a `Vec` whose capacity is `min(count, remaining_bytes)`. The
    /// caller's read loop runs `count` iterations and surfaces
    /// [`DecodeError::UnexpectedEof`] when the input runs out, so an
    /// over-declared `count` still errors cleanly — it just no longer
    /// pre-reserves gigabytes for elements that cannot possibly be present.
    #[must_use]
    pub(crate) fn bounded_capacity<T>(&self, count: usize) -> Vec<T> {
        Vec::with_capacity(count.min(self.remaining_in_body()))
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::LengthOverflow)?;
        if end > self.input.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        let slice = &self.input[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    /// Read one unsigned byte.
    pub fn read_u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }

    /// Read a `u16` in network (big-endian) byte order.
    pub fn read_u16_be(&mut self) -> Result<u16, DecodeError> {
        let slice = self.take(2)?;
        // SAFETY-free: slice length verified by `take`.
        let arr: [u8; 2] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(u16::from_be_bytes(arr))
    }

    /// Read a `u32` in network (big-endian) byte order.
    pub fn read_u32_be(&mut self) -> Result<u32, DecodeError> {
        let slice = self.take(4)?;
        let arr: [u8; 4] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(u32::from_be_bytes(arr))
    }

    /// Read a `u64` in network (big-endian) byte order.
    pub fn read_u64_be(&mut self) -> Result<u64, DecodeError> {
        let slice = self.take(8)?;
        let arr: [u8; 8] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(u64::from_be_bytes(arr))
    }

    /// Read an `i64` in network (big-endian) byte order.
    ///
    /// Two's-complement decoding; pairs with
    /// [`super::encode::Encoder::write_i64_be`]. Used by
    /// `SessionInfo::created_at_unix_secs`.
    pub fn read_i64_be(&mut self) -> Result<i64, DecodeError> {
        let slice = self.take(8)?;
        let arr: [u8; 8] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(i64::from_be_bytes(arr))
    }

    /// Read an IEEE-754 `f32` in network (big-endian) byte order.
    ///
    /// Bit-for-bit decoding via [`f32::from_be_bytes`] — preserves NaNs and
    /// signed zeros. Pairs with [`super::encode::Encoder::write_f32_be`].
    pub fn read_f32_be(&mut self) -> Result<f32, DecodeError> {
        let slice = self.take(4)?;
        let arr: [u8; 4] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(f32::from_be_bytes(arr))
    }

    /// Read an IEEE-754 `f64` in network (big-endian) byte order.
    ///
    /// Bit-for-bit decoding via [`f64::from_be_bytes`] — preserves NaNs and
    /// signed zeros. Pairs with [`super::encode::Encoder::write_f64_be`].
    pub fn read_f64_be(&mut self) -> Result<f64, DecodeError> {
        let slice = self.take(8)?;
        let arr: [u8; 8] = slice.try_into().map_err(|_| DecodeError::UnexpectedEof)?;
        Ok(f64::from_be_bytes(arr))
    }

    /// Read a length-prefixed byte slice.
    ///
    /// The length prefix is a big-endian `u32`. Returns `LengthOverflow` if
    /// the declared length exceeds the remaining input or the protocol cap.
    pub fn read_bytes(&mut self) -> Result<&'a [u8], DecodeError> {
        let len = self.read_u32_be()?;
        if len > MAX_FRAME_LEN {
            return Err(DecodeError::LengthOverflow);
        }
        let len_usize = usize::try_from(len).map_err(|_| DecodeError::LengthOverflow)?;
        self.take(len_usize)
    }

    /// Read a length-prefixed UTF-8 string.
    pub fn read_str(&mut self) -> Result<&'a str, DecodeError> {
        let bytes = self.read_bytes()?;
        core::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)
    }

    /// Read a complete wire frame from the current position. Returns the
    /// decoded frame and the unconsumed tail of the underlying input.
    ///
    /// The body is one big `match` over the SPEC §7 catalog, intentionally —
    /// keeping the dispatch table in one place trades length for locality.
    #[allow(clippy::too_many_lines)]
    pub fn read_frame(&mut self) -> Result<(FrameKind, &'a [u8]), DecodeError> {
        // Length header: u32 big-endian, excludes itself, includes type byte.
        let length = self.read_u32_be()?;
        if !(1..=MAX_FRAME_LEN).contains(&length) {
            return Err(DecodeError::LengthOverflow);
        }
        let length_usize = usize::try_from(length).map_err(|_| DecodeError::LengthOverflow)?;

        // Carve out the frame body so trailing fields can be ignored cleanly.
        let body_start = self.pos;
        let body_end = body_start
            .checked_add(length_usize)
            .ok_or(DecodeError::LengthOverflow)?;
        if body_end > self.input.len() {
            return Err(DecodeError::UnexpectedEof);
        }
        // Record the body boundary so variant decoders can detect absent
        // additive trailing fields (e.g. GET_SCREEN's `cells`) without
        // mistaking a following frame's bytes for this frame's tail.
        self.body_end = Some(body_end);

        let type_byte = self.read_u8()?;
        let frame = match type_byte {
            TYPE_HELLO => {
                let client_name = self.read_str()?.to_owned();
                let protocol_major = self.read_u16_be()?;
                let protocol_minor = self.read_u16_be()?;
                let protocol_patch = self.read_u16_be()?;
                // Backward-compat trailing fields (SPEC §6.2). A pre-7lf
                // HELLO ends right after `protocol_patch`; a 7lf-era HELLO
                // adds one byte for ColorSupport; a 4li.2-era HELLO adds
                // a further byte for the Layer bitset. phux-4rj appends
                // image_protocols, kbd_protocols, and hyperlinks. The
                // decoder tolerates every prefix per SPEC §6 ("skip them
                // by length") and applies defaults for missing bytes.
                let mut client_caps = crate::caps::ClientCapabilities::default();
                if self.pos < body_end {
                    let tag = self.read_u8()?;
                    let color_support =
                        crate::caps::ColorSupport::from_wire(tag).unwrap_or_default();
                    client_caps = client_caps.with_color_support(color_support);
                }
                if self.pos < body_end {
                    let layers = crate::caps::LayerSet::from_wire(self.read_u8()?);
                    client_caps = client_caps.with_layers(layers);
                }
                if self.pos < body_end {
                    let image_protocols = crate::caps::ImageProtocolSet::from_wire(self.read_u8()?);
                    client_caps = client_caps.with_image_protocols(image_protocols);
                }
                if self.pos < body_end {
                    let kbd_protocols =
                        crate::caps::KeyboardProtocolSet::from_wire(self.read_u8()?);
                    client_caps = client_caps.with_kbd_protocols(kbd_protocols);
                }
                if self.pos < body_end {
                    client_caps = client_caps.with_hyperlinks(self.read_u8()? != 0);
                }
                FrameKind::Hello {
                    client_name,
                    protocol_major,
                    protocol_minor,
                    protocol_patch,
                    client_caps,
                }
            }
            TYPE_PING => {
                let nonce = self.read_u64_be()?;
                FrameKind::Ping { nonce }
            }
            TYPE_TERMINAL_OUTPUT => {
                let terminal_id = decode_terminal_id(self)?;
                let seq = self.read_u64_be()?;
                let bytes = self.read_bytes()?.to_vec();
                FrameKind::TerminalOutput {
                    terminal_id,
                    seq,
                    bytes,
                }
            }
            TYPE_ATTACH => {
                let target = decode_attach_target(self)?;
                let viewport = decode_viewport_info(self)?;
                let request_scrollback = self.read_u8()? != 0;
                let scrollback_limit_lines = self.read_u32_be()?;
                FrameKind::Attach {
                    target,
                    viewport,
                    request_scrollback,
                    scrollback_limit_lines,
                }
            }
            TYPE_DETACH => FrameKind::Detach,
            TYPE_INPUT_KEY => {
                let terminal_id = decode_terminal_id(self)?;
                let event = decode_key_event(self)?;
                FrameKind::InputKey { terminal_id, event }
            }
            TYPE_INPUT_MOUSE => {
                let terminal_id = decode_terminal_id(self)?;
                let event = decode_mouse_event(self)?;
                FrameKind::InputMouse { terminal_id, event }
            }
            TYPE_INPUT_FOCUS => {
                let terminal_id = decode_terminal_id(self)?;
                let event = decode_focus_event(self.read_u8()?)?;
                FrameKind::InputFocus { terminal_id, event }
            }
            TYPE_INPUT_PASTE => {
                let terminal_id = decode_terminal_id(self)?;
                let event = decode_paste_event(self)?;
                FrameKind::InputPaste { terminal_id, event }
            }
            TYPE_FRAME_ACK => {
                let terminal_id = decode_terminal_id(self)?;
                let seq = self.read_u64_be()?;
                FrameKind::FrameAck { terminal_id, seq }
            }
            TYPE_VIEWPORT_RESIZE => {
                let viewport = decode_viewport_info(self)?;
                FrameKind::ViewportResize { viewport }
            }
            TYPE_ATTACHED => {
                let snapshot = decode_session_snapshot(self)?;
                let initial_client_id = decode_client_id(self)?;
                FrameKind::Attached {
                    snapshot,
                    initial_client_id,
                }
            }
            TYPE_TERMINAL_SNAPSHOT => {
                let terminal_id = decode_terminal_id(self)?;
                let cols = self.read_u16_be()?;
                let rows = self.read_u16_be()?;
                let vt_replay_bytes = self.read_bytes()?.to_vec();
                let scrollback_bytes = decode_optional_bytes(self)?;
                FrameKind::TerminalSnapshot {
                    terminal_id,
                    cols,
                    rows,
                    vt_replay_bytes,
                    scrollback_bytes,
                }
            }
            TYPE_DETACHED => FrameKind::Detached,
            TYPE_BELL => {
                let terminal_id = decode_terminal_id(self)?;
                FrameKind::Bell { terminal_id }
            }
            TYPE_ERROR => {
                let request_id = decode_optional_u32(self)?;
                let code_raw = self.read_u16_be()?;
                let code = ErrorCode::from_wire(code_raw).ok_or_else(|| {
                    DecodeError::UnknownEnumValue {
                        field: "ErrorCode",
                        value: u32::from(code_raw),
                    }
                })?;
                let message = self.read_str()?.to_owned();
                FrameKind::Error {
                    request_id,
                    code,
                    message,
                }
            }
            TYPE_GET_METADATA => {
                let request_id = self.read_u32_be()?;
                let scope = decode_scope(self)?;
                let key = self.read_str()?.to_owned();
                FrameKind::GetMetadata {
                    request_id,
                    scope,
                    key,
                }
            }
            TYPE_SET_METADATA => {
                let request_id = self.read_u32_be()?;
                let scope = decode_scope(self)?;
                let key = self.read_str()?.to_owned();
                let value = self.read_bytes()?.to_vec();
                FrameKind::SetMetadata {
                    request_id,
                    scope,
                    key,
                    value,
                }
            }
            TYPE_DELETE_METADATA => {
                let request_id = self.read_u32_be()?;
                let scope = decode_scope(self)?;
                let key = self.read_str()?.to_owned();
                FrameKind::DeleteMetadata {
                    request_id,
                    scope,
                    key,
                }
            }
            TYPE_LIST_METADATA => {
                let request_id = self.read_u32_be()?;
                let scope = decode_scope(self)?;
                FrameKind::ListMetadata { request_id, scope }
            }
            TYPE_SUBSCRIBE_METADATA => {
                let scope = decode_scope(self)?;
                let key = self.read_str()?.to_owned();
                FrameKind::SubscribeMetadata { scope, key }
            }
            TYPE_METADATA_CHANGED => {
                let scope = decode_scope(self)?;
                let key = self.read_str()?.to_owned();
                let value = decode_optional_bytes(self)?;
                FrameKind::MetadataChanged { scope, key, value }
            }
            TYPE_METADATA_VALUE => {
                let request_id = self.read_u32_be()?;
                let value = decode_optional_bytes(self)?;
                FrameKind::MetadataValue { request_id, value }
            }
            TYPE_METADATA_KEYS => {
                let request_id = self.read_u32_be()?;
                let count = self.read_u32_be()?;
                let count_usize =
                    usize::try_from(count).map_err(|_| DecodeError::LengthOverflow)?;
                // Bound pre-reservation by remaining bytes: an over-declared
                // count must error on EOF, not pre-allocate gigabytes.
                let mut keys = self.bounded_capacity(count_usize);
                for _ in 0..count_usize {
                    keys.push(self.read_str()?.to_owned());
                }
                FrameKind::MetadataKeys { request_id, keys }
            }
            TYPE_SPAWN_TERMINAL => {
                let request_id = self.read_u32_be()?;
                let collection = CollectionId::new(self.read_u32_be()?);
                // `command`, `cwd`, `env` follow the standard
                // `0/1`-tagged `Option` convention. See SPEC §7.2 / §10.1
                // (phux-4li.10) and the encoder symmetry in `frame.rs`.
                let command = decode_optional_string_list(self)?;
                let cwd = decode_optional_str(self)?.map(str::to_owned);
                let env = decode_optional_env(self)?;
                FrameKind::SpawnTerminal {
                    request_id,
                    collection,
                    command,
                    cwd,
                    env,
                }
            }
            TYPE_TERMINAL_SPAWNED => {
                let request_id = self.read_u32_be()?;
                let result = decode_spawn_result(self)?;
                FrameKind::TerminalSpawned { request_id, result }
            }
            TYPE_TERMINAL_CLOSED => {
                let terminal_id = decode_terminal_id(self)?;
                let exit_status = decode_optional_i32(self)?;
                FrameKind::TerminalClosed {
                    terminal_id,
                    exit_status,
                }
            }
            TYPE_TERMINAL_RESIZE => {
                let terminal_id = decode_terminal_id(self)?;
                let cols = self.read_u16_be()?;
                let rows = self.read_u16_be()?;
                FrameKind::TerminalResize {
                    terminal_id,
                    cols,
                    rows,
                }
            }
            TYPE_COMMAND => {
                let request_id = self.read_u32_be()?;
                let command = decode_command(self)?;
                FrameKind::Command {
                    request_id,
                    command,
                }
            }
            TYPE_COMMAND_RESULT => {
                let request_id = self.read_u32_be()?;
                let result = decode_command_result(self)?;
                FrameKind::CommandResult { request_id, result }
            }
            TYPE_SUBSCRIBE_EVENTS => {
                let terminal = decode_optional_terminal_id(self)?;
                FrameKind::SubscribeEvents { terminal }
            }
            TYPE_EVENT => {
                let terminal = decode_optional_terminal_id(self)?;
                let event = decode_agent_event(self)?;
                FrameKind::Event { terminal, event }
            }
            // `HELLO_OK` / `PONG` and the deferred message-catalog variants
            // (`TerminalEvent`, `Alert`, `InputRaw`, resize/ack/command/etc.)
            // are recognised by the SPEC §7 catalog but not yet populated as
            // `FrameKind` variants. Sibling tasks lift them in during the
            // integration pass. Treat them as unknown alongside genuinely
            // unallocated tags.
            other => {
                return Err(DecodeError::UnknownFrameKind {
                    tag: u16::from(other),
                });
            }
        };

        // Trailing fields the decoder didn't consume MUST be skipped per
        // SPEC §6 ("skip them by length"). Advance to the declared end.
        if self.pos > body_end {
            // The frame body claimed N bytes but the variant read more —
            // means the encoder produced a longer body than the length
            // header advertised. Treat as malformed.
            return Err(DecodeError::LengthOverflow);
        }
        self.pos = body_end;

        Ok((frame, self.remaining()))
    }
}
