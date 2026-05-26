//! Wire-level integration test for the PTY-EOF → DETACHED path
//! (`phux-it8`).
//!
//! Reproduces the user-visible "type `exit` in the inner shell and the
//! client freezes forever in alt-screen" bug. Before the fix, the
//! `TerminalActor`'s EOF branch just dropped its PTY receiver and kept the
//! actor alive "for snapshot/input drain" — but neither the runtime
//! nor any attached client got told the pane was dead, so the client
//! sat in its `tokio::select!` waiting for frames that never came.
//!
//! The fix (option (a) in the bd phux-it8 ticket):
//!
//! 1. `TerminalActor` fires an internal `exit_notify` oneshot when it sees
//!    `PtyEvent::Eof`, then exits cleanly.
//! 2. The runtime spawns a per-pane EOF watcher when it seeds the
//!    pane. On notification, the watcher walks `attached` and sends
//!    `FrameKind::Detached` to every client whose focused pane is the
//!    now-dead pane, then `state.detach(client_id)`s them.
//!
//! This test pins down the second half of that contract from the
//! wire's point of view: pre-seed with `/usr/bin/true` (or `/bin/true`)
//! — a process that exits immediately, guaranteeing prompt PTY EOF —
//! attach a client, and assert a `DETACHED` frame arrives within a
//! generous two-second deadline.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

mod common;

use std::time::Duration;

use phux_protocol::wire::frame::{FrameKind, TYPE_ATTACHED, TYPE_DETACHED, TYPE_TERMINAL_SNAPSHOT};
use portable_pty::CommandBuilder;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server_with_seed_cmd, wait_for_socket,
};

/// `/bin/true` (POSIX) and `/usr/bin/true` (some Linux distros) are
/// both common. Pick the first one that resolves at test time so the
/// test is robust across CI images. Falls back to `/bin/true` and
/// lets `TerminalActor::new_with_command` surface the real `spawn` error
/// if neither exists — that itself is a useful failure signal.
fn pick_true_command() -> CommandBuilder {
    // `/bin/sh -c 'sleep 0.2; exit 0'` instead of bare `/bin/true`.
    //
    // We want the child to live long enough for the test client to
    // complete ATTACH + receive TERMINAL_SNAPSHOT (the existing wire
    // contract) BEFORE the EOF fires — otherwise we're asserting on
    // a race we don't actually care about ("client never even
    // received the snapshot because the actor was already gone"),
    // not the lifecycle we want to pin down ("server tells attached
    // clients to detach when the pane dies").
    //
    // 200ms is a generous floor against a slow CI scheduler while
    // still being well below the test's 2-second DETACHED deadline.
    // The exact value isn't load-bearing; anything from ~50ms up
    // works. `sleep` is POSIX so no path probing needed.
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.arg("-c");
    cmd.arg("sleep 0.2; exit 0");
    cmd
}

/// Drain non-`DETACHED` frames (`ATTACHED`, `TERMINAL_SNAPSHOT`, late
/// `TERMINAL_OUTPUT`) until a `DETACHED` frame arrives or `deadline` elapses.
///
/// We can't just `recv_typed` once and assert: between `ATTACHED` and
/// the EOF-driven `DETACHED`, the runtime ships one `TERMINAL_SNAPSHOT` per
/// pane, and the `TerminalActor`'s PTY pump may emit a few stray
/// `TERMINAL_OUTPUT` chunks from libghostty's snapshot replay before the
/// EOF fires. Skip those rather than fail on them — they're correct
/// behavior on the byc.5 PTY path.
async fn await_detached(stream: &mut UnixStream, deadline: Duration) -> bool {
    let end = tokio::time::Instant::now() + deadline;
    loop {
        let remaining = end.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            return false;
        };
        if type_byte == TYPE_DETACHED {
            assert!(
                matches!(frame, FrameKind::Detached),
                "TYPE_DETACHED must decode to FrameKind::Detached",
            );
            return true;
        }
    }
}

/// `TerminalActor` PTY EOF (from `/bin/true` exiting promptly) drives the
/// runtime to send `FrameKind::Detached` to the attached client.
///
/// Before phux-it8, the client would never receive any post-snapshot
/// frame and this test would time out — exactly the user-facing
/// "client freezes in alt-screen" symptom, reduced to a deterministic
/// wire assertion.
#[test]
fn pty_eof_drives_detached_frame_to_attached_client() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");

        // `/bin/true` exits immediately → PTY EOF lands in the actor
        // within milliseconds, guaranteeing the EOF watcher fires
        // before our two-second deadline. Mirrors the
        // `ServerConfig.seed_command` pattern the `input_dispatch`
        // test uses for `cat`.
        let cmd = pick_true_command();
        let (shutdown_tx, server_handle) =
            spawn_server_with_seed_cmd(socket_path.clone(), "demo", cmd);

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // ---- ATTACH ----
        send_frame(&mut stream, &attach_by_name("demo")).await;

        // ---- ATTACHED ----
        let (type_byte, _attached) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_ATTACHED,
            "first server-to-client frame must be ATTACHED",
        );

        // ---- TERMINAL_SNAPSHOT (one per pane in focused window) ----
        let (type_byte, _snap_frame) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte, TYPE_TERMINAL_SNAPSHOT,
            "second server-to-client frame must be TERMINAL_SNAPSHOT",
        );

        // ---- DETACHED (the contract under test) ----
        //
        // The two-second deadline is generous on purpose: a CI box
        // under load needs headroom for the LocalSet to schedule the
        // accept loop, the per-pane actor task, the EOF watcher task,
        // and the writer task in that order. In local testing the
        // arrival is sub-100ms.
        let got_detached = await_detached(&mut stream, Duration::from_secs(2)).await;
        assert!(
            got_detached,
            "client must receive FrameKind::Detached within 2s of PTY EOF",
        );

        // Clean teardown. The server should still be alive (only the
        // one pane died); we shut it down explicitly.
        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}
