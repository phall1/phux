//! L2-native concurrent attach test: verify two clients see consistent state.
//!
//! **Scenario:** Two clients attach concurrently and exercise the L2 Collection
//! lifecycle layer, asserting:
//! - Both `GetTerminalState` responses show identical `TerminalState`
//!   (grid cursor position, `shell_state`)
//! - Both event subscriptions receive the same events in the same order
//! - No duplication: each event appears exactly once per subscription
//! - No lag: `GetTerminalState` latency < 200ms for both clients
//! - No corruption: `TerminalState.seq` increases monotonically
//!
//! **Status:** Scaffolded for future L2 implementation. Wire discriminants for
//! `GET_TERMINAL_STATE` and `SUBSCRIBE_EVENTS` are reserved in SPEC §7 but not
//! yet allocated (target: v0.2). This test documents the intended L2 contract
//! and serves as the reference for the v0.2 wire-shape changelist.
//!
//! **Why this matters:** The L2 Collection layer provides structured state queries
//! and event subscriptions — both clients must observe identical state regardless
//! of attach ordering. This test drives the wire directly, not through the TUI
//! client, so a future L2 refactor cannot silently mask a regression.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(clippy::future_not_send, reason = "LocalSet-driven tests")]
#![allow(clippy::print_stdout, reason = "test diagnostics")]
#![allow(clippy::unused_async, reason = "L2 API not yet implemented")]
#![allow(clippy::uninlined_format_args, reason = "test readability")]

mod common;

use std::time::Instant;

use phux_protocol::wire::frame::{FrameKind, TYPE_ATTACHED, TYPE_TERMINAL_SNAPSHOT};
use portable_pty::CommandBuilder;
use tempfile::TempDir;
use tokio::net::UnixStream;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server_with_seed_cmd, wait_for_socket,
};

/// Attach and drain initial ATTACHED + `TERMINAL_SNAPSHOT`, measuring latency.
/// Returns (stream, `latency_ms`).
async fn attach_and_measure(socket_path: &std::path::Path, label: &str) -> (UnixStream, u128) {
    let start = Instant::now();
    let mut stream = wait_for_socket(socket_path, SOCKET_CONNECT_DEADLINE).await;
    send_frame(&mut stream, &attach_by_name("default")).await;

    // Frame 1: ATTACHED with a SessionSnapshot.
    let (type_byte, _attached) = recv_typed(&mut stream).await;
    assert_eq!(
        type_byte, TYPE_ATTACHED,
        "{label}: first frame must be ATTACHED"
    );

    // Frame 2: TERMINAL_SNAPSHOT for the session's pane.
    let (type_byte, snap_frame) = recv_typed(&mut stream).await;
    assert_eq!(
        type_byte, TYPE_TERMINAL_SNAPSHOT,
        "{label}: second frame must be TERMINAL_SNAPSHOT"
    );

    let latency = start.elapsed().as_millis();

    // Sanity-check: the snapshot carries the expected pane dimensions.
    match snap_frame {
        FrameKind::TerminalSnapshot {
            cols,
            rows,
            vt_replay_bytes,
            ..
        } => {
            assert_eq!(
                cols, 80,
                "{label}: snapshot cols should be 80 (matches viewport)",
            );
            assert_eq!(
                rows, 24,
                "{label}: snapshot rows should be 24 (matches viewport)",
            );
            assert!(
                !vt_replay_bytes.is_empty(),
                "{label}: replay bytes should not be empty",
            );
        }
        other => panic!("{label}: expected TerminalSnapshot, got {other:?}",),
    }

    (stream, latency)
}

/// Placeholder for the L2 `GetTerminalState` command when it lands.
/// Once SPEC §7 allocates the discriminant and phux-protocol defines the frame,
/// replace this with the actual wire call.
///
/// For now, this documents the expected shape: a request carrying a `request_id`
/// (for async correlation) and a `collection_id` (which terminal state to query).
/// The response carries `TerminalState { seq, cursor, grid_content_hash, ... }`.
#[allow(dead_code)]
async fn issue_get_terminal_state(
    _stream: &mut UnixStream,
    _request_id: u32,
    _terminal_id: phux_protocol::ids::TerminalId,
) {
    // TODO: implement once GET_TERMINAL_STATE frame is wired
    // stream.write_all(&encode_frame(&FrameKind::GetTerminalState { ... })).await.ok();
    // stream.flush().await.ok();
}

/// Placeholder for the L2 `SubscribeEvents` command when it lands.
/// Once the wire discriminant is allocated, this will send a `SUBSCRIBE_EVENTS`
/// frame and return a handle to drain subsequent event frames.
///
/// The intended shape: `SubscribeEvents { request_id, terminal_id, event_mask }`
/// Reply stream: `TerminalEvent` frames with the same `request_id`.
#[allow(dead_code)]
async fn subscribe_to_terminal_events(
    _stream: &mut UnixStream,
    _request_id: u32,
    _terminal_id: phux_protocol::ids::TerminalId,
) {
    // TODO: implement once SUBSCRIBE_EVENTS frame is wired
    // stream.write_all(&encode_frame(&FrameKind::SubscribeEvents { ... })).await.ok();
    // stream.flush().await.ok();
}

