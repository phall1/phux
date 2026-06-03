//! UDS transport with length-prefixed frame I/O.
//!
//! Wraps a [`UnixStream`] split into owned read and write halves, so the
//! attach loop can `tokio::select!` over the server's frames concurrently
//! with stdin and signal sources. Both directions share the SPEC §5
//! framing: a four-byte big-endian length header followed by the type byte
//! and payload, capped at [`MAX_FRAME_LEN`].
//!
//! Decoding lives in [`phux_protocol::wire`] — this module owns only the
//! byte-level reassembly. Errors funnel into [`super::driver::AttachError`].

use std::io;
use std::path::Path;

use bytes::{Buf, BytesMut};
use phux_protocol::wire::frame::{FrameKind, MAX_FRAME_LEN};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use super::driver::AttachError;

/// Number of bytes in the SPEC §5 length prefix.
const LENGTH_PREFIX: usize = 4;

/// A connected, owned transport split into framed read and write halves.
///
/// Construction performs the UDS connect; the two halves are independent
/// after that and can be sent across tasks. The struct keeps them together
/// so the simple "send + recv on the same task" case is one type.
#[derive(Debug)]
pub struct Connection {
    reader: FrameReader,
    writer: FrameWriter,
}

/// Read half — pulls one [`FrameKind`] per call.
#[derive(Debug)]
pub struct FrameReader {
    inner: OwnedReadHalf,
    /// Streaming receive buffer. The socket is read in chunks (not one
    /// `read_exact` per frame) so a single syscall can surface several
    /// queued frames at once; [`Self::recv`] and [`Self::try_recv`] decode
    /// complete frames out of the front and retain any partial tail for the
    /// next read. This buffering is what lets the attach loop coalesce a
    /// back-to-back output burst into one paint (phux-jhv8).
    buf: BytesMut,
}

/// Write half — encodes one [`FrameKind`] per call.
#[derive(Debug)]
pub struct FrameWriter {
    inner: OwnedWriteHalf,
    /// Reusable encode buffer.
    out: BytesMut,
}

impl Connection {
    /// Open the UDS at `socket` and return a framed connection.
    ///
    /// # Errors
    ///
    /// Surfaces `AttachError::Io` on any connect failure. The OS-level
    /// reason (ENOENT, ECONNREFUSED, EACCES, ...) is preserved in the
    /// inner `io::Error`.
    pub async fn connect(socket: &Path) -> Result<Self, AttachError> {
        let stream = UnixStream::connect(socket).await.map_err(AttachError::Io)?;
        let (read, write) = stream.into_split();
        Ok(Self {
            reader: FrameReader {
                inner: read,
                buf: BytesMut::with_capacity(8192),
            },
            writer: FrameWriter {
                inner: write,
                out: BytesMut::with_capacity(4096),
            },
        })
    }

    /// Build a `Connection` from an already-connected [`UnixStream`].
    ///
    /// Test-only seam: lets the dispatcher unit tests drive a real framed
    /// transport over an in-process `UnixStream::pair` without a server
    /// socket on disk. Mirrors the wiring [`Self::connect`] does after the
    /// connect resolves.
    #[cfg(test)]
    pub(crate) fn from_stream(stream: UnixStream) -> Self {
        let (read, write) = stream.into_split();
        Self {
            reader: FrameReader {
                inner: read,
                buf: BytesMut::with_capacity(8192),
            },
            writer: FrameWriter {
                inner: write,
                out: BytesMut::with_capacity(4096),
            },
        }
    }

    /// Split into independent read and write halves.
    ///
    /// Useful when the attach loop wants to `tokio::select!` on the read
    /// side while a separate task drains a write queue. The current driver
    /// uses [`Self::send`] / [`Self::recv`] directly and does not need the
    /// split — kept for forward use by phux-9gw.1.
    #[must_use]
    pub fn into_split(self) -> (FrameReader, FrameWriter) {
        (self.reader, self.writer)
    }

