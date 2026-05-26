//! Shared scaffolding for the `phux-byc.6.*` wire integration tests.
//!
//! Each `tests/*.rs` file in this crate compiles to its own integration
//! binary. Cargo's convention for sharing helper code across binaries is
//! a `tests/common/mod.rs` that gets pulled in via `mod common;` from
//! each binary. The module name `common` is special-cased by Cargo:
//! files under `tests/common/` are NOT compiled as standalone test
//! binaries (avoiding the "unused dead code" warnings that would
//! otherwise appear in any binary that doesn't use a given helper).
//!
//! The helpers here intentionally avoid touching `phux-server`'s
//! internals — every interaction goes through the public `ServerRuntime`
//! API plus the wire-frame surface from `phux_protocol`. That keeps the
//! tests honest: a regression that only shows up over the wire will
//! show up here, even if `ServerState` unit tests keep passing.
//!
//! All `recv` paths in these helpers are wrapped in `tokio::time::timeout`
//! per the byc.6.* tickets (5-second deadline). A hang is a failure, not
//! a wait-for-Godot.

#![allow(dead_code, reason = "shared helpers; some binaries use a subset")]
#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(clippy::missing_panics_doc, reason = "tests")]
// `unreachable_pub` and `clippy::redundant_pub_crate` are mutually
// exclusive on this file: `pub(crate)` triggers the latter (module is
// private, so the restriction is "redundant"), while `pub` triggers
// the former (no re-export path). The cargo `tests/common/` pattern is
// non-negotiable — each integration binary `mod common;`s the file in
// fresh, so `pub` is the only visibility that actually exports helpers
// to the binaries that need them. Suppress `unreachable_pub` here and
// keep `pub`.
#![allow(unreachable_pub, reason = "tests/common shared-helpers pattern")]

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use phux_protocol::wire::frame::{AttachTarget, FrameKind, ViewportInfo};
use phux_server::{ServerConfig, ServerError, ServerRuntime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

/// Deadline applied to every wire `recv` in the byc.6.* tests. Matches the
/// ticket's "wrap every recv in `tokio::time::timeout(Duration::from_secs(5))`"
/// requirement.
pub const WIRE_RECV_TIMEOUT: Duration = Duration::from_secs(5);

/// Deadline for the per-test socket-connect bootstrap. Two seconds matches
/// the byc.8 `attach_lifecycle` helper and is a comfortable margin for the
/// `bind() + LocalSet::run_until` ramp on a busy CI box.
pub const SOCKET_CONNECT_DEADLINE: Duration = Duration::from_secs(2);

/// Spawn a [`ServerRuntime`] on the current `LocalSet`, optionally pre-
/// seeding a session by name. Returns the shutdown sender and the join
/// handle so each test can drive a clean shutdown.
///
/// Per ADR-0014 the server runs on a `LocalSet` because per-pane
/// `PaneActor`s own `!Send` `libghostty_vt::Terminal`s — callers MUST
/// invoke this from inside a `LocalSet::run_until` (see [`run_local`]).
pub fn spawn_server(
    socket_path: PathBuf,
    pre_seeded: Option<&str>,
) -> (oneshot::Sender<()>, JoinHandle<Result<(), ServerError>>) {
    let (tx, rx) = oneshot::channel::<()>();
    let cfg = ServerConfig {
        socket_path,
        pre_seeded_session: pre_seeded.map(str::to_owned),
        seed_with_pty: false,
        seed_command: None,
    };
    let handle = tokio::task::spawn_local(async move {
        let server = ServerRuntime::new(cfg);
        server
            .run_async(async move {
                let _ = rx.await;
            })
            .await
    });
    (tx, handle)
}

/// Like [`spawn_server`] but pre-seeds a PTY-backed pane running `cmd`.
/// Used by the `input_dispatch` test to drive a deterministic echo
/// fixture (`cat`) for wire→PTY round-trip assertions.
pub fn spawn_server_with_seed_cmd(
    socket_path: PathBuf,
    pre_seeded: &str,
    cmd: portable_pty::CommandBuilder,
) -> (oneshot::Sender<()>, JoinHandle<Result<(), ServerError>>) {
    let (tx, rx) = oneshot::channel::<()>();
    let cfg = ServerConfig {
        socket_path,
        pre_seeded_session: Some(pre_seeded.to_owned()),
        seed_with_pty: true,
        seed_command: Some(cmd),
    };
    let handle = tokio::task::spawn_local(async move {
        let server = ServerRuntime::new(cfg);
        server
            .run_async(async move {
                let _ = rx.await;
            })
            .await
    });
    (tx, handle)
}

/// Block on `fut` inside a fresh `current_thread` runtime + `LocalSet`.
/// Mirrors the byc.8 `attach_lifecycle` helper exactly so the wire
/// surface stays identical across tests.
pub fn run_local<F>(fut: F)
where
    F: Future<Output = ()>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, fut);
}

