//! `phux-k61.3` — `AttachTarget::CreateIfMissing` wire integration.
//!
//! Three scenarios pin the behavior added by phux-k61.3:
//!
//! 1. **Create on miss.** Server starts with no sessions. A client
//!    sends `ATTACH { CreateIfMissing { name: "foo", … } }`. The
//!    server creates `foo` (one window, one pane) and replies with
//!    `ATTACHED` whose `SessionSnapshot` carries that single session,
//!    then a `TERMINAL_SNAPSHOT` for the seed pane.
//!
//! 2. **Reuse on hit.** Server starts with `existing` pre-seeded
//!    (one window, one pane). The same `CreateIfMissing { name:
//!    "existing", … }` attaches to the pre-existing session and does
//!    not create a duplicate — the snapshot still lists exactly one
//!    session named `existing`.
//!
//! 3. **`CreateIfMissing` with empty name == empty.** The wire schema
//!    pins `name: String` (not `Option<String>`), so the "default
//!    name" semantics live on the *client* (naked `phux` chooses
//!    `"default"` per `crates/phux/src/main.rs:46`). Exercising
//!    "default name" at the server boundary means sending the literal
//!    string `"default"`; we cover that under (1) by parameterising
//!    one extra subcase.
//!
//! All three scenarios run in the same test binary because they share
//! the helper plumbing and the wire `recv` shape — see
//! `byc_6_6_attach_unknown_session_error.rs` for the precedent of
//! one-binary-per-ticket with multiple `#[test]` functions.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

mod common;

use phux_protocol::wire::frame::{
    AttachTarget, FrameKind, TYPE_ATTACHED, TYPE_TERMINAL_SNAPSHOT, ViewportInfo,
};
use tempfile::TempDir;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, recv_typed, run_local, send_frame, spawn_server, wait_for_socket,
};

/// Canonical 80x24 viewport, matching the byc.6.* fixtures so snapshot
/// dimensions line up with the no-PTY seed actor's defaults.
fn create_if_missing_frame(name: &str) -> FrameKind {
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

#[test]
fn create_if_missing_creates_session_when_absent() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        // No pre-seeded session: CreateIfMissing must fill the gap.
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), None);

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream, &create_if_missing_frame("foo")).await;

        // ---- ATTACHED ----
        let (type_byte, attached) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_ATTACHED,
            "expected ATTACHED (got 0x{type_byte:02x})",
        );
        match attached {
            FrameKind::Attached {
                snapshot,
                initial_client_id,
            } => {
                assert_eq!(snapshot.sessions.len(), 1, "exactly one session");
                assert_eq!(
                    snapshot.sessions[0].name, "foo",
                    "CreateIfMissing must create the named session",
                );
                assert_eq!(snapshot.windows.len(), 1, "one seed window");
                assert_eq!(snapshot.panes.len(), 1, "one seed pane");
                assert!(
                    initial_client_id.get() >= 1,
                    "initial_client_id must be allocated (monotonic from 1)",
                );
            }
            other => panic!("expected Attached, got {other:?}"),
        }

        // ---- TERMINAL_SNAPSHOT for the seed pane ----
        let (type_byte, snap_frame) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_TERMINAL_SNAPSHOT,
            "expected TERMINAL_SNAPSHOT after ATTACHED (got 0x{type_byte:02x})",
        );
        match snap_frame {
            FrameKind::TerminalSnapshot { cols, rows, .. } => {
                // seed_session_with_actor seeds 80x24 — matches the byc.6.1
                // expectation. The dimension assertion is what proves the
                // seed pane actually got created (otherwise we'd never
                // reach this frame; see `prepare_attach` collecting
                // panes_to_snapshot from `session.windows`).
                assert_eq!(cols, 80, "seed pane defaults to 80 cols");
                assert_eq!(rows, 24, "seed pane defaults to 24 rows");
            }
            other => panic!("expected TerminalSnapshot, got {other:?}"),
        }

        drop(stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}

#[test]
fn create_if_missing_uses_default_name_string() {
    // The naked-phux dispatcher (`crates/phux/src/main.rs:46
    // DEFAULT_SESSION_NAME = "default"`) sends the literal string
    // `"default"` when no `-s NAME` is given. Pin the server's
    // round-trip on that exact value so a refactor of either side
    // notices the contract change.
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), None);

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream, &create_if_missing_frame("default")).await;

        let (_type_byte, attached) = recv_typed(&mut stream).await;
        match attached {
            FrameKind::Attached { snapshot, .. } => {
                assert_eq!(snapshot.sessions.len(), 1);
                assert_eq!(
                    snapshot.sessions[0].name, "default",
                    "CreateIfMissing with the naked-phux default name must round-trip",
                );
            }
            other => panic!("expected Attached, got {other:?}"),
        }
        // Drain the trailing TERMINAL_SNAPSHOT so the writer task isn't
        // blocked on its mailbox when the server tears down.
        let _ = recv_typed(&mut stream).await;

        drop(stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}

#[test]
fn create_if_missing_attaches_to_existing_session_without_duplicating() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        // Pre-seed `existing` so CreateIfMissing hits the fast path.
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), Some("existing"));

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream, &create_if_missing_frame("existing")).await;

        let (type_byte, attached) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_ATTACHED,
            "expected ATTACHED (got 0x{type_byte:02x})",
        );
        match attached {
            FrameKind::Attached { snapshot, .. } => {
                // Critical: exactly ONE session named "existing". If the
                // server had created a second "existing" alongside the
                // pre-seed, the snapshot would carry two SessionInfo
                // entries (the registry has no name-uniqueness gate of
                // its own).
                assert_eq!(
                    snapshot.sessions.len(),
                    1,
                    "CreateIfMissing must not duplicate an existing session",
                );
                assert_eq!(snapshot.sessions[0].name, "existing");
                // And exactly the pre-seed's single window+pane — no
                // extra resources allocated.
                assert_eq!(snapshot.windows.len(), 1, "one window");
                assert_eq!(snapshot.panes.len(), 1, "one pane");
            }
            other => panic!("expected Attached, got {other:?}"),
        }
        let _ = recv_typed(&mut stream).await;

        drop(stream);
        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}
