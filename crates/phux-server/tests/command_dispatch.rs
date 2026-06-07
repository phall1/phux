//! Server-side dispatch for the generic `COMMAND` envelope (SPEC §5,
//! phux-k61 / ADR-0021).
//!
//! Covers the v0.1 commands the CLI control verbs ride:
//!
//! 1. **GET_STATE** → `COMMAND_RESULT { Ok_With(State(snapshot)) }` whose
//!    `sessions` list names the seeded session (this is what `phux ls`
//!    reads, and what client-side selector resolution walks).
//! 2. **GET_SCREEN on a live pane** → `COMMAND_RESULT { Ok_With(Json(..)) }`
//!    carrying a `ScreenState` whose pane id and dims match the target —
//!    the side-effect-free agent read (ADR-0022 §5). Plus the unknown-id
//!    `TerminalNotFound` path.
//! 3. **KILL_TERMINAL on an unknown id** → `COMMAND_RESULT { Error(
//!    TerminalNotFound, …) }`.
//! 4. **KILL_TERMINAL on a live pane** → `COMMAND_RESULT { Ok }` plus the
//!    asynchronous `TERMINAL_CLOSED` the reap path emits. Because the
//!    seeded session is the server's only one, the kill also triggers the
//!    tmux-model self-exit (phux-60s), so the test tolerates the
//!    connection closing.
//! 5. **KILL_TERMINALS** → `COMMAND_RESULT { Ok }` atomically tearing down a
//!    multi-terminal group in one round-trip (the v0.3.0 "Option B" re-tier
//!    op that replaced KILL_COLLECTION; ADR-0019 / ADR-0027), plus the
//!    unknown-id no-op (idempotent) path.
//! 6. **Session create / rename via L3 metadata** — the v0.3.0 replacements
//!    for the dissolved CREATE_SESSION / RENAME_SESSION verbs: a
//!    `SESSION_CREATE_KEY` write seeds a session and publishes its seed-pane
//!    id under `SESSION_CREATE_RESULT_KEY`; a `SESSION_NAME_KEY` write
//!    renames a session (GET_STATE reflects the new name).

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(
    clippy::doc_markdown,
    reason = "test narrative uses bare wire-frame names (COMMAND, GET_STATE, …) for symmetry with sibling tests"
)]

mod common;

use std::time::Duration;

use phux_protocol::ids::{GroupId, TerminalId};
use phux_protocol::wire::frame::{
    Command, CommandResult, CommandValue, ErrorCode, FrameKind, StateScope, TYPE_COMMAND_RESULT,
    TYPE_TERMINAL_CLOSED,
};
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, WIRE_RECV_TIMEOUT, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server, spawn_server_seed_pty_no_cmd, try_recv_typed, wait_for_socket,
};

/// Drain frames until a `COMMAND_RESULT` with `request_id` arrives.
async fn await_command_result(stream: &mut UnixStream, request_id: u32) -> CommandResult {
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_COMMAND_RESULT {
            continue;
        }
        if let FrameKind::CommandResult {
            request_id: got,
            result,
        } = frame
            && got == request_id
        {
            return result;
        }
    }
    panic!("no COMMAND_RESULT with request_id={request_id} within deadline");
}

#[test]
fn get_state_lists_the_seeded_session() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 1,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 1).await;
        match result {
            CommandResult::OkWith(CommandValue::State(snapshot)) => {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                assert!(
                    names.contains(&"work"),
                    "GET_STATE snapshot must list the seeded session; got {names:?}",
                );
            }
            other => panic!("expected Ok_With(State(..)), got {other:?}"),
        }
    });
}