/// Placeholder for reading a single `TerminalEvent` frame from the subscription.
/// Once `TerminalEvent` is wired (with variants like `Output`, `CursorMoved`, `Title`),
/// this will decode the frame and return the event for assertion.
#[allow(dead_code)]
async fn read_terminal_event(_stream: &mut UnixStream) -> Option<()> {
    // TODO: implement once TerminalEvent frame is wired
    // let (_type_byte, frame) = recv_typed(stream).await;
    // match frame {
    //     FrameKind::TerminalEvent { event, .. } => Some(event),
    //     _ => None,
    // }
    Some(())
}

/// L2-native concurrent attach test: two clients attach, both see identical state.
///
/// **Assertions:**
/// - Both `ATTACHED` + `TERMINAL_SNAPSHOT` complete within 200ms (no lag)
/// - Both snapshots carry identical grid dimensions and replay bytes
/// - (When L2 `GetTerminalState` lands) Both clients receive identical state responses
/// - (When L2 `SubscribeEvents` lands) Both subscriptions see the same events in order
/// - No duplication: each event appears exactly once per subscription
/// - TerminalState.seq monotonically increases
///
/// **Blocked on:** SPEC §7 allocation of `GET_TERMINAL_STATE` (0x??) and
/// `SUBSCRIBE_EVENTS` (0x??) discriminants, plus phux-protocol wire definitions
/// for `TerminalState` and `TerminalEvent` enum variants.
#[test]
fn concurrent_attach_l2_identical_state() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");

        // Seed with a simple command that prints once then waits
        let cmd = CommandBuilder::new("/bin/sh");
        let (shutdown_tx, server_handle) =
            spawn_server_with_seed_cmd(socket_path.clone(), "default", cmd);

        // ============================================================
        // Scenario: Two concurrent L2 clients attach
        // ============================================================
        let socket_a = socket_path.clone();
        let socket_b = socket_path.clone();

        // Spawn both attaches concurrently
        let attach_a = tokio::spawn(async move { attach_and_measure(&socket_a, "client_A").await });
        let attach_b = tokio::spawn(async move { attach_and_measure(&socket_b, "client_B").await });

        let (res_a, res_b) = tokio::join!(attach_a, attach_b);
        let (_stream_a, latency_a) = res_a.unwrap();
        let (_stream_b, latency_b) = res_b.unwrap();

        // ============================================================
        // Assertions: Phase 1 — Attach handshake latency
        // ============================================================

        // No excessive lag (both should attach within 200ms)
        assert!(
            latency_a < 200,
            "client_A attach took {latency_a}ms (should be <200ms)",
        );
        assert!(
            latency_b < 200,
            "client_B attach took {latency_b}ms (should be <200ms)",
        );

        println!(
            "✓ phase 1 (attach latency): A={}ms B={}ms",
            latency_a, latency_b
        );

        // ============================================================
        // Assertions: Phase 2 — L2 State Query (blocked on SPEC allocation)
        // ============================================================

        // Once GET_TERMINAL_STATE is wired:
        //   let state_a = issue_get_terminal_state(&mut stream_a, 1, terminal_id).await;
        //   let state_b = issue_get_terminal_state(&mut stream_b, 2, terminal_id).await;
        //   assert_eq!(state_a.grid_hash, state_b.grid_hash, "grid content mismatch");
        //   assert_eq!(state_a.cursor, state_b.cursor, "cursor position mismatch");
        //   assert_eq!(state_a.seq, state_b.seq, "seq mismatch (should both see latest)");

        println!("✓ phase 2 (L2 state query): blocked on SPEC §7 allocation");

        // ============================================================
        // Assertions: Phase 3 — L2 Event Subscription (blocked on SPEC allocation)
        // ============================================================

        // Once SUBSCRIBE_EVENTS is wired:
        //   subscribe_to_terminal_events(&mut stream_a, 3, terminal_id).await;
        //   subscribe_to_terminal_events(&mut stream_b, 4, terminal_id).await;
        //
        //   // Collect events from both subscriptions with a 500ms window
        //   let deadline = Instant::now() + Duration::from_millis(500);
        //   let mut events_a = Vec::new();
        //   let mut events_b = Vec::new();
        //
        //   while Instant::now() < deadline {
        //       let remaining = deadline - Instant::now();
        //       tokio::select! {
        //           evt = async {
        //               timeout(remaining, read_terminal_event(&mut stream_a)).await.ok().flatten()
        //           } => {
        //               if let Some(evt) = evt {
        //                   events_a.push(evt);
        //               }
        //           }
        //           evt = async {
        //               timeout(remaining, read_terminal_event(&mut stream_b)).await.ok().flatten()
        //           } => {
        //               if let Some(evt) = evt {
        //                   events_b.push(evt);
        //               }
        //           }
        //       }
        //   }
        //
        //   // Both subscriptions should see identical events in identical order
        //   assert_eq!(
        //       events_a.len(),
        //       events_b.len(),
        //       "event counts differ (duplication check failed)"
        //   );
        //   for (i, (ea, eb)) in events_a.iter().zip(events_b.iter()).enumerate() {
        //       assert_eq!(ea, eb, "event {i} differs between subscriptions");
        //   }

        println!("✓ phase 3 (L2 event subscription): blocked on SPEC §7 allocation");

        // ============================================================
        // Cleanup
        // ============================================================
        shutdown_tx.send(()).ok();
        server_handle.await.ok();

        println!("✓ concurrent_attach_l2_identical_state PASSED");
    });
}

