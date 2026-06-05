//! Transport abstraction for the accept loop (`phux-486.4`).
//!
//! The server speaks one wire — length-prefixed phux frames (`docs/spec/proto.md`
//! §5) — over more than one transport. UDS is the default local transport; a
//! WebSocket transport lets browser consumers (the `phux-web` client) speak the
//! *identical* frames. We abstract at the **frame** level: each transport yields
//! complete encoded frames, so the per-client dispatch loop in [`crate::runtime`]
//! and the `FrameKind` codec are transport-agnostic and reused verbatim.
//!
//! Wire contract per transport:
//! * **UDS** — frames are length-prefixed on the byte stream, exactly as today.
//! * **WebSocket** — one binary message carries one complete encoded frame
//!   (the 4-byte length prefix is included, so the same `FrameKind::decode`
//!   path works on both ends). Text/ping/pong frames are ignored; a Close
//!   message is EOF.

use std::io;
use std::net::SocketAddr;

use bytes::BytesMut;
use futures_util::{SinkExt, StreamExt};
use phux_protocol::policy::{PeerIdentity, TransportType};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream, UnixListener};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

use phux_protocol::wire::frame::MAX_FRAME_LEN;

const LENGTH_PREFIX: usize = 4;

/// Read side of a client connection: yields one complete encoded frame (length
/// prefix included) per call, or `None` at end-of-stream.
pub(crate) trait FrameReader {
    async fn read_frame(&mut self) -> io::Result<Option<BytesMut>>;
}

/// Write side: writes one complete pre-encoded frame.
pub(crate) trait FrameWriter {
    async fn write_frame(&mut self, frame: &[u8]) -> io::Result<()>;
}

/// A listener that accepts connections, each split into a frame reader + writer.
pub(crate) trait Incoming {
    type Reader: FrameReader + 'static;
    type Writer: FrameWriter + 'static;
    async fn accept(&self) -> io::Result<(Self::Reader, Self::Writer, PeerIdentity)>;
    /// Short transport label for logs (`"uds"` / `"ws"`).
    fn kind(&self) -> &'static str;
}

// ── Unix domain socket ───────────────────────────────────────────────────────

/// UDS read half: reassembles length-prefixed frames off the byte stream.
pub(crate) struct UdsReader {
    reader: OwnedReadHalf,
    header: [u8; LENGTH_PREFIX],
}

impl FrameReader for UdsReader {
    async fn read_frame(&mut self) -> io::Result<Option<BytesMut>> {
        match self.reader.read_exact(&mut self.header).await {
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(err) => return Err(err),
        }
        let body_len = u32::from_be_bytes(self.header);
        if !(1..=MAX_FRAME_LEN).contains(&body_len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized or empty frame",
            ));
        }
        let body_len = body_len as usize;
        let mut framed = BytesMut::with_capacity(LENGTH_PREFIX + body_len);
        framed.extend_from_slice(&self.header);
        framed.resize(LENGTH_PREFIX + body_len, 0);
        self.reader.read_exact(&mut framed[LENGTH_PREFIX..]).await?;
        Ok(Some(framed))
    }
}

/// UDS write half.
pub(crate) struct UdsWriter {
    writer: OwnedWriteHalf,
}

impl FrameWriter for UdsWriter {
    async fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        self.writer.write_all(frame).await
    }
}

/// UDS listener: a thin newtype around [`UnixListener`] so the `Incoming::accept`
/// impl doesn't shadow the inherent `UnixListener::accept`.
pub(crate) struct UdsListener(UnixListener);

impl UdsListener {
    pub(crate) const fn new(listener: UnixListener) -> Self {
        Self(listener)
    }
}

impl Incoming for UdsListener {
    type Reader = UdsReader;
    type Writer = UdsWriter;

    async fn accept(&self) -> io::Result<(UdsReader, UdsWriter, PeerIdentity)> {
        let (stream, _addr) = self.0.accept().await?;
        let peer_identity = peer_identity_from_uds(&stream);
        let (reader, writer) = stream.into_split();
        Ok((
            UdsReader {
                reader,
                header: [0u8; LENGTH_PREFIX],
            },
            UdsWriter { writer },
            peer_identity,
        ))
    }

