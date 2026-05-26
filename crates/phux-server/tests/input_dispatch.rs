//! Wire-level integration test for the `INPUT_KEY` dispatch seam
//! (`phux-5g9`).
//!
//! Reproduces the keystroke-drop bug the parent agent caught with a
//! real `phux attach`: every client→server input frame was hitting
//! `handle_client`'s catch-all `_ => debug!("unhandled message type")`
//! arm and being silently discarded. The `pty_pump.rs` test exercises
//! the `PaneActor` input mpsc directly (bypassing the wire), which is
//! why it kept passing while the binary was half-deaf.
//!
//! This test drives the actual `handle_client` dispatch:
//!
//! 1. Pre-seeds a session whose pane is backed by a real PTY running
//!    `cat` — `cat` echoes stdin to stdout in cooked mode, so we get a
//!    crisp echo signal without depending on a user shell's prompt.
//! 2. Sends `ATTACH { ByName("default") }` over a real Unix socket,
//!    waits for `ATTACHED` + `PANE_SNAPSHOT` so subscription is in
//!    place (the dispatch path gates on the client being subscribed
//!    to the pane it's sending input to).
//! 3. Sends one `INPUT_KEY { pane_id, KeyEvent("a") }` followed by an
//!    Enter key (cooked mode is line-buffered). On any subsequent
//!    `PANE_OUTPUT` frame, the byte `b'a'` must appear — that proves
//!    the wire dispatch arm exists, routes to the right `PaneActor`,
//!    the actor encodes the key into PTY bytes, and `cat` echoes them
//!    back through the `PaneActor`'s output broadcast → fanout → the
//!    attached client.
//!
//! If the dispatch is missing (the bug), step 3 produces no output
//! and the test times out — exactly the symptom the parent agent
//! observed in real life.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

mod common;

use std::time::Duration;

use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::wire::frame::{FrameKind, TYPE_ATTACHED, TYPE_PANE_OUTPUT, TYPE_PANE_SNAPSHOT};
use portable_pty::CommandBuilder;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, WIRE_RECV_TIMEOUT, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server_with_seed_cmd, wait_for_socket,
};

/// Build a `KeyEvent` for an ASCII printable matching `phux-byc.6.5`'s
/// fixture shape: press, no modifiers, no composition.
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

/// Build an Enter key — no `text`, libghostty's encoder synthesizes the
/// CR. Matches `pty_pump.rs::input_keystroke_reaches_pty_and_echoes_back`.
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

/// Drain `PANE_OUTPUT` frames until either `needle` appears in the
/// accumulated bytes or `WIRE_RECV_TIMEOUT` elapses.
///
/// `cat` may emit the echo in several chunks (terminal driver + program
/// echo), so we accumulate until we see the byte we sent or we give up.
async fn await_echo(stream: &mut UnixStream, needle: u8) -> Vec<u8> {
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_PANE_OUTPUT {
            // Other frames (e.g. metadata) are fine; ignore.
            continue;
        }
        if let FrameKind::PaneOutput { bytes, .. } = frame {
            acc.extend_from_slice(&bytes);
            if acc.contains(&needle) {
                return acc;
            }
        }
    }
    acc
}

#[test]
fn input_key_dispatch_routes_to_pane_actor_pty() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");

        // `cat` is the deterministic echo fixture: cooked-mode PTY +
        // line-buffered cat → `a\r\n` (or similar) comes back after we
        // send `a` then Enter. Mirrors pty_pump.rs's fixture exactly.
        let cmd = CommandBuilder::new("/bin/cat");
        let (shutdown_tx, server_handle) =
            spawn_server_with_seed_cmd(socket_path.clone(), "default", cmd);

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // ---- ATTACH ----
        send_frame(&mut stream, &attach_by_name("default")).await;

        // ---- ATTACHED ----
        let (type_byte, attached) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_ATTACHED,
            "first server-to-client frame must be ATTACHED",
        );
        let wire_pane_id = match attached {
            FrameKind::Attached { snapshot, .. } => {
                assert_eq!(snapshot.panes.len(), 1, "exactly one pane");
                snapshot.panes[0].id.0
            }
            other => panic!("expected ATTACHED, got {other:?}"),
        };

        // ---- PANE_SNAPSHOT (one per pane in focused window) ----
        let (type_byte, _snap_frame) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_PANE_SNAPSHOT,
            "second server-to-client frame must be PANE_SNAPSHOT",
        );

        // ---- INPUT_KEY: press 'a' ----
        send_frame(
            &mut stream,
            &FrameKind::InputKey {
                pane_id: wire_pane_id,
                event: ascii_key('a', PhysicalKey::A),
            },
        )
        .await;

        // ---- INPUT_KEY: Enter (cat is line-buffered in cooked mode) ----
        send_frame(
            &mut stream,
            &FrameKind::InputKey {
                pane_id: wire_pane_id,
                event: enter_key(),
            },
        )
        .await;

        // The PTY driver echoes the input (and `cat` echoes the line
        // after Enter). Either way, `b'a'` must appear in some
        // PANE_OUTPUT chunk. If the dispatch arm is missing, NO
        // PANE_OUTPUT will arrive at all and this drains to timeout.
        let acc = await_echo(&mut stream, b'a').await;
        assert!(
            acc.contains(&b'a'),
            "INPUT_KEY('a') must round-trip through the PaneActor PTY and back as PANE_OUTPUT (got {} bytes: {:?})",
            acc.len(),
            acc,
        );

        // Clean teardown.
        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}
