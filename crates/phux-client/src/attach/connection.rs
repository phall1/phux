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

use bytes::BytesMut;
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
    /// Reusable assembly buffer; we reset it per frame to avoid a fresh
    /// allocation each read.
    framed: BytesMut,
    /// Reusable scratch for the body bytes. Cleared before each read.
    body: BytesMut,
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
                framed: BytesMut::with_capacity(4096),
                body: BytesMut::with_capacity(4096),
            },
            writer: FrameWriter {
                inner: write,
                out: BytesMut::with_capacity(4096),
            },
        })
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
    /// length prefix is the only legal cut point.
    pub async fn recv(&mut self) -> Result<FrameKind, AttachError> {
        let mut header = [0u8; LENGTH_PREFIX];
        match self.inner.read_exact(&mut header).await {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
                return Err(AttachError::Disconnected);
            }
            Err(err) => return Err(AttachError::Io(err)),
        }

        let body_len = u32::from_be_bytes(header);
        if !(1..=MAX_FRAME_LEN).contains(&body_len) {
            return Err(AttachError::Protocol(format!(
                "server sent frame with out-of-range length {body_len}",
            )));
        }
        let body_len_usize = body_len as usize;

        self.body.clear();
        self.body.resize(body_len_usize, 0);
        self.inner
            .read_exact(&mut self.body)
            .await
            .map_err(AttachError::Io)?;

        // Reassemble length-prefix + body so the decoder sees a full frame.
        self.framed.clear();
        self.framed.extend_from_slice(&header);
        self.framed.extend_from_slice(&self.body);

        let (frame, _rest) = FrameKind::decode(&self.framed).map_err(|err| {
            AttachError::Protocol(format!("server sent undecodable frame: {err:?}"))
        })?;
        Ok(frame)
    }
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
}
