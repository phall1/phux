//! Wire-level integration tests for `AttachTarget::Last`.
//!
//! These tests exercise the SPEC §13 "most-recently-focused session"
//! resolution end-to-end:
//!
//! 1. `last_resolves_to_prior_attach`: ATTACH by name → success;
//!    then on a fresh connection, ATTACH with `Last` resolves to the
//!    same session and returns `ATTACHED`.
//! 2. `last_with_no_prior_returns_error`: ATTACH with `Last` against a
//!    fresh server (no prior attach) returns `ERROR { SessionNotFound }`.
//!    Per SPEC §13: "Implementations without prior-attach memory MAY
//!    return `SESSION_NOT_FOUND`" — we follow that allowance (until a
//!    dedicated `NoLastSession` `ErrorCode` lands; see the
//!    `TODO(error-codes)` in `runtime::resolve_attach_target`).
//!
//! Sibling of `byc_6_1_attach_snapshot.rs`; intentionally a separate
//! binary so 6.1's snapshot assertions stay isolated.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

mod common;

use phux_protocol::input::focus::FocusEvent;
use phux_protocol::wire::frame::{
    AttachTarget, ErrorCode, FrameKind, TYPE_ATTACHED, TYPE_ERROR, ViewportInfo,
};
use tempfile::TempDir;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, attach_by_name, recv_typed, run_local, send_frame, spawn_server,
    wait_for_socket,
};

/// Build an `ATTACH { Last }` with the same viewport/scrollback knobs
/// `attach_by_name` uses, so the two are otherwise wire-identical.
const fn attach_last() -> FrameKind {
    FrameKind::Attach {
        target: AttachTarget::Last,
        viewport: ViewportInfo::new(80, 24),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    }
}

fn attach_create_if_missing(name: &str) -> FrameKind {
    FrameKind::Attach {
        target: AttachTarget::CreateIfMissing {
            name: name.to_owned(),
            command: None,
            cwd: None,
        },
        viewport: ViewportInfo::new(80, 24),
        request_scrollback: false,
        scrollback_limit_lines: 0,
    }
}

/// Drain the per-attach response sequence: ATTACHED then one
/// `TERMINAL_SNAPSHOT` per pane in the focused window. The server pre-seeds
/// exactly one pane, so we read both frames and assert types.
async fn drain_successful_attach(
    stream: &mut tokio::net::UnixStream,
    expected_name: &str,
) -> phux_protocol::ids::TerminalId {
    let (type_byte, frame) = recv_typed(stream).await;
    assert_eq!(
        type_byte, TYPE_ATTACHED,
        "first server-to-client frame must be ATTACHED (got 0x{type_byte:02x})",
    );
    let focused_pane = match frame {
        FrameKind::Attached { snapshot, .. } => {
            let focused = snapshot
                .sessions
                .iter()
                .find(|session| session.id == snapshot.focused_session)
                .expect("focused session must be listed in snapshot");
            assert_eq!(focused.name, expected_name, "focused session name");
            snapshot.focused_pane
        }
        other => panic!("expected FrameKind::Attached, got {other:?}"),
    };

    // One TERMINAL_SNAPSHOT follows. We don't inspect its body; the
    // round-trip / dim assertions live in byc_6_1. Just consume it so
    // subsequent reads see a clean stream boundary.
    let (_type_byte, _snap_frame) = recv_typed(stream).await;
    focused_pane
}

async fn round_trip_ping(stream: &mut tokio::net::UnixStream, nonce: u64) {
    send_frame(stream, &FrameKind::Ping { nonce }).await;
    let (_type_byte, frame) = recv_typed(stream).await;
    assert_eq!(frame, FrameKind::Pong { nonce }, "PING must receive PONG");
}

#[test]
fn last_resolves_to_prior_attach() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), Some("default"));

        // First connection: ATTACH by name. This populates the
        // server-side last-touched session order.
        {
            let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
            send_frame(&mut stream, &attach_by_name("default")).await;
            drain_successful_attach(&mut stream, "default").await;
            drop(stream);
        }

        // Second connection from scratch: ATTACH with Last must
        // resolve to the same session and complete successfully.
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream, &attach_last()).await;
        drain_successful_attach(&mut stream, "default").await;

        drop(stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}

#[test]
fn last_resolves_to_most_recently_focused_not_last_attached() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), Some("default"));

        let mut default_stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut default_stream, &attach_by_name("default")).await;
        let default_pane = drain_successful_attach(&mut default_stream, "default").await;

        let mut other_stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut other_stream, &attach_create_if_missing("other")).await;
        drain_successful_attach(&mut other_stream, "other").await;

        send_frame(
            &mut default_stream,
            &FrameKind::InputFocus {
                terminal_id: default_pane,
                event: FocusEvent::Gained,
            },
        )
        .await;
        round_trip_ping(&mut default_stream, 0xA11C_E5ED).await;

        let mut last_stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut last_stream, &attach_last()).await;
        drain_successful_attach(&mut last_stream, "default").await;

        drop(last_stream);
        drop(other_stream);
        drop(default_stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}

#[test]
fn last_with_no_prior_returns_error() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        // Pre-seed a session so the registry isn't empty — we want to
        // prove that `Last` errors specifically because no client has
        // attached yet, not because no session exists.
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), Some("default"));

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream, &attach_last()).await;

        let (type_byte, frame) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_ERROR,
            "first frame must be ERROR (got 0x{type_byte:02x})",
        );
        match frame {
            FrameKind::Error {
                request_id,
                code,
                message,
            } => {
                assert!(
                    request_id.is_none(),
                    "ATTACH errors must not carry a request_id",
                );
                // TODO(error-codes): expect a dedicated
                // ErrorCode::NoLastSession once it lands; for now we
                // pin SPEC §13's "MAY return SESSION_NOT_FOUND" path.
                assert_eq!(
                    code,
                    ErrorCode::SessionNotFound,
                    "AttachTarget::Last with no prior must currently surface SessionNotFound",
                );
                assert!(
                    message.to_lowercase().contains("prior")
                        || message.to_lowercase().contains("last"),
                    "error message must hint at the 'no last session' diagnosis, got: {message:?}",
                );
            }
            other => panic!("expected FrameKind::Error, got {other:?}"),
        }

        drop(stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}