#[test]
fn get_screen_returns_structured_screen_for_live_pane() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Attach to learn a real wire terminal id + its dims.
        send_frame(&mut stream, &attach_by_name("work")).await;
        let (pane_id, cols, rows) = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                let p = &snapshot.panes[0];
                break (p.id.clone(), p.cols, p.rows);
            }
        };

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 5,
                command: Command::GetScreen {
                    terminal_id: pane_id.clone(),
                    request_scrollback: None,
                    cells: false,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 5).await;
        match result {
            CommandResult::OkWith(CommandValue::Json(json)) => {
                let screen: phux_core::screen::ScreenState = serde_json::from_str(&json)
                    .expect("GET_SCREEN reply must be valid ScreenState");
                assert_eq!(screen.schema_version, phux_core::screen::SCHEMA_VERSION);
                assert_eq!(
                    screen.pane,
                    pane_id.local_id().unwrap(),
                    "projected pane id must match the requested terminal",
                );
                assert_eq!((screen.cols, screen.rows), (cols, rows));
                assert_eq!(
                    screen.lines.len(),
                    usize::from(rows),
                    "one line per grid row",
                );
                assert!(
                    screen.scrollback.is_empty(),
                    "no scrollback requested -> empty scrollback (phux-o1v)",
                );
                assert!(
                    screen.cells.is_none(),
                    "no cells requested -> cells None (phux-8yl)",
                );
            }
            other => panic!("expected Ok_With(Json(..)), got {other:?}"),
        }
    });
}

/// Wire-level GET_SCREEN with `cells: true` must drive the production read
/// loop (`handle_client`) all the way to a `ScreenState` whose `cells`
/// field is `Some(..)` rather than `None` (`phux-8yl`). This proves the
/// flag threads from the decoded `Command::GetScreen` through dispatch ->
/// `ScreenRequest` -> the grid walk; the per-cell *content* (semantic
/// marks, styles) is exercised by the grid.rs unit tests against a
/// Terminal seeded with `vt_write`, where the grid is deterministic.
#[test]
fn get_screen_with_cells_requests_cell_projection() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(&mut stream, &attach_by_name("work")).await;
        let pane_id = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                break snapshot.panes[0].id.clone();
            }
        };

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 21,
                command: Command::GetScreen {
                    terminal_id: pane_id.clone(),
                    request_scrollback: None,
                    cells: true,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 21).await;
        match result {
            CommandResult::OkWith(CommandValue::Json(json)) => {
                let screen: phux_core::screen::ScreenState = serde_json::from_str(&json)
                    .expect("GET_SCREEN reply must be valid ScreenState");
                assert!(
                    screen.cells.is_some(),
                    "cells: true must populate Some(..) through dispatch (phux-8yl)",
                );
            }
            other => panic!("expected Ok_With(Json(..)), got {other:?}"),
        }
    });
}

#[test]
fn get_screen_unknown_id_returns_terminal_not_found() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 8,
                command: Command::GetScreen {
                    terminal_id: TerminalId::local(99_999),
                    request_scrollback: None,
                    cells: false,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 8).await;
        match result {
            CommandResult::Error { code, .. } => assert_eq!(code, ErrorCode::TerminalNotFound),
            other => panic!("expected Error(TerminalNotFound), got {other:?}"),
        }
    });
}

#[test]
fn kill_terminal_unknown_id_returns_terminal_not_found() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 7,
                command: Command::KillTerminal {
                    // A wire id the server never allocated.
                    terminal_id: TerminalId::local(99_999),
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 7).await;
        match result {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::TerminalNotFound);
            }
            other => panic!("expected Error(TerminalNotFound), got {other:?}"),
        }
    });
}

