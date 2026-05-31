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
//! 5. **CREATE_SESSION** → `COMMAND_RESULT { Ok_With(TerminalId(..)) }`
//!    creating a named session under the default collection *without*
//!    attaching (the returned id resolves to a live seed pane, GET_STATE
//!    lists the new session). Plus the duplicate-name and unknown-collection
//!    refusals (`phux-fdh`, ADR-0021 §3), and the non-empty-seed-command
//!    path — the wire `command` actually runs in the seed pane (`phux-rhh`).
//! 6. **KILL_COLLECTION** → `COMMAND_RESULT { Ok }` tearing down a whole
//!    named session in one round-trip; a fresh GET_STATE no longer lists it.
//!    Plus the unknown-session (`SessionNotFound`) and unknown-collection
//!    (`InvalidCommand`) refusals (`phux-h9s`, ADR-0021 §3).

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(
    clippy::doc_markdown,
    reason = "test narrative uses bare wire-frame names (COMMAND, GET_STATE, …) for symmetry with sibling tests"
)]

mod common;

use std::time::Duration;

use phux_protocol::ids::{CollectionId, TerminalId};
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

/// CREATE_SESSION creates a named session under the default collection
/// without attaching, and the reply carries the seed pane's id. The whole
/// path rides `handle_client` (the production read loop) — the test only
/// speaks wire bytes. Driving it through `handle_client` (not
/// `handle_create_session` directly) is the house rule for wire→dispatch
/// coverage.
#[test]
fn create_session_creates_without_attaching_and_returns_seed_pane() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 9,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "scratch".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;

        let created = match await_command_result(&mut stream, 9).await {
            CommandResult::OkWith(CommandValue::TerminalId(id)) => id,
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        };

        // The new session shows up in a fresh snapshot — proof the create
        // happened server-side, with no attach (this client never sent
        // ATTACH and never received an ATTACHED frame).
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 10,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 10).await {
            CommandResult::OkWith(CommandValue::State(snapshot)) => {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                assert!(
                    names.contains(&"scratch"),
                    "CREATE_SESSION must register the session; got {names:?}",
                );
            }
            other => panic!("expected Ok_With(State(..)), got {other:?}"),
        }

        // The returned id resolves to a live pane: GET_SCREEN on it succeeds
        // (the side-effect-free read path needs a real actor behind the id).
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 11,
                command: Command::GetScreen {
                    terminal_id: created.clone(),
                    request_scrollback: None,
                    cells: false,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 11).await {
            CommandResult::OkWith(CommandValue::Json(_)) => {}
            other => panic!("seed pane id must back a live pane; got {other:?}"),
        }
    });
}

/// CREATE_SESSION run twice with distinct names yields two distinct seed
/// panes — the always-new guarantee, without a client-side GET_STATE round
/// trip. Distinct ids prove no silent reuse.
#[test]
fn create_session_twice_yields_distinct_panes() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 1,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "a".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        let first = match await_command_result(&mut stream, 1).await {
            CommandResult::OkWith(CommandValue::TerminalId(id)) => id,
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        };

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 2,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "b".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        let second = match await_command_result(&mut stream, 2).await {
            CommandResult::OkWith(CommandValue::TerminalId(id)) => id,
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        };

        assert_ne!(
            first, second,
            "two CREATE_SESSION calls must seed distinct panes"
        );
    });
}

/// CREATE_SESSION with a name already in use is refused — create-only, never
/// create-or-attach (unlike `ATTACH { CreateIfMissing }`).
#[test]
fn create_session_duplicate_name_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 4,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "work".to_owned(), // the seeded session's name
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 4).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::InvalidCommand);
            }
            other => panic!("expected Error(InvalidCommand), got {other:?}"),
        }
    });
}

/// CREATE_SESSION under an unknown collection is refused; v0.1 servers host
/// only the default `CollectionId(1)`.
#[test]
fn create_session_unknown_collection_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 6,
                command: Command::CreateSession {
                    collection: CollectionId::new(99),
                    name: "other".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 6).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::InvalidCommand);
            }
            other => panic!("expected Error(InvalidCommand), got {other:?}"),
        }
    });
}

