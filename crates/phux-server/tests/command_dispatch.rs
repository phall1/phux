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

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(
    clippy::doc_markdown,
    reason = "test narrative uses bare wire-frame names (COMMAND, GET_STATE, …) for symmetry with sibling tests"
)]

mod common;

use std::time::Duration;

use phux_protocol::ids::TerminalId;
use phux_protocol::wire::frame::{
    Command, CommandResult, CommandValue, ErrorCode, FrameKind, StateScope, TYPE_COMMAND_RESULT,
    TYPE_TERMINAL_CLOSED,
};
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, WIRE_RECV_TIMEOUT, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server, try_recv_typed, wait_for_socket,
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
