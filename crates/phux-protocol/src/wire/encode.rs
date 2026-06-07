//! Wire-frame encoder. Writes into `&mut bytes::BytesMut`.
//!
//! Owned by phux-6yl.4. See `docs/spec/appendix-encoding.md`.
//!
//! Message bodies are **field-tagged TLV** (`docs/spec/appendix-encoding.md`):
//! every top-level field of a message is written as
//! `field_id: varint || wire_type: u8 || value`, where `value` for the
//! length-delimited wire types is `varint length || bytes`. A decoder reads
//! fields by id and skips any field id it does not recognise by that field's
//! declared length — the forward-compat lever. Optional / trailing fields are
//! simply absent tagged fields.
//!
//! Inside a field's value the leaf primitives below are still encoded
//! positionally and **big-endian**: multi-byte integers big-endian, strings
//! and byte sequences length-prefixed with a `u32` big-endian count. The
//! encoder never allocates outside the `BytesMut` it borrows.

use bytes::BytesMut;

/// TLV wire types (`docs/spec/appendix-encoding.md` §1).
///
/// The `wire_type` byte follows the `field_id` varint and tells a decoder how
/// to read — and, for an unrecognised `field_id`, how to skip — the field's
/// value.
///
/// Every wire type phux emits at the message-body level is **length-delimited**
/// (`varint length || bytes`): a decoder skips an unknown field by reading its
/// varint length and advancing past that many bytes, without needing to know
/// the field's logical type. The fixed-width / varint scalar types in the
/// Appendix A table are reserved for future nested use and are not emitted at
/// the top level today.
pub mod wire_type {
    /// Length-delimited opaque bytes: `varint length || bytes`.
    ///
    /// The single wire type phux emits for every top-level message field — the
    /// field's value is the positional encoding of that logical field, captured
    /// as an opaque length-delimited blob so unknown fields skip cleanly by
    /// length.
    pub const BYTES: u8 = 4;
}

/// Low-level primitive encoder.
///
/// The encoder is a thin wrapper around a borrowed `BytesMut`; dropping it has
/// no effect on the underlying buffer. Construct one per logical write batch.
#[derive(Debug)]
pub struct Encoder<'a> {
    buf: &'a mut BytesMut,
}

impl<'a> Encoder<'a> {
    /// Wrap `buf` for primitive writes.
    #[must_use]
    pub const fn new(buf: &'a mut BytesMut) -> Self {
        Self { buf }
    }

    /// Borrow the underlying buffer.
    #[must_use]
    pub const fn buffer(&self) -> &BytesMut {
        self.buf
    }

    /// Total bytes currently in the underlying buffer.
    #[must_use]
    pub fn position(&self) -> usize {
        self.buf.len()
    }

    /// Write one unsigned byte.
    pub fn write_u8(&mut self, value: u8) {
        self.buf.extend_from_slice(&[value]);
    }

    /// Write a `u16` in network (big-endian) byte order.
    pub fn write_u16_be(&mut self, value: u16) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a `u32` in network (big-endian) byte order.
    pub fn write_u32_be(&mut self, value: u32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a `u64` in network (big-endian) byte order.
    pub fn write_u64_be(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write an `i64` in network (big-endian) byte order.
    ///
    /// Two's-complement encoding (bit-identical to `u64` after reinterpret).
    /// Used by `SessionInfo::created_at_unix_secs` per SPEC §13 wire-portable
    /// timestamp convention.
    pub fn write_i64_be(&mut self, value: i64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write an IEEE-754 `f32` in network (big-endian) byte order.
    ///
    /// Bit-for-bit encoding via [`f32::to_be_bytes`] — preserves NaNs and
    /// signed zeros. Used by `LayoutNode::Split::ratio` per SPEC §13.
    pub fn write_f32_be(&mut self, value: f32) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write an IEEE-754 `f64` in network (big-endian) byte order.
    ///
    /// Bit-for-bit encoding via [`f64::to_be_bytes`] — preserves NaNs and
    /// signed zeros. Used by mouse events whose pane-local positions are
    /// pixel-precise per docs/spec/input.md §3.1.
    pub fn write_f64_be(&mut self, value: f64) {
        self.buf.extend_from_slice(&value.to_be_bytes());
    }

    /// Write a length-prefixed UTF-8 string.
    ///
    /// The length prefix is a `u32` big-endian count of UTF-8 bytes (not
    /// `char`s). Empty strings encode as a four-byte zero header.
    ///
    /// # Panics
    ///
    /// Debug-asserts that the byte length fits in a `u32`. In release builds
    /// the length is saturated to `u32::MAX`; the decoder's `LengthOverflow`
    /// check will reject the malformed frame on the other end.
    pub fn write_str(&mut self, value: &str) {
        self.write_bytes(value.as_bytes());
    }

    /// Write a length-prefixed byte slice.
    ///
    /// The length prefix is a `u32` big-endian byte count.
    pub fn write_bytes(&mut self, value: &[u8]) {
        debug_assert!(
            u32::try_from(value.len()).is_ok(),
            "length-prefixed payload exceeds u32",
        );
        let len = u32::try_from(value.len()).unwrap_or(u32::MAX);
        self.write_u32_be(len);
        self.buf.extend_from_slice(value);
    }

    /// Write an unsigned LEB128 varint (`docs/spec/appendix-encoding.md`,
    /// `wire_type` `VARINT`).
    ///
    /// Each byte carries seven value bits in its low bits; the high bit is a
    /// continuation flag (`1` = more bytes follow). Used for TLV `field_id`s
    /// and length prefixes. Small values (`< 128`) encode in a single byte,
    /// which is what keeps the common low-numbered field ids cheap.
    pub fn write_varint(&mut self, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.buf.extend_from_slice(&[byte]);
                break;
            }
            self.buf.extend_from_slice(&[byte | 0x80]);
        }
    }

    /// Write one TLV field at the message-body level: `field_id: varint ||
    /// wire_type: u8 (BYTES) || varint length || value bytes`
    /// (`docs/spec/appendix-encoding.md` §1).
    ///
    /// `value` is the positional encoding of the logical field captured as an
    /// opaque blob; carrying it length-delimited is what lets a decoder skip a
    /// field id it does not recognise. Callers assign stable, contiguous
    /// `field_id`s per message (see [`super::field`]); a value that is logically
    /// absent (`None`, an empty trailing field) is simply not written, so an
    /// older or newer peer round-trips by id rather than position.
    pub fn write_field(&mut self, field_id: u32, value: &[u8]) {
        self.write_varint(u64::from(field_id));
        self.write_u8(wire_type::BYTES);
        debug_assert!(
            u32::try_from(value.len()).is_ok(),
            "TLV field value exceeds u32",
        );
        self.write_varint(value.len() as u64);
        self.buf.extend_from_slice(value);
    }

    /// Write one TLV field whose value is produced by `build` writing
    /// positionally into a scratch [`Encoder`].
    ///
    /// The ergonomic counterpart to [`Self::write_field`] for the common case
    /// where a field's value is the positional encoding of a leaf primitive or
    /// a nested tagged union: the closure writes into a fresh buffer, and the
    /// captured bytes become the field's length-delimited value. Keeping the
    /// nested encoders positional (and only the *message body* field-tagged)
    /// is what lets the existing leaf / sub-record codecs stay untouched.
    pub fn write_field_with<F>(&mut self, field_id: u32, build: F)
    where
        F: FnOnce(&mut Encoder<'_>),
    {
        let mut scratch = BytesMut::new();
        {
            let mut sub = Encoder::new(&mut scratch);
            build(&mut sub);
        }
        self.write_field(field_id, &scratch);
    }
}
