//! Wire-level integration test for the side-effect-free `ROUTE_INPUT`
//! command dispatch seam (`phux-3j3`).
//!
//! `ROUTE_INPUT` is the write counterpart to `GET_SCREEN`: it delivers an
//! already-built input event to a Terminal without an `ATTACH`,
//! subscription, or resize. The bug it closes off is the last disruptive
//! side effect on the agent surface — `send-keys`/`run` used to `ATTACH`
//! with an `80x24` viewport, which transiently resized the live pane.
//!
//! This test drives the real `handle_client` read loop end-to-end (no
//! TUI, no tmux) and proves the no-resize invariant empirically:
//!
//! 1. Pre-seed a session whose pane is backed by a real PTY running
//!    `cat` (cooked-mode echo gives a crisp signal).
//! 2. Resolve the pane id via the side-effect-free `GET_STATE` (no attach).
//! 3. Size the pane to `120x40` with `TERMINAL_RESIZE` and confirm the
//!    post-resize dims via `GET_SCREEN`.
//! 4. `ROUTE_INPUT` a key + Enter — on a fresh connection that never
//!    attaches and never advertises a viewport.
//! 5. `GET_SCREEN` again: the pane MUST still report `120x40`, not the
//!    `80x24` the old attach path would have imposed, and the echoed
//!    byte must have landed.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(
    clippy::doc_markdown,
    reason = "test narrative uses bare wire-frame names (ROUTE_INPUT, GET_SCREEN, …)"
)]

mod common;

use std::time::Duration;

use phux_protocol::input::InputEvent;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::wire::frame::{
    Command, CommandResult, CommandValue, FrameKind, StateScope, TYPE_COMMAND_RESULT,
};
use portable_pty::CommandBuilder;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, WIRE_RECV_TIMEOUT, recv_typed, run_local, send_frame,
    spawn_server_with_seed_cmd, wait_for_socket,
};

/// Build a press `KeyEvent` for an ASCII printable.
fn ascii_key(c: char, key: PhysicalKey) -> KeyEvent {
    KeyEvent {
        action: KeyAction::Press,
        key,
        mods: ModSet::empty(),
        consumed_mods: ModSet::empty(),
        composing: false,
        text: Some(c.to_string()),
        unshifted_codepoint: Some(c as u32),
    }
}

/// Enter — no `text`; libghostty's encoder synthesizes the CR.
const fn enter_key() -> KeyEvent {
    KeyEvent {
        action: KeyAction::Press,
        key: PhysicalKey::Enter,
        mods: ModSet::empty(),
        consumed_mods: ModSet::empty(),
        composing: false,
        text: None,
        unshifted_codepoint: None,
    }
}

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

/// `GET_STATE { Server }` → the focused pane id of the seeded session.
async fn focused_pane(stream: &mut UnixStream, request_id: u32) -> phux_protocol::ids::TerminalId {
    send_frame(
        stream,
        &FrameKind::Command {
            request_id,
            command: Command::GetState {
                scope: StateScope::Server,
            },
        },
    )
    .await;
    match await_command_result(stream, request_id).await {
        CommandResult::OkWith(CommandValue::State(snap)) => snap.focused_pane,
        other => panic!("expected Ok_With(State(..)), got {other:?}"),
    }
}

/// `GET_SCREEN` → the pane's current `(cols, rows)` and its joined text.
async fn screen(
    stream: &mut UnixStream,
    request_id: u32,
    terminal_id: &phux_protocol::ids::TerminalId,
) -> phux_core::screen::ScreenState {
    send_frame(
        stream,
        &FrameKind::Command {
            request_id,
            command: Command::GetScreen {
                terminal_id: terminal_id.clone(),
                request_scrollback: None,
            },
        },
    )
    .await;
    match await_command_result(stream, request_id).await {
        CommandResult::OkWith(CommandValue::Json(json)) => {
            serde_json::from_str(&json).expect("GET_SCREEN reply must be a valid ScreenState")
        }
        other => panic!("expected Ok_With(Json(..)), got {other:?}"),
    }
}

#[test]
fn route_input_delivers_keys_without_resizing_the_pane() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (_shutdown_tx, _server) =
            spawn_server_with_seed_cmd(socket_path.clone(), "work", CommandBuilder::new("cat"));
        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // Resolve the pane id without attaching.
        let pane = focused_pane(&mut stream, 1).await;

        // Size the pane to 120x40 (the dims the agent surface must preserve).
        send_frame(
            &mut stream,
            &FrameKind::TerminalResize {
                terminal_id: pane.clone(),
                cols: 120,
                rows: 40,
            },
        )
        .await;

        // Confirm the resize landed before we send any input.
        let before = poll_for_dims(&mut stream, &pane, (120, 40), 10).await;
        assert_eq!(
            (before.cols, before.rows),
            (120, 40),
            "TERMINAL_RESIZE must size the pane to 120x40 before ROUTE_INPUT",
        );

        // Route input on a FRESH connection that never attaches and never
        // advertises a viewport — this is the path `send-keys`/`run` take.
        let mut route_conn = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        for (i, key) in [ascii_key('z', PhysicalKey::Z), enter_key()]
            .into_iter()
            .enumerate()
        {
            let request_id = u32::try_from(i).unwrap() + 1;
            send_frame(
                &mut route_conn,
                &FrameKind::Command {
                    request_id,
                    command: Command::RouteInput {
                        terminal_id: pane.clone(),
                        event: InputEvent::Key(key),
                    },
                },
            )
            .await;
            match await_command_result(&mut route_conn, request_id).await {
                CommandResult::Ok => {}
                other => panic!("ROUTE_INPUT must ack with Ok, got {other:?}"),
            }
        }

        // The pane MUST still be 120x40 — ROUTE_INPUT carries no viewport
        // and never attaches, so unlike the old attach path it does not
        // shrink the pane to 80x24. Poll for the echoed byte to confirm the
        // event actually reached the PTY, then assert the dims held.
        let after = poll_for_echo(&mut stream, &pane, 'z', 40).await;
        assert_eq!(
            (after.cols, after.rows),
            (120, 40),
            "ROUTE_INPUT must NOT resize the pane: expected 120x40, got {}x{}",
            after.cols,
            after.rows,
        );
        let joined: String = after.lines.join("");
        assert!(
            joined.contains('z'),
            "routed key must reach the PTY and echo back; screen text: {joined:?}",
        );
    });
}

/// Poll `GET_SCREEN` until the pane reports `want` dims (or attempts run out).
async fn poll_for_dims(
    stream: &mut UnixStream,
    pane: &phux_protocol::ids::TerminalId,
    want: (u16, u16),
    attempts: u32,
) -> phux_core::screen::ScreenState {
    let mut last = None;
    for i in 0..attempts {
        let s = screen(stream, 1000 + i, pane).await;
        if (s.cols, s.rows) == want {
            return s;
        }
        last = Some(s);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    last.expect("at least one GET_SCREEN")
}

/// Poll `GET_SCREEN` until `needle` appears in the joined screen text.
async fn poll_for_echo(
    stream: &mut UnixStream,
    pane: &phux_protocol::ids::TerminalId,
    needle: char,
    attempts: u32,
) -> phux_core::screen::ScreenState {
    let mut last = None;
    for i in 0..attempts {
        let s = screen(stream, 2000 + i, pane).await;
        if s.lines.iter().any(|l| l.contains(needle)) {
            return s;
        }
        last = Some(s);
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    last.expect("at least one GET_SCREEN")
}