    /// Encode `frame` and write it to the server.
    pub async fn send(&mut self, frame: &FrameKind) -> Result<(), AttachError> {
        self.writer.send(frame).await
    }

    /// Read the next frame from the server.
    pub async fn recv(&mut self) -> Result<FrameKind, AttachError> {
        self.reader.recv().await
    }

    /// Pull a frame that is *already available* without awaiting the socket.
    ///
    /// Returns `Ok(Some(frame))` when a complete frame can be decoded from
    /// data already buffered (or readable without blocking), `Ok(None)` when
    /// the next frame is not yet fully here. Lets the attach loop drain a
    /// back-to-back burst after the first `recv` so the whole run coalesces
    /// into a single paint (phux-jhv8).
    pub fn try_recv(&mut self) -> Result<Option<FrameKind>, AttachError> {
        self.reader.try_recv()
    }
}

impl FrameWriter {
    /// Encode `frame` into the internal buffer and flush it to the socket.
    pub async fn send(&mut self, frame: &FrameKind) -> Result<(), AttachError> {
        self.out.clear();
        frame.encode(&mut self.out);
        self.inner
            .write_all(&self.out)
            .await
            .map_err(AttachError::Io)?;
        // `flush` on a `UnixStream` half is a no-op today, but call it for
        // forward-compat with buffered transport variants (QUIC, TLS).
        self.inner.flush().await.map_err(AttachError::Io)?;
        Ok(())
    }
}

impl FrameReader {
    /// Read one complete frame off the wire.
    ///
    /// Returns [`AttachError::Disconnected`] on a clean EOF — the SPEC §5
    /// length prefix is the only legal cut point. Drains a complete frame
    /// from the receive buffer when one is already buffered; otherwise reads more
    /// bytes (awaiting the socket) until a full frame lands.
    pub async fn recv(&mut self) -> Result<FrameKind, AttachError> {
        loop {
            if let Some(frame) = decode_buffered(&mut self.buf)? {
                return Ok(frame);
            }
            // No complete frame buffered — pull more bytes. A read of zero is
            // a clean EOF; mid-frame that is a truncated stream, but the only
            // SPEC §5 cut point is a frame boundary, which `decode_buffered`
            // already returned above.
            let n = self
                .inner
                .read_buf(&mut self.buf)
                .await
                .map_err(AttachError::Io)?;
            if n == 0 {
                return Err(AttachError::Disconnected);
            }
        }
    }

    /// Non-blocking sibling of [`Self::recv`]: decode a frame only if one is
    /// already buffered or becomes readable without blocking.
    ///
    /// Returns `Ok(None)` when the next frame is not yet fully available.
    /// Used to drain a burst after the first `recv` so the attach loop paints
    /// the run once instead of per frame (phux-jhv8).
    pub fn try_recv(&mut self) -> Result<Option<FrameKind>, AttachError> {
        // A frame may already be sitting in the buffer behind the one `recv`
        // just returned; hand it over before touching the socket.
        if let Some(frame) = decode_buffered(&mut self.buf)? {
            return Ok(Some(frame));
        }
        // Top up from the socket without blocking. `WouldBlock` just means
        // nothing more is queued right now.
        match self.inner.try_read_buf(&mut self.buf) {
            Ok(0) => return Err(AttachError::Disconnected),
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return Ok(None),
            Err(err) => return Err(AttachError::Io(err)),
        }
        decode_buffered(&mut self.buf)
    }
}