/// CREATE_SESSION carrying a NON-EMPTY seed command actually runs that
/// command in the seed pane (`phux-rhh`). The server is configured with a
/// real PTY but no server-wide override command, so the wire `command`
/// takes effect; the fixture writes a deterministic marker, which a poll of
/// GET_SCREEN on the returned seed-pane id must observe. The whole path
/// rides `handle_client` (the production read loop) — the test speaks only
/// wire bytes — closing the Q5 coverage gap a prior verify flagged (the
/// existing CREATE_SESSION tests only exercised the default-seed path).
#[test]
fn create_session_runs_non_empty_seed_command() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        // PTY-backed seeds, but no override command: the wire `command`
        // below is what the seed pane execs.
        let (_shutdown_tx, _server) =
            spawn_server_seed_pty_no_cmd(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Deterministic fixture: print a marker, then idle so the pane stays
        // live long enough for the GET_SCREEN poll to sample it.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 40,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "seeded".to_owned(),
                    command: Some(vec![
                        "/bin/sh".to_owned(),
                        "-c".to_owned(),
                        "printf RHHMARKER; sleep 30".to_owned(),
                    ]),
                    cwd: None,
                },
            },
        )
        .await;
        let seed = match await_command_result(&mut stream, 40).await {
            CommandResult::OkWith(CommandValue::TerminalId(id)) => id,
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        };

        // Poll GET_SCREEN on the seed pane until the marker the seed command
        // printed shows up — proof the wire `command` actually ran in the
        // seed pane (PTY startup + exec is asynchronous).
        let mut request_id = 41;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        let mut saw_marker = false;
        while tokio::time::Instant::now() < deadline {
            send_frame(
                &mut stream,
                &FrameKind::Command {
                    request_id,
                    command: Command::GetScreen {
                        terminal_id: seed.clone(),
                        request_scrollback: None,
                        cells: false,
                    },
                },
            )
            .await;
            if let CommandResult::OkWith(CommandValue::Json(json)) =
                await_command_result(&mut stream, request_id).await
            {
                let screen: phux_core::screen::ScreenState = serde_json::from_str(&json)
                    .expect("GET_SCREEN reply must be valid ScreenState");
                if screen.lines.iter().any(|line| line.contains("RHHMARKER")) {
                    saw_marker = true;
                    break;
                }
            }
            request_id += 1;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            saw_marker,
            "CREATE_SESSION seed command must run in the seed pane (RHHMARKER never rendered)",
        );
    });
}

/// KILL_COLLECTION tears down a whole named session in ONE round-trip
/// (`phux-h9s`). The test creates a session, kills it via KILL_COLLECTION,
/// then asserts a fresh GET_STATE no longer lists it. The whole path rides
/// `handle_client` (the production read loop) — the house rule for
/// wire->dispatch coverage. The kill replies `Ok` (the same ack shape
/// KILL_TERMINAL uses); the async TERMINAL_CLOSED frames confirm teardown.
#[test]
fn kill_collection_tears_down_named_session() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        // Pre-seed "work" so the server survives "scratch"'s teardown
        // (the tmux-model self-exit only fires when the LAST session reaps).
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Create a second session to tear down.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 50,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "scratch".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 50).await {
            CommandResult::OkWith(CommandValue::TerminalId(_)) => {}
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        }

        // One round-trip teardown.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 51,
                command: Command::KillCollection {
                    collection: CollectionId::new(1),
                    name: "scratch".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 51).await {
            CommandResult::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }

        // The reaped session leaves the snapshot. Teardown is asynchronous
        // (the Ok acks the start, TERMINAL_CLOSED follows), so poll GET_STATE
        // until "scratch" is gone while "work" remains.
        let mut request_id = 52;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let mut gone = false;
        while tokio::time::Instant::now() < deadline {
            send_frame(
                &mut stream,
                &FrameKind::Command {
                    request_id,
                    command: Command::GetState {
                        scope: StateScope::Server,
                    },
                },
            )
            .await;
            if let CommandResult::OkWith(CommandValue::State(snapshot)) =
                await_command_result(&mut stream, request_id).await
            {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                if !names.contains(&"scratch") {
                    assert!(
                        names.contains(&"work"),
                        "KILL_COLLECTION must tear down only the named session; got {names:?}",
                    );
                    gone = true;
                    break;
                }
            }
            request_id += 1;
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            gone,
            "KILL_COLLECTION must remove the named session from GET_STATE",
        );
    });
}

