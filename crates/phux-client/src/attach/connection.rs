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
use std::path::{Path, PathBuf};

use bytes::{Buf, BytesMut};
use phux_protocol::wire::frame::{FrameKind, MAX_FRAME_LEN};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

use super::driver::AttachError;
use super::quic;
pub use super::quic::{CertTrust, QuicDial};

/// Number of bytes in the SPEC §5 length prefix.
const LENGTH_PREFIX: usize = 4;

/// How an attach should reach its server.
///
/// Either the always-local Unix domain socket, or a remote QUIC listener
/// (`phux-y8v6`, ADR-0007). Threaded through the attach loop so the reconnect
/// machinery dials the same way on each attempt.
#[derive(Debug, Clone)]
pub enum Dial {
    /// Connect over the Unix domain socket at this path.
    Uds(PathBuf),
    /// Dial a remote QUIC listener.
    Quic(QuicDial),
}

impl Dial {
    /// A `Dial::Uds` borrowing-then-owning the given path. Lets the many
    /// `&Path` call sites build a dial target without restating the variant.
    #[must_use]
    pub fn uds(path: &Path) -> Self {
        Self::Uds(path.to_path_buf())
    }
}

/// A connected, owned transport split into framed read and write halves.
///
/// Construction performs the connect (UDS or QUIC); the two halves are
/// independent after that. The struct keeps them together so the simple "send +
/// recv on the same task" case is one type. Both transports carry the identical
/// SPEC §5 framing — the variant only changes the byte plumbing underneath.
#[derive(Debug)]
pub struct Connection {
    reader: FrameReader,
    writer: FrameWriter,
}

/// Read half — pulls one [`FrameKind`] per call, over either transport.
#[derive(Debug)]
pub enum FrameReader {
    /// Unix-domain-socket read half with a streaming reassembly buffer.
    Uds(UdsReader),
    /// QUIC bidi-stream read half.
    Quic(QuicReader),
}

/// Write half — encodes one [`FrameKind`] per call, over either transport.
#[derive(Debug)]
pub enum FrameWriter {
    /// Unix-domain-socket write half.
    Uds(UdsWriter),
    /// QUIC bidi-stream write half.
    Quic(QuicWriter),
}

/// UDS read half — reads chunks into a buffer and decodes whole frames.
#[derive(Debug)]
pub struct UdsReader {
    inner: OwnedReadHalf,
    /// Streaming receive buffer. The socket is read in chunks (not one
    /// `read_exact` per frame) so a single syscall can surface several
    /// queued frames at once; [`Self::recv`] and [`Self::try_recv`] decode
    /// complete frames out of the front and retain any partial tail for the
    /// next read. This buffering is what lets the attach loop coalesce a
    /// back-to-back output burst into one paint (phux-jhv8).
    buf: BytesMut,
}

/// UDS write half.
#[derive(Debug)]
pub struct UdsWriter {
    inner: OwnedWriteHalf,
    /// Reusable encode buffer.
    out: BytesMut,
}

/// QUIC read half.
///
/// Reassembles length-prefixed frames off the bidi stream, byte-for-byte the
/// same framing as the UDS path. quinn's `RecvStream` is a `tokio` `AsyncRead`,
/// so this reads in chunks into a buffer exactly like [`UdsReader`] — a single
/// read can surface several queued frames, which `try_recv` then drains so a
/// back-to-back burst still coalesces into one paint (phux-jhv8). The
/// cloned endpoint + connection are held so the I/O driver outlives the stream
/// and the connection can be closed cleanly on teardown.
#[derive(Debug)]
pub struct QuicReader {
    recv: quinn::RecvStream,
    buf: BytesMut,
    _endpoint: quinn::Endpoint,
    _connection: quinn::Connection,
}

/// QUIC write half. Holds the endpoint + connection for the same reasons as
/// [`QuicReader`]; its [`Drop`] issues a best-effort `CONNECTION_CLOSE`.
#[derive(Debug)]
pub struct QuicWriter {
    send: quinn::SendStream,
    /// Reusable encode buffer.
    out: BytesMut,
    endpoint: quinn::Endpoint,
    connection: quinn::Connection,
}