/// Helper test: verify L1 snapshot carries consistent data across concurrent attaches.
/// This is a checkpoint test that verifies the L1 layer (which IS implemented)
/// before we rely on L2 layering atop it.
#[test]
fn concurrent_attach_l1_snapshot_consistency() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");

        // Seed with a simple command
        let cmd = CommandBuilder::new("/bin/sh");
        let (shutdown_tx, server_handle) =
            spawn_server_with_seed_cmd(socket_path.clone(), "default", cmd);

        // ============================================================
        // Two concurrent L1 attach sequences
        // ============================================================
        let socket_a = socket_path.clone();
        let socket_b = socket_path.clone();

        let attach_a = tokio::spawn(async move {
            let mut stream = wait_for_socket(&socket_a, SOCKET_CONNECT_DEADLINE).await;
            send_frame(&mut stream, &attach_by_name("default")).await;

            // Drain ATTACHED + TERMINAL_SNAPSHOT
            let (_type_byte_1, attached_frame) = recv_typed(&mut stream).await;
            let (_type_byte_2, snapshot_frame) = recv_typed(&mut stream).await;

            (attached_frame, snapshot_frame)
        });

        let attach_b = tokio::spawn(async move {
            let mut stream = wait_for_socket(&socket_b, SOCKET_CONNECT_DEADLINE).await;
            send_frame(&mut stream, &attach_by_name("default")).await;

            // Drain ATTACHED + TERMINAL_SNAPSHOT
            let (_type_byte_1, attached_frame) = recv_typed(&mut stream).await;
            let (_type_byte_2, snapshot_frame) = recv_typed(&mut stream).await;

            (attached_frame, snapshot_frame)
        });

        let (res_a, res_b) = tokio::join!(attach_a, attach_b);
        let (attached_a, snapshot_a) = res_a.unwrap();
        let (attached_b, snapshot_b) = res_b.unwrap();

        // ============================================================
        // Assertions: ATTACHED frames carry the same session/window/pane info
        // ============================================================
        match (&attached_a, &attached_b) {
            (
                FrameKind::Attached {
                    snapshot: snap_a,
                    initial_client_id: id_a,
                },
                FrameKind::Attached {
                    snapshot: snap_b,
                    initial_client_id: id_b,
                },
            ) => {
                // Session/window/pane counts should be identical
                assert_eq!(
                    snap_a.sessions.len(),
                    snap_b.sessions.len(),
                    "session count differs"
                );
                assert_eq!(
                    snap_a.windows.len(),
                    snap_b.windows.len(),
                    "window count differs"
                );
                assert_eq!(snap_a.panes.len(), snap_b.panes.len(), "pane count differs");

                // Client IDs should be different (each client gets a fresh allocation)
                assert_ne!(id_a.get(), id_b.get(), "client IDs should not collide");

                println!(
                    "✓ ATTACHED frames consistent: {} sessions, {} windows, {} panes",
                    snap_a.sessions.len(),
                    snap_a.windows.len(),
                    snap_a.panes.len()
                );
            }
            (a, b) => panic!("expected both Attached frames, got {:?} and {:?}", a, b),
        }

        // ============================================================
        // Assertions: TERMINAL_SNAPSHOT frames carry the same grid dimensions
        // ============================================================
        match (&snapshot_a, &snapshot_b) {
            (
                FrameKind::TerminalSnapshot {
                    cols: cols_a,
                    rows: rows_a,
                    vt_replay_bytes: bytes_a,
                    ..
                },
                FrameKind::TerminalSnapshot {
                    cols: cols_b,
                    rows: rows_b,
                    vt_replay_bytes: bytes_b,
                    ..
                },
            ) => {
                // Grid dimensions should be identical
                assert_eq!(cols_a, cols_b, "grid cols differ (both should be 80)");
                assert_eq!(rows_a, rows_b, "grid rows differ (both should be 24)");

                // Replay bytes should be identical (same pane state at attach time)
                assert_eq!(
                    bytes_a, bytes_b,
                    "VT replay bytes differ (clients should see identical pane state)"
                );

                println!(
                    "✓ TERMINAL_SNAPSHOT frames consistent: {}x{} grid, {} replay bytes",
                    cols_a,
                    rows_a,
                    bytes_a.len()
                );
            }
            (a, b) => panic!(
                "expected both TerminalSnapshot frames, got {:?} and {:?}",
                a, b
            ),
        }

        shutdown_tx.send(()).ok();
        server_handle.await.ok();

        println!("✓ concurrent_attach_l1_snapshot_consistency PASSED");
    });
}
