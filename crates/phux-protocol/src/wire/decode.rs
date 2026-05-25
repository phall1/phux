//! Wire-frame decoder. Bounds-checked; never panics on malformed input.
//!
//! Owned by phux-6yl.4. See `SPEC.md` §5 (framing) and Appendix A
//! (primitives). Every decode method returns `Result` and refuses to read
//! past the end of the borrowed slice.

use super::diff::decode_diff_ops;
use super::error::DecodeError;
use super::frame::{
    FrameKind, MAX_FRAME_LEN, TYPE_ATTACH, TYPE_ATTACHED, TYPE_BELL, TYPE_DETACH, TYPE_DETACHED,
    TYPE_HELLO, TYPE_INPUT_FOCUS, TYPE_INPUT_KEY, TYPE_INPUT_MOUSE, TYPE_INPUT_PASTE,
    TYPE_PANE_DIFF, TYPE_PING, decode_attach_role, decode_focus_event, decode_key_event,
    decode_mouse_event, decode_pane_snapshot, decode_paste_event,
};

/// Cursor-style decoder over an immutable byte slice.
///
/// The decoder borrows its input; `read_*` methods advance an internal
/// position. None of them panic on truncated or otherwise malformed input;
/// they return [`DecodeError`] instead.
#[derive(Debug)]
pub struct Decoder<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Decoder<'a> {
    /// Wrap `input` for primitive reads.
    #[must_use]
    pub const fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    /// Current read offset within the input.
    #[must_use]
    pub const fn position(&self) -> usize {
        self.pos
    }

    /// Remaining (unread) bytes.
    #[must_use]
    pub fn remaining(&self) -> &'a [u8] {
        &self.input[self.pos..]
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

        let type_byte = self.read_u8()?;
        let frame = match type_byte {
            TYPE_HELLO => {
                let client_name = self.read_str()?.to_owned();
                let protocol_major = self.read_u16_be()?;
                let protocol_minor = self.read_u16_be()?;
                let protocol_patch = self.read_u16_be()?;
                FrameKind::Hello {
                    client_name,
                    protocol_major,
                    protocol_minor,
                    protocol_patch,
                }
            }
            TYPE_PING => {
                let nonce = self.read_u64_be()?;
                FrameKind::Ping { nonce }
            }
            TYPE_PANE_DIFF => {
                let pane_id = self.read_u32_be()?;
                let frame_id = self.read_u64_be()?;
                let ops = decode_diff_ops(self)?;
                FrameKind::PaneDiff {
                    pane_id,
                    frame_id,
                    ops,
                }
            }
            TYPE_ATTACH => {
                let session_name = self.read_str()?.to_owned();
                let role = decode_attach_role(self.read_u8()?)?;
                FrameKind::Attach { session_name, role }
            }
            TYPE_DETACH => FrameKind::Detach,
            TYPE_INPUT_KEY => {
                let pane_id = self.read_u32_be()?;
                let event = decode_key_event(self)?;
                FrameKind::InputKey { pane_id, event }
            }
            TYPE_INPUT_MOUSE => {
                let pane_id = self.read_u32_be()?;
                let event = decode_mouse_event(self)?;
                FrameKind::InputMouse { pane_id, event }
            }
            TYPE_INPUT_FOCUS => {
                let pane_id = self.read_u32_be()?;
                let event = decode_focus_event(self.read_u8()?)?;
                FrameKind::InputFocus { pane_id, event }
            }
            TYPE_INPUT_PASTE => {
                let pane_id = self.read_u32_be()?;
                let event = decode_paste_event(self)?;
                FrameKind::InputPaste { pane_id, event }
            }
            TYPE_ATTACHED => {
                let session_id = self.read_u32_be()?;
                let window_id = self.read_u32_be()?;
                let pane_id = self.read_u32_be()?;
                let snapshot = decode_pane_snapshot(self)?;
                FrameKind::Attached {
                    session_id,
                    window_id,
                    pane_id,
                    snapshot,
                }
            }
            TYPE_DETACHED => FrameKind::Detached,
            TYPE_BELL => {
                let pane_id = self.read_u32_be()?;
                FrameKind::Bell { pane_id }
            }
            // `HELLO_OK` / `PONG` and the deferred message-catalog variants
            // (`OscEvent`, `Alert`, `InputRaw`, resize/ack/command/etc.) are
            // recognised by the SPEC §7 catalog but not yet populated as
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
