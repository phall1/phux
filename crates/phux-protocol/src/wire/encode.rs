//! Wire-frame encoder. Writes into `&mut bytes::BytesMut`.
//!
//! Owned by phux-6yl.4. See `SPEC.md` Appendix A.
//!
//! All multi-byte integers are encoded **big-endian**. Strings and byte
//! sequences are length-prefixed with a `u32` big-endian count. The encoder
//! never allocates outside the `BytesMut` it borrows.

use bytes::BytesMut;

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
}