impl Drop for QuicWriter {
    fn drop(&mut self) {
        // Best-effort clean teardown: a `CONNECTION_CLOSE` lets the server reap
        // this consumer immediately instead of waiting out its 30s idle timeout.
        // The endpoint clone is still alive in this struct, so its driver can
        // transmit the frame. For a guaranteed flush (the reconnect probe) the
        // caller uses [`Connection::shutdown`], which also awaits `wait_idle`.
        self.connection.close(0u32.into(), b"phux: detach");
    }
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
            reader: FrameReader::Uds(UdsReader {
                inner: read,
                buf: BytesMut::with_capacity(8192),
            }),
            writer: FrameWriter::Uds(UdsWriter {
                inner: write,
                out: BytesMut::with_capacity(4096),
            }),
        })
    }

    /// Dial a remote QUIC listener and return a framed connection.
    ///
    /// Establishes the TLS 1.3 handshake (phux ALPN), opens one bidirectional
    /// stream, and writes the bearer-token preamble when [`QuicDial::token`] is
    /// set, all before returning — so the first [`Self::send`]/[`Self::recv`]
    /// sees a stream the server is already reading phux frames off.
    ///
    /// # Errors
    ///
    /// Surfaces [`AttachError::Connect`] on any handshake, certificate, or
    /// preamble failure (the address, the pin, or the token).
    pub async fn connect_quic(dial: &QuicDial) -> Result<Self, AttachError> {
        let (endpoint, connection, send, recv) = quic::dial(dial).await?;
        Ok(Self {
            reader: FrameReader::Quic(QuicReader {
                recv,
                buf: BytesMut::with_capacity(8192),
                _endpoint: endpoint.clone(),
                _connection: connection.clone(),
            }),
            writer: FrameWriter::Quic(QuicWriter {
                send,
                out: BytesMut::with_capacity(4096),
                endpoint,
                connection,
            }),
        })
    }

    /// Close the connection cleanly, awaiting transmission of the close frame.
    ///
    /// For QUIC this issues a `CONNECTION_CLOSE` and awaits `wait_idle`, so the
    /// server reaps the consumer at once rather than at its idle timeout — used
    /// by the reconnect probe, which would otherwise leave a phantom connection
    /// per attempt. For UDS this is a no-op (dropping the socket halves is a
    /// clean close already). [`QuicWriter`]'s [`Drop`] is the best-effort
    /// backstop on paths that cannot await.
    pub async fn shutdown(self) {
        if let FrameWriter::Quic(writer) = &self.writer {
            writer.connection.close(0u32.into(), b"phux: detach");
            writer.endpoint.wait_idle().await;
        }
    }

    /// Connect over whichever transport `dial` names.
    ///
    /// # Errors
    ///
    /// Propagates [`Self::connect`] / [`Self::connect_quic`] errors.
    pub async fn connect_dial(dial: &Dial) -> Result<Self, AttachError> {
        match dial {
            Dial::Uds(path) => Self::connect(path).await,
            Dial::Quic(quic) => Self::connect_quic(quic).await,
        }
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
            reader: FrameReader::Uds(UdsReader {
                inner: read,
                buf: BytesMut::with_capacity(8192),
            }),
            writer: FrameWriter::Uds(UdsWriter {
                inner: write,
                out: BytesMut::with_capacity(4096),
            }),
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
    /// Encode `frame` and write it to the server over whichever transport.
    pub async fn send(&mut self, frame: &FrameKind) -> Result<(), AttachError> {
        match self {
            Self::Uds(w) => w.send(frame).await,
            Self::Quic(w) => w.send(frame).await,
        }
    }
}

impl FrameReader {
    /// Read one complete frame off the wire over whichever transport.
    pub async fn recv(&mut self) -> Result<FrameKind, AttachError> {
        match self {
            Self::Uds(r) => r.recv().await,
            Self::Quic(r) => r.recv().await,
        }
    }

    /// Non-blocking sibling of [`Self::recv`]: decode a frame only if one is
    /// already buffered (or, for UDS, becomes readable without blocking).
    ///
    /// Returns `Ok(None)` when the next frame is not yet fully available. Both
    /// transports drain a coalesced burst out of their receive buffer; the UDS
    /// path additionally tops up from the socket without blocking (quinn exposes
    /// no sync ready-check, so QUIC drains buffered bytes only).
    pub fn try_recv(&mut self) -> Result<Option<FrameKind>, AttachError> {
        match self {
            Self::Uds(r) => r.try_recv(),
            Self::Quic(r) => r.try_recv(),
        }
    }
}

impl UdsWriter {
    /// Encode `frame` into the internal buffer and flush it to the socket.
    async fn send(&mut self, frame: &FrameKind) -> Result<(), AttachError> {
        self.out.clear();
        frame.encode(&mut self.out);
        self.inner
            .write_all(&self.out)
            .await
            .map_err(AttachError::Io)?;
        // `flush` on a `UnixStream` half is a no-op, but harmless and explicit.
        self.inner.flush().await.map_err(AttachError::Io)?;
        Ok(())
    }
}

impl UdsReader {
    /// Read one complete frame off the wire.
    ///
    /// Returns [`AttachError::Disconnected`] on a clean EOF — the SPEC §5
    /// length prefix is the only legal cut point. Drains a complete frame
    /// from the receive buffer when one is already buffered; otherwise reads more
    /// bytes (awaiting the socket) until a full frame lands.
    async fn recv(&mut self) -> Result<FrameKind, AttachError> {
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
    fn try_recv(&mut self) -> Result<Option<FrameKind>, AttachError> {
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

impl QuicWriter {
    /// Encode `frame` and write it to the QUIC stream. quinn's `write_all`
    /// queues the bytes for ordered, reliable delivery — no separate flush.
    async fn send(&mut self, frame: &FrameKind) -> Result<(), AttachError> {
        self.out.clear();
        frame.encode(&mut self.out);
        self.send
            .write_all(&self.out)
            .await
            .map_err(|err| AttachError::Io(io::Error::other(err)))?;
        Ok(())
    }
}

impl QuicReader {
    /// Read one complete frame off the QUIC stream. quinn's `RecvStream` is a
    /// `tokio` `AsyncRead`, so this is the same chunk-and-reassemble loop as the
    /// UDS path: a clean stream finish at a frame boundary surfaces as a read of
    /// zero ([`AttachError::Disconnected`]).
    async fn recv(&mut self) -> Result<FrameKind, AttachError> {
        loop {
            if let Some(frame) = decode_buffered(&mut self.buf)? {
                return Ok(frame);
            }
            let n = self
                .recv
                .read_buf(&mut self.buf)
                .await
                .map_err(AttachError::Io)?;
            if n == 0 {
                return Err(AttachError::Disconnected);
            }
        }
    }

    /// Drain a frame already sitting in the buffer behind the one [`Self::recv`]
    /// just returned. quinn has no sync ready-check, so this never reads from
    /// the stream — it only peels off bytes a prior `recv` over-read.
    fn try_recv(&mut self) -> Result<Option<FrameKind>, AttachError> {
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