/// KILL_COLLECTION on an unknown session name is refused with
/// `SESSION_NOT_FOUND` rather than silently acked (`phux-h9s`) — symmetric
/// with CREATE_SESSION's duplicate-name refusal.
#[test]
fn kill_collection_unknown_session_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 60,
                command: Command::KillCollection {
                    collection: CollectionId::new(1),
                    name: "does-not-exist".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 60).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::SessionNotFound);
            }
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    });
}

/// KILL_COLLECTION under an unknown collection is refused with
/// `INVALID_COMMAND`; v0.1 servers host only the default `CollectionId(1)`.
#[test]
fn kill_collection_unknown_collection_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 61,
                command: Command::KillCollection {
                    collection: CollectionId::new(99),
                    name: "work".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 61).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::InvalidCommand);
            }
            other => panic!("expected Error(InvalidCommand), got {other:?}"),
        }
    });
}

/// RENAME_SESSION reassigns a session's name in one round-trip; a
/// subsequent GET_STATE snapshot shows the new name and not the old one.
#[test]
fn rename_session_renames_and_snapshot_reflects_new_name() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 70,
                command: Command::RenameSession {
                    collection: CollectionId::new(1),
                    name: "work".to_owned(),
                    new_name: "notes".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 70).await {
            CommandResult::Ok => {}
            other => panic!("expected Ok, got {other:?}"),
        }

        // The rename is synchronous server-side (a single field write), so the
        // very next snapshot must already carry the new name.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 71,
                command: Command::GetState {
                    scope: StateScope::Server,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 71).await {
            CommandResult::OkWith(CommandValue::State(snapshot)) => {
                let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
                assert!(
                    names.contains(&"notes"),
                    "RENAME_SESSION must surface the new name in GET_STATE; got {names:?}",
                );
                assert!(
                    !names.contains(&"work"),
                    "RENAME_SESSION must drop the old name; got {names:?}",
                );
            }
            other => panic!("expected Ok_With(State(..)), got {other:?}"),
        }
    });
}

/// RENAME_SESSION on an unknown current name is refused with
/// `SESSION_NOT_FOUND` — symmetric with KILL_COLLECTION's refusal.
#[test]
fn rename_session_unknown_name_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 72,
                command: Command::RenameSession {
                    collection: CollectionId::new(1),
                    name: "does-not-exist".to_owned(),
                    new_name: "notes".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 72).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::SessionNotFound);
            }
            other => panic!("expected Error(SessionNotFound), got {other:?}"),
        }
    });
}

/// RENAME_SESSION to a name already in use is refused with `INVALID_COMMAND`
/// — the same code CREATE_SESSION uses for a taken name. Names are unique
/// within a collection, so a rename never silently merges two sessions.
#[test]
fn rename_session_duplicate_new_name_is_refused() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) = spawn_server(socket_path.clone(), Some("work"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Create a second session whose name "work" already owns.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 73,
                command: Command::CreateSession {
                    collection: CollectionId::new(1),
                    name: "scratch".to_owned(),
                    command: None,
                    cwd: None,
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 73).await {
            CommandResult::OkWith(CommandValue::TerminalId(_)) => {}
            other => panic!("expected Ok_With(TerminalId(..)), got {other:?}"),
        }

        // Renaming "scratch" to the taken "work" must be refused.
        send_frame(
            &mut stream,
            &FrameKind::Command {
                request_id: 74,
                command: Command::RenameSession {
                    collection: CollectionId::new(1),
                    name: "scratch".to_owned(),
                    new_name: "work".to_owned(),
                },
            },
        )
        .await;
        match await_command_result(&mut stream, 74).await {
            CommandResult::Error { code, .. } => {
                assert_eq!(code, ErrorCode::InvalidCommand);
            }
            other => panic!("expected Error(InvalidCommand), got {other:?}"),
        }
    });
}