#[test]
fn kill_terminal_live_pane_acks_and_closes() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Attach to learn a real wire terminal id from the snapshot.
        send_frame(&mut stream, &attach_by_name("work")).await;
        let pane_id = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                break snapshot.panes[0].id.clone();
            }
        };

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 3,
                command: Command::KillTerminal {
                    terminal_id: pane_id.clone(),
                },
            },
        )
        .await;

        // The Ok ack and an async TERMINAL_CLOSED may arrive in either
        // order (SPEC §5). Collect both, tolerating the server's self-exit
        // close once its only session is reaped.
        let mut saw_ok = false;
        let mut saw_closed = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !(saw_ok && saw_closed) && tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Ok(maybe) = timeout(remaining, try_recv_typed(&mut stream)).await else {
                break;
            };
            let Some((type_byte, frame)) = maybe else {
                break; // server self-exited and closed the connection
            };
            match (type_byte, frame) {
                (
                    _,
                    FrameKind::CommandResult {
                        request_id: 3,
                        result: CommandResult::Ok,
                    },
                ) => {
                    saw_ok = true;
                }
                (TYPE_TERMINAL_CLOSED, FrameKind::TerminalClosed { terminal_id, .. })
                    if terminal_id == pane_id =>
                {
                    saw_closed = true;
                }
                _ => {}
            }
        }
        assert!(saw_ok, "KILL_TERMINAL must ack with COMMAND_RESULT::Ok");
        assert!(
            saw_closed,
            "KILL_TERMINAL must drive TERMINAL_CLOSED for the pane"
        );
    });
}

/// **KILL_TERMINALS** atomically tears down a multi-terminal group in ONE
/// round-trip — the irreducible op the v0.3.0 "Option B" re-tier (ADR-0019 /
/// ADR-0027) put in place of the dissolved KILL_COLLECTION verb. The test
/// attaches to "work", adds a second pane via SPAWN_TERMINAL so the session
/// owns two Terminals, then KILL_TERMINALS both ids and asserts the `Ok` ack
/// plus a TERMINAL_CLOSED for *each* pane. The whole path rides
/// `handle_client` (the production read loop).
#[test]
fn kill_terminals_tears_down_a_multi_terminal_group_atomically() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) =
            spawn_server_seed_pty_no_cmd(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Attach to learn the seed pane id and to satisfy SPAWN_TERMINAL's
        // "spawning client must be attached" precondition.
        send_frame(&mut stream, &attach_by_name("work")).await;
        let pane_a = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                break snapshot.panes[0].id.clone();
            }
        };

        // Add a second pane to the same session.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 40,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;
        let _ = await_command_result(&mut stream, 40).await;
        send_frame(
            &mut stream,
            &FrameKind::SpawnTerminal {
                request_id: 41,
                group: GroupId::new(1),
                command: None,
                cwd: None,
                env: None,
            },
        )
        .await;
        let pane_b = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::TerminalSpawned {
                request_id: 41,
                result,
            } = frame
            {
                match result {
                    phux_protocol::wire::frame::SpawnResult::Ok(id) => break id,
                    other => panic!("SPAWN_TERMINAL failed: {other:?}"),
                }
            }
        };
        assert_ne!(pane_a, pane_b, "the two panes must be distinct");

        // Atomic teardown of BOTH panes in one round-trip.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 42,
                command: Command::KillTerminals {
                    ids: vec![pane_a.clone(), pane_b.clone()],
                },
            },
        )
        .await;

        // Expect the Ok ack plus a TERMINAL_CLOSED for each pane (any order;
        // tolerate the server's self-exit close once its only session reaps).
        let mut saw_ok = false;
        let mut closed_a = false;
        let mut closed_b = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !(saw_ok && closed_a && closed_b) && tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Ok(maybe) = timeout(remaining, try_recv_typed(&mut stream)).await else {
                break;
            };
            let Some((type_byte, frame)) = maybe else {
                break; // server self-exited and closed the connection
            };
            match (type_byte, frame) {
                (
                    _,
                    FrameKind::CommandResult {
                        request_id: 42,
                        result: CommandResult::Ok,
                    },
                ) => saw_ok = true,
                (TYPE_TERMINAL_CLOSED, FrameKind::TerminalClosed { terminal_id, .. }) => {
                    if terminal_id == pane_a {
                        closed_a = true;
                    } else if terminal_id == pane_b {
                        closed_b = true;
                    }
                }
                _ => {}
            }
        }
        assert!(saw_ok, "KILL_TERMINALS must ack with COMMAND_RESULT::Ok");
        assert!(closed_a, "KILL_TERMINALS must close the first pane");
        assert!(closed_b, "KILL_TERMINALS must close the second pane");
    });
}