/// Poll `UnixStream::connect(path)` until success or the deadline expires.
/// `path.exists()` is racy with the "stale-socket unlink + bind" sequence
/// the server performs at startup; only an actual connect is race-free.
pub async fn wait_for_socket(path: &Path, deadline: Duration) -> UnixStream {
    let start = Instant::now();
    let mut last_err: Option<std::io::Error> = None;
    while start.elapsed() < deadline {
        match UnixStream::connect(path).await {
            Ok(s) => return s,
            Err(e) => last_err = Some(e),
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!(
        "socket {} never became connectable: last_err={:?}",
        path.display(),
        last_err,
    );
}

/// Read exactly one length-prefixed wire frame and return the full
/// framed bytes (4-byte BE header + body). Wrapped in
/// [`WIRE_RECV_TIMEOUT`]; panics on either timeout or I/O error so the
/// test fails loudly.
pub async fn recv_framed(stream: &mut UnixStream) -> Vec<u8> {
    timeout(WIRE_RECV_TIMEOUT, async {
        let mut header = [0u8; 4];
        stream.read_exact(&mut header).await.unwrap();
        let body_len = u32::from_be_bytes(header) as usize;
        let mut body = vec![0u8; body_len];
        stream.read_exact(&mut body).await.unwrap();
        let mut framed = Vec::with_capacity(4 + body_len);
        framed.extend_from_slice(&header);
        framed.extend_from_slice(&body);
        framed
    })
    .await
    .expect("timed out waiting for frame")
}

/// Decode one wire frame and return both the type byte (for type-level
/// assertions that don't want to match the full enum) and the decoded
/// [`FrameKind`].
pub async fn recv_typed(stream: &mut UnixStream) -> (u8, FrameKind) {
    let framed = recv_framed(stream).await;
    let type_byte = framed[4];
    let (frame, rest) = FrameKind::decode(&framed).expect("decode frame");
    assert!(rest.is_empty(), "decoder did not consume entire frame");
    (type_byte, frame)
}

/// Encode a [`FrameKind`] into a length-prefixed wire buffer.
pub fn encode_frame(frame: &FrameKind) -> BytesMut {
    let mut buf = BytesMut::new();
    frame.encode(&mut buf);
    buf
}

/// Build the canonical `ATTACH { ByName(name) }` used by the byc.6 tests.
/// 80x24 viewport, no scrollback requested — matches the byc.8 fixture so
/// the snapshot dimensions line up.
#[must_use]
pub fn attach_by_name(name: &str) -> FrameKind {
    FrameKind::Attach {
        target: AttachTarget::ByName(name.to_owned()),
        viewport: ViewportInfo::new(80, 24),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    }
}

/// Write a frame to the stream and flush. Convenience wrapper so each
/// test reads as a sequence of named protocol steps instead of a
/// `write_all` + `flush` pair.
pub async fn send_frame(stream: &mut UnixStream, frame: &FrameKind) {
    let buf = encode_frame(frame);
    stream.write_all(&buf).await.unwrap();
    stream.flush().await.unwrap();
}

/// Read with [`WIRE_RECV_TIMEOUT`] but expect EOF: returns `Ok(())` if
/// the next `read` yields `0` bytes (clean half-close), `Err` otherwise.
/// Used by the detach test to assert that the server has fully torn the
/// connection down once the client closes its write side.
pub async fn expect_eof_within(stream: &mut UnixStream, deadline: Duration) -> Result<(), String> {
    let mut buf = [0u8; 16];
    match timeout(deadline, stream.read(&mut buf)).await {
        Ok(Ok(0)) => Ok(()),
        Ok(Ok(n)) => Err(format!(
            "expected EOF, got {n} bytes (server still talking?)",
        )),
        Ok(Err(e)) => Err(format!("read error while expecting EOF: {e}")),
        Err(_) => Err(format!(
            "no EOF within {}ms; connection still open",
            deadline.as_millis(),
        )),
    }
}