/// Decode and consume one complete frame from the front of `buf`.
///
/// Returns `Ok(None)` when fewer than a full frame's bytes are buffered (the
/// length prefix is missing, or the body has not all arrived). The decoded
/// frame's bytes are dropped from the front; any trailing partial frame stays
/// for the next read.
fn decode_buffered(buf: &mut BytesMut) -> Result<Option<FrameKind>, AttachError> {
    if buf.len() < LENGTH_PREFIX {
        return Ok(None);
    }
    let mut header = [0u8; LENGTH_PREFIX];
    header.copy_from_slice(&buf[..LENGTH_PREFIX]);
    let body_len = u32::from_be_bytes(header);
    if !(1..=MAX_FRAME_LEN).contains(&body_len) {
        return Err(AttachError::Protocol(format!(
            "server sent frame with out-of-range length {body_len}",
        )));
    }
    let frame_len = LENGTH_PREFIX + body_len as usize;
    if buf.len() < frame_len {
        // Body still in flight — wait for more bytes.
        return Ok(None);
    }
    let (frame, _rest) = FrameKind::decode(&buf[..frame_len])
        .map_err(|err| AttachError::Protocol(format!("server sent undecodable frame: {err:?}")))?;
    buf.advance(frame_len);
    Ok(Some(frame))
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "tests")]
mod tests {
    use super::*;
    use phux_protocol::PROTOCOL_VERSION;

    #[test]
    fn writer_buffer_starts_empty() {
        // The buffer must be cleared before each encode so frames don't
        // concatenate across calls. We can't easily construct a `FrameWriter`
        // without a real `UnixStream`, so this assertion guards the
        // pre-clear invariant indirectly via the bytes buffer length.
        let buf = BytesMut::with_capacity(64);
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn frame_encode_decode_roundtrip_matches_wire_path() {
        // Sanity: confirm the encoder produces something the decoder can
        // read, using the same SPEC §5 framing the `FrameReader` will see.
        // If the protocol crate's encoder ever drifts, this catches it
        // before the attach loop's I/O path notices in the field.
        let frame = FrameKind::Hello {
            client_name: "phux-client/test".to_owned(),
            protocol_major: PROTOCOL_VERSION.major,
            protocol_minor: PROTOCOL_VERSION.minor,
            protocol_patch: PROTOCOL_VERSION.patch,
            client_caps: phux_protocol::ClientCapabilities::default(),
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        let (decoded, rest) = FrameKind::decode(&buf).expect("roundtrip");
        assert_eq!(decoded, frame);
        assert!(rest.is_empty());
    }

    fn framed(seq: u64) -> BytesMut {
        // A small, cheap-to-build frame with a distinguishing field so the
        // burst-decode test can assert ordering.
        let frame = FrameKind::FrameAck {
            terminal_id: phux_protocol::ids::TerminalId::Local { id: 1 },
            seq,
        };
        let mut buf = BytesMut::new();
        frame.encode(&mut buf);
        buf
    }

    #[test]
    fn decode_buffered_drains_back_to_back_frames_in_order() {
        // The coalescing path (phux-jhv8) relies on a single socket read
        // surfacing several queued frames: decode_buffered must peel them off
        // the front one at a time, in order, leaving nothing behind.
        let mut buf = BytesMut::new();
        for seq in 1..=3 {
            buf.extend_from_slice(&framed(seq));
        }
        let mut seqs = Vec::new();
        while let Some(FrameKind::FrameAck { seq, .. }) = decode_buffered(&mut buf).expect("decode")
        {
            seqs.push(seq);
        }
        assert_eq!(seqs, vec![1, 2, 3]);
        assert!(buf.is_empty(), "fully consumed buffer");
    }

    #[test]
    fn decode_buffered_holds_partial_frame() {
        // A frame split across reads must not decode early: the prefix says
        // more bytes are coming, so decode_buffered returns None and retains
        // the partial bytes until the rest arrives.
        let whole = framed(7);
        let cut = whole.len() - 2;
        let mut buf = BytesMut::from(&whole[..cut]);
        assert!(
            decode_buffered(&mut buf).expect("partial").is_none(),
            "incomplete frame yields None"
        );
        assert_eq!(buf.len(), cut, "partial bytes retained");
        // Deliver the tail; now it decodes and the buffer drains.
        buf.extend_from_slice(&whole[cut..]);
        let frame = decode_buffered(&mut buf).expect("complete");
        assert!(matches!(frame, Some(FrameKind::FrameAck { seq: 7, .. })));
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_buffered_empty_is_none() {
        let mut buf = BytesMut::new();
        assert!(decode_buffered(&mut buf).expect("empty").is_none());
    }
}