/// **KILL_TERMINALS with an unknown / already-dead id is a no-op**, not an
/// error: the op is idempotent so a caller racing a natural exit still
/// succeeds. A list mixing one live pane and one bogus id acks `Ok` and
/// closes only the live pane.
#[test]
fn kill_terminals_skips_unknown_ids() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(&mut stream, &attach_by_name("work")).await;
        let pane = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                break snapshot.panes[0].id.clone();
            }
        };

        // One live id + one id that does not exist.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 45,
                command: Command::KillTerminals {
                    ids: vec![pane.clone(), TerminalId::local(999_999)],
                },
            },
        )
        .await;

        let mut saw_ok = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while !saw_ok && tokio::time::Instant::now() < deadline {
            let remaining = deadline - tokio::time::Instant::now();
            let Ok(maybe) = timeout(remaining, try_recv_typed(&mut stream)).await else {
                break;
            };
            let Some((_t, frame)) = maybe else { break };
            if matches!(
                frame,
                FrameKind::CommandResult {
                    request_id: 45,
                    result: CommandResult::Ok,
                }
            ) {
                saw_ok = true;
            }
        }
        assert!(
            saw_ok,
            "KILL_TERMINALS with an unknown id must still ack Ok (idempotent)"
        );
    });
}

/// Session create-without-attach via the conventional `SESSION_CREATE_KEY`
/// L3 metadata write (the v0.3.0 replacement for CREATE_SESSION). The server
/// seeds the session + pane and publishes the seed-pane id under
/// `SESSION_CREATE_RESULT_KEY`; a fresh GET_STATE lists the new session and a
/// GET_METADATA on the result key returns the id.
#[test]
fn session_create_via_metadata_seeds_session_and_publishes_id() {
    run_local(async {
        use phux_protocol::wire::frame::{SESSION_CREATE_KEY, SESSION_CREATE_RESULT_KEY, Scope};
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        let value = serde_json::to_vec(&serde_json::json!({
            "name": "scratch",
            "command": serde_json::Value::Null,
            "cwd": serde_json::Value::Null,
        }))
        .unwrap();
        send_frame(
            &mut stream,
            &FrameKind::SetMetadata {
                request_id: 1,
                scope: Scope::Global,
                key: SESSION_CREATE_KEY.to_owned(),
                value,
            },
        )
        .await;

        // GET_STATE must list the new session.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 2,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 2).await {
            CommandResult::OkWith(CommandValue::State(snapshot)) => {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                assert!(
                    names.contains(&"scratch"),
                    "session-create must register the session; got {names:?}",
                );
            }
            other => panic!("expected Ok_With(State(..)), got {other:?}"),
        }

        // The result key carries {name, terminal_id} for the created session.
        send_frame(
            &mut stream,
            &FrameKind::GetMetadata {
                request_id: 3,
                scope: Scope::Global,
                key: SESSION_CREATE_RESULT_KEY.to_owned(),
            },
        )
        .await;
        let result = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::MetadataValue {
                request_id: 3,
                value,
            } = frame
            {
                break value;
            }
        };
        let bytes = result.expect("result key must be present after a successful create");
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json.get("name").and_then(|v| v.as_str()), Some("scratch"));
        assert!(
            json.get("terminal_id")
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "result must carry a terminal_id; got {json:?}",
        );
    });
}