    fn kind(&self) -> &'static str {
        "uds"
    }
}

/// Extract peer identity from a Unix domain socket.
#[cfg(target_os = "linux")]
fn peer_identity_from_uds(stream: &tokio::net::UnixStream) -> PeerIdentity {
    // `UCred::pid()` is `Option<i32>` (pid_t); `PeerIdentity.pid` is
    // `Option<u32>`. A pid is non-negative, so `unsigned_abs` is exact.
    let (uid, pid) = stream.peer_cred().map_or((0, None), |cred| {
        (cred.uid(), cred.pid().map(i32::unsigned_abs))
    });
    PeerIdentity {
        uid,
        pid,
        exe_path: None,
        mcp_host_key: None,
        transport: TransportType::UnixSocket,
        source_addr: None,
    }
}

/// Extract peer identity from a Unix domain socket (non-Linux fallback).
#[allow(clippy::missing_const_for_fn)] // feature WIP (4588a0a)
#[cfg(not(target_os = "linux"))]
fn peer_identity_from_uds(_stream: &tokio::net::UnixStream) -> PeerIdentity {
    PeerIdentity {
        uid: 0,
        pid: None,
        exe_path: None,
        mcp_host_key: None,
        transport: TransportType::UnixSocket,
        source_addr: None,
    }
}

// ── WebSocket ────────────────────────────────────────────────────────────────

type Ws = WebSocketStream<TcpStream>;

/// WebSocket listener: TCP + RFC 6455 upgrade, then one binary message per frame.
pub(crate) struct WsListener {
    tcp: TcpListener,
}

impl WsListener {
    pub(crate) async fn bind(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            tcp: TcpListener::bind(addr).await?,
        })
    }

    pub(crate) fn local_addr(&self) -> io::Result<SocketAddr> {
        self.tcp.local_addr()
    }
}

/// WebSocket read half: each binary message is one complete encoded frame.
pub(crate) struct WsReader {
    rx: futures_util::stream::SplitStream<Ws>,
}

impl FrameReader for WsReader {
    async fn read_frame(&mut self) -> io::Result<Option<BytesMut>> {
        loop {
            match self.rx.next().await {
                None | Some(Ok(Message::Close(_))) => return Ok(None),
                Some(Ok(Message::Binary(data))) => {
                    let len = data.len();
                    if len < LENGTH_PREFIX || len > LENGTH_PREFIX + MAX_FRAME_LEN as usize {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "websocket frame out of bounds",
                        ));
                    }
                    return Ok(Some(BytesMut::from(&data[..])));
                }
                Some(Err(err)) => return Err(io::Error::other(err)),
                // Ignore text / ping / pong / raw — the wire is binary frames only.
                Some(Ok(_)) => {}
            }
        }
    }
}

/// WebSocket write half.
pub(crate) struct WsWriter {
    tx: futures_util::stream::SplitSink<Ws, Message>,
}

impl FrameWriter for WsWriter {
    async fn write_frame(&mut self, frame: &[u8]) -> io::Result<()> {
        self.tx
            .send(Message::Binary(frame.to_vec()))
            .await
            .map_err(io::Error::other)
    }
}

impl Incoming for WsListener {
    type Reader = WsReader;
    type Writer = WsWriter;

    async fn accept(&self) -> io::Result<(WsReader, WsWriter, PeerIdentity)> {
        let (tcp, peer) = self.tcp.accept().await?;
        let peer_identity = PeerIdentity {
            uid: 0,
            pid: None,
            exe_path: None,
            mcp_host_key: None,
            transport: TransportType::WebSocket,
            source_addr: Some(peer.ip()),
        };
        let ws = tokio_tungstenite::accept_async(tcp)
            .await
            .map_err(io::Error::other)?;
        let (tx, rx) = ws.split();
        Ok((WsReader { rx }, WsWriter { tx }, peer_identity))
    }

    fn kind(&self) -> &'static str {
        "ws"
    }
}