/// Session rename via the conventional `SESSION_NAME_KEY` L3 metadata write
/// (the v0.3.0 replacement for RENAME_SESSION). The server intercepts the
/// `current\0new` value and applies the registry rename; a fresh GET_STATE
/// reflects the new name.
#[test]
fn session_rename_via_metadata_updates_registry_name() {
    run_local(async {
        use phux_protocol::wire::frame::{SESSION_NAME_KEY, Scope};
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        let mut value = b"work".to_vec();
        value.push(0);
        value.extend_from_slice(b"renamed");
        send_frame(
            &mut stream,
            &FrameKind::SetMetadata {
                request_id: 1,
                scope: Scope::Global,
                key: SESSION_NAME_KEY.to_owned(),
                value,
            },
        )
        .await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 2,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 2).await {
            CommandResult::OkWith(CommandValue::State(snapshot)) => {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                assert!(
                    names.contains(&"renamed") && !names.contains(&"work"),
                    "session-rename must reflect the new name in GET_STATE; got {names:?}",
                );
            }
            other => panic!("expected Ok_With(State(..)), got {other:?}"),
        }
    });
}

/// **GET_TERMINAL_STATE** on a live pane returns a structured `TerminalState`
/// JSON object with grid dimensions, cells, cursor, scrollback, shell state,
/// and metadata (sequence number, timestamp).
#[test]
fn get_terminal_state_returns_structured_snapshot_for_live_pane() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Attach to learn a real wire terminal id.
        send_frame(&mut stream, &attach_by_name("work")).await;
        let pane_id = loop {
            let (_t, frame) = recv_typed(&mut stream).await;
            if let FrameKind::Attached { snapshot, .. } = frame {
                break snapshot.panes[0].id.clone();
            }
        };

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 10,
                command: Command::GetTerminalState {
                    terminal_id: pane_id.clone(),
                    include_scrollback: false,
                    max_scrollback_lines: 0,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 10).await;
        match result {
            CommandResult::OkWith(CommandValue::Json(json)) => {
                // Parse as a generic JSON object to verify structure.
                let obj: serde_json::Value = serde_json::from_str(&json)
                    .expect("GET_TERMINAL_STATE reply must be valid JSON");

                // Verify TerminalState contract fields.
                assert!(obj.get("cols").is_some(), "TerminalState must include cols");
                assert!(obj.get("rows").is_some(), "TerminalState must include rows");
                assert!(
                    obj.get("cells").is_some(),
                    "TerminalState must include cells"
                );
                assert!(
                    obj.get("cursor").is_some(),
                    "TerminalState must include cursor"
                );
                assert!(
                    obj.get("scrollback").is_some(),
                    "TerminalState must include scrollback"
                );
                assert!(
                    obj.get("scrollback_count_total").is_some(),
                    "TerminalState must include scrollback_count_total"
                );
                assert!(
                    obj.get("shell_state").is_some(),
                    "TerminalState must include shell_state"
                );
                assert!(
                    obj.get("timestamp_secs").is_some(),
                    "TerminalState must include timestamp_secs"
                );
                assert!(obj.get("seq").is_some(), "TerminalState must include seq");
            }
            other => panic!("expected Ok_With(Json(..)), got {other:?}"),
        }
    });
}

/// **GET_TERMINAL_STATE** on an unknown terminal_id is rejected with
/// `TERMINAL_NOT_FOUND` error.
#[test]
fn get_terminal_state_unknown_terminal_returns_not_found_error() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Send GET_TERMINAL_STATE with a made-up terminal_id that doesn't exist.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 11,
                command: Command::GetTerminalState {
                    terminal_id: TerminalId::local(9999),
                    include_scrollback: false,
                    max_scrollback_lines: 0,
                },
            },
        )
        .await;

        let result = await_command_result(&mut stream, 11).await;
        match result {
            CommandResult::Error { code, message } => {
                assert_eq!(code, ErrorCode::TerminalNotFound);
                assert!(
                    message.contains("no such terminal"),
                    "Error message: {message}"
                );
            }
            other => panic!("expected Error(TerminalNotFound, ..), got {other:?}"),
        }
    });
}
