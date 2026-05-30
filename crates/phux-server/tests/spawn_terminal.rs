//! `phux-4li.11` — Server-side SPAWN_TERMINAL handler + TERMINAL_CLOSED
//! emit + TERMINAL_RESIZE TIOCSWINSZ.
//!
//! Four scenarios pin the behavior:
//!
//! 1. **Spawn into the default Collection.** A client sends
//!    `SPAWN_TERMINAL { collection: DEFAULT, command: Some(/bin/cat) }`.
//!    The server replies `TERMINAL_SPAWNED { result: Ok(new_id) }`.
//!    A subsequent `INPUT_KEY { terminal_id: new_id, … }` round-trips
//!    via the freshly-spawned PTY's stdin → stdout, observable as
//!    `TERMINAL_OUTPUT { terminal_id: new_id, … }`.
//!
//! 2. **Spawn into an unknown Collection.** A client sends
//!    `SPAWN_TERMINAL { collection: CollectionId::new(99999), … }`.
//!    The server replies `TERMINAL_SPAWNED { result:
//!    Err(CollectionNotFound) }`.
//!
//! 3. **TERMINAL_CLOSED on PTY exit.** Spawn a Terminal running
//!    `sh -c 'exit 42'`. The PTY exits; the server emits
//!    `TERMINAL_CLOSED { terminal_id, exit_status: Some(42) }` to the
//!    subscribed (spawning) client.
//!
//! 4. **TERMINAL_RESIZE.** Spawn a Terminal, send `TERMINAL_RESIZE {
//!    terminal_id, cols: 120, rows: 40 }`. Detach + reattach via a
//!    second connection; the new `TERMINAL_SNAPSHOT` for the same
//!    pane reports the post-resize dims. Verifying via re-attach
//!    rather than waiting for an inline snapshot keeps the test on
//!    the public wire surface (the registry's `dims` field is what
//!    the snapshot pipeline reads).

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]
#![allow(
    clippy::doc_markdown,
    reason = "test-only file; the module/comment narrative uses bare wire-frame names (SPAWN_TERMINAL, TERMINAL_CLOSED, …) the way the integration tests above do for symmetry"
)]

mod common;

use std::time::Duration;

use phux_protocol::ids::CollectionId;
use phux_protocol::input::key::{KeyAction, KeyEvent, ModSet, PhysicalKey};
use phux_protocol::wire::frame::{
    FrameKind, SpawnError, SpawnResult, TYPE_TERMINAL_CLOSED, TYPE_TERMINAL_OUTPUT,
    TYPE_TERMINAL_SPAWNED,
};
use phux_server::DEFAULT_COLLECTION_ID;
use portable_pty::CommandBuilder;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::time::timeout;

use crate::common::{
    SOCKET_CONNECT_DEADLINE, WIRE_RECV_TIMEOUT, attach_by_name, recv_typed, run_local, send_frame,
    spawn_server, spawn_server_with_seed_cmd, wait_for_socket,
};

/// Drain frames until a `TERMINAL_SPAWNED` arrives whose `request_id`
/// matches `request_id`. Other frames (TERMINAL_OUTPUT bursts from the
/// fresh PTY, METADATA_CHANGED noise, etc.) are silently consumed.
async fn await_terminal_spawned(stream: &mut UnixStream, request_id: u32) -> SpawnResult {
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_TERMINAL_SPAWNED {
            continue;
        }
        if let FrameKind::TerminalSpawned {
            request_id: got,
            result,
        } = frame
            && got == request_id
        {
            return result;
        }
    }
    panic!("timed out waiting for TERMINAL_SPAWNED request_id={request_id}");
}

/// Drain until the accumulated TERMINAL_OUTPUT bytes for `pane`
/// contain `needle`, or the timeout fires. Mirrors `input_dispatch.rs`'s
/// `await_echo` but pane-scoped so other panes' output is ignored.
async fn await_echo_on(
    stream: &mut UnixStream,
    pane: &phux_protocol::ids::TerminalId,
    needle: u8,
) -> Vec<u8> {
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_TERMINAL_OUTPUT {
            continue;
        }
        if let FrameKind::TerminalOutput {
            terminal_id, bytes, ..
        } = frame
            && &terminal_id == pane
        {
            acc.extend_from_slice(&bytes);
            if acc.contains(&needle) {
                return acc;
            }
        }
    }
    acc
}

/// Drain until a `TERMINAL_CLOSED` for `pane` arrives, or the timeout
/// fires. Other frames are ignored.
async fn await_terminal_closed(
    stream: &mut UnixStream,
    pane: &phux_protocol::ids::TerminalId,
) -> Option<i32> {
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_TERMINAL_CLOSED {
            continue;
        }
        if let FrameKind::TerminalClosed {
            terminal_id,
            exit_status,
        } = frame
            && &terminal_id == pane
        {
            return exit_status;
        }
    }
    panic!("timed out waiting for TERMINAL_CLOSED for {pane:?}");
}

/// `KeyEvent` for an ASCII printable. Matches `input_dispatch.rs`'s
/// `ascii_key` fixture so the wire encoding is identical.
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

/// Enter key — no `text`, libghostty's encoder synthesizes the CR.
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

/// Build an `ATTACH { CreateIfMissing(name) }` frame so the test client
/// gets attached state (and thus an outbound mailbox) before sending
/// SPAWN_TERMINAL. Without an attached slot the auto-subscribe path in
/// `handle_spawn_terminal` skips the new pane, the spawning client
/// would not receive its own TERMINAL_OUTPUT, and the round-trip
/// assertion in scenario 1 could not be made.
fn attach_create_if_missing(name: &str) -> FrameKind {
    use phux_protocol::wire::frame::{AttachTarget, ViewportInfo};
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

/// Spawn a server, attach the client, and consume the initial
/// `ATTACHED` + `TERMINAL_SNAPSHOT` frames so subsequent `recv_typed`
/// calls only see test-driven traffic. Returns the stream + shutdown
/// channel + server handle.
async fn spawn_and_attach(
    tmp: &TempDir,
    session_name: &str,
) -> (
    UnixStream,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<(), phux_server::ServerError>>,
) {
    use phux_protocol::wire::frame::{TYPE_ATTACHED, TYPE_TERMINAL_SNAPSHOT};

    let socket_path = tmp.path().join("phux.sock");
    let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), None);
    let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
    send_frame(&mut stream, &attach_create_if_missing(session_name)).await;
    // ATTACHED
    let (type_byte, _attached) = recv_typed(&mut stream).await;
    assert_eq!(type_byte, TYPE_ATTACHED, "expected ATTACHED");
    // TERMINAL_SNAPSHOT for the seed pane
    let (type_byte, _snap) = recv_typed(&mut stream).await;
    assert_eq!(
        type_byte, TYPE_TERMINAL_SNAPSHOT,
        "expected TERMINAL_SNAPSHOT",
    );
    (stream, shutdown_tx, server_handle)
}

#[test]
fn spawn_terminal_in_default_collection_round_trips_input() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let (mut stream, shutdown_tx, server_handle) = spawn_and_attach(&tmp, "default").await;

        // SPAWN_TERMINAL with /bin/cat — cooked-mode echo fixture from
        // input_dispatch.rs. cat echoes the input back through the PTY
        // so we can prove the spawning client is wired to the new pane.
        send_frame(
            &mut stream,
            &FrameKind::SpawnTerminal {
                request_id: 42,
                collection: DEFAULT_COLLECTION_ID,
                command: Some(vec!["/bin/cat".to_owned()]),
                cwd: None,
                env: None,
            },
        )
        .await;

        // Reply must carry our request_id and an Ok TerminalId.
        let result = await_terminal_spawned(&mut stream, 42).await;
        let new_id = match result {
            SpawnResult::Ok(id) => id,
            SpawnResult::Err(e) => panic!("expected Ok, got Err({e:?})"),
            other => panic!("unexpected SpawnResult variant: {other:?}"),
        };
        assert!(
            new_id.is_local(),
            "freshly spawned TerminalId must be LOCAL (got {new_id:?})",
        );

        // INPUT_KEY('a') + Enter through the new pane.
        send_frame(
            &mut stream,
            &FrameKind::InputKey {
                terminal_id: new_id.clone(),
                event: ascii_key('a', PhysicalKey::A),
            },
        )
        .await;
        send_frame(
            &mut stream,
            &FrameKind::InputKey {
                terminal_id: new_id.clone(),
                event: enter_key(),
            },
        )
        .await;

        // cat echoes the typed byte back through the PTY → broadcast →
        // outbound pump → TERMINAL_OUTPUT for `new_id`.
        let acc = await_echo_on(&mut stream, &new_id, b'a').await;
        assert!(
            acc.contains(&b'a'),
            "INPUT_KEY('a') to spawned pane must round-trip through PTY (got {} bytes: {:?})",
            acc.len(),
            acc,
        );

        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}

#[test]
fn spawn_terminal_unknown_collection_returns_collection_not_found() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let (mut stream, shutdown_tx, server_handle) = spawn_and_attach(&tmp, "default").await;

        // CollectionId::new(99999) — any non-default id MUST surface
        // SpawnError::CollectionNotFound per SPEC §7.4's L2-dependency
        // note and the wire frame's doc.
        send_frame(
            &mut stream,
            &FrameKind::SpawnTerminal {
                request_id: 7,
                collection: CollectionId::new(99_999),
                command: None,
                cwd: None,
                env: None,
            },
        )
        .await;

        let result = await_terminal_spawned(&mut stream, 7).await;
        match result {
            SpawnResult::Err(SpawnError::CollectionNotFound) => {}
            other => panic!("expected Err(CollectionNotFound), got {other:?}"),
        }
        // SAFETY note for the future reader: SpawnError is
        // #[non_exhaustive] so the outer SpawnResult::Err arm above
        // catches both CollectionNotFound and any v0.2.x additions —
        // `other` covers both unknown SpawnResult variants and
        // unknown SpawnError variants nested inside Err.

        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}

#[test]
fn spawn_terminal_emits_terminal_closed_on_pty_exit() {
    run_local(async {
        let tmp = TempDir::new().unwrap();
        let (mut stream, shutdown_tx, server_handle) = spawn_and_attach(&tmp, "default").await;

        // sh -c 'exit 42' is portable (BSD, Linux, macOS) and produces
        // a deterministic exit code we can assert on the wire.
        send_frame(
            &mut stream,
            &FrameKind::SpawnTerminal {
                request_id: 1,
                collection: DEFAULT_COLLECTION_ID,
                command: Some(vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "exit 42".to_owned(),
                ]),
                cwd: None,
                env: None,
            },
        )
        .await;

        let result = await_terminal_spawned(&mut stream, 1).await;
        let new_id = match result {
            SpawnResult::Ok(id) => id,
            SpawnResult::Err(e) => panic!("expected Ok, got Err({e:?})"),
            other => panic!("unexpected SpawnResult variant: {other:?}"),
        };

        // The child exits almost immediately; the PTY EOF watcher
        // fires the exit_notify oneshot which drives the
        // TERMINAL_CLOSED broadcast. WIRE_RECV_TIMEOUT (5s) is the
        // budget.
        let exit_status = await_terminal_closed(&mut stream, &new_id).await;
        assert_eq!(
            exit_status,
            Some(42),
            "TERMINAL_CLOSED exit_status must be Some(42) for `sh -c 'exit 42'`",
        );

        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}

#[test]
fn terminal_resize_updates_pane_dims_observable_on_reattach() {
    run_local(async {
        use phux_protocol::wire::frame::{
            AttachTarget, TYPE_ATTACHED, TYPE_TERMINAL_SNAPSHOT, ViewportInfo,
        };

        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone(), None);

        // First client: create the session, spawn a terminal, resize it.
        let mut stream_a = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(&mut stream_a, &attach_create_if_missing("resize-test")).await;
        let (type_byte, _attached) = recv_typed(&mut stream_a).await;
        assert_eq!(type_byte, TYPE_ATTACHED);
        let (type_byte, _snap) = recv_typed(&mut stream_a).await;
        assert_eq!(type_byte, TYPE_TERMINAL_SNAPSHOT);

        // Use /bin/cat so the actor stays alive for the duration of the
        // resize round-trip. A short-lived command would race with the
        // resize ioctl (the actor could already be tearing down by the
        // time TERMINAL_RESIZE arrives).
        send_frame(
            &mut stream_a,
            &FrameKind::SpawnTerminal {
                request_id: 99,
                collection: DEFAULT_COLLECTION_ID,
                command: Some(vec!["/bin/cat".to_owned()]),
                cwd: None,
                env: None,
            },
        )
        .await;
        let result = await_terminal_spawned(&mut stream_a, 99).await;
        let new_id = match result {
            SpawnResult::Ok(id) => id,
            SpawnResult::Err(e) => panic!("expected Ok, got Err({e:?})"),
            other => panic!("unexpected SpawnResult variant: {other:?}"),
        };

        // Send TERMINAL_RESIZE with non-default dims (the spawn defaults
        // to 80x24; assert the resize is observable as the *changed*
        // dims, not the original).
        send_frame(
            &mut stream_a,
            &FrameKind::TerminalResize {
                terminal_id: new_id.clone(),
                cols: 120,
                rows: 40,
            },
        )
        .await;

        // The resize is fire-and-forget (no reply on the wire). Give
        // the actor a tick to process the mailbox before we observe
        // the effect via re-attach. The resize handler calls
        // `try_send` synchronously inside a `with_mut`, so the only
        // async hop is the actor pulling from its resize_rx.
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Open a second connection and ATTACH ByName to the same
        // session. The new TERMINAL_SNAPSHOT frames will report the
        // post-resize dims (the registry's `dims` field is what
        // `build_session_snapshot` reads — see state.rs).
        let mut stream_b = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;
        send_frame(
            &mut stream_b,
            &FrameKind::Attach {
                target: AttachTarget::ByName("spawn-1".to_owned()),
                viewport: ViewportInfo::new(80, 24),
                request_scrollback: false,
                scrollback_limit_lines: 0,
            },
        )
        .await;
        // ATTACHED
        let (type_byte, attached) = recv_typed(&mut stream_b).await;
        assert_eq!(
            type_byte, TYPE_ATTACHED,
            "second client must see ATTACHED for spawn-1 session",
        );
        // `SessionSnapshot.panes` aggregates panes across ALL sessions
        // (resize-test + spawn-1 in this test), so filter by the
        // spawned terminal id rather than asserting on the slice's
        // length. The id is what the client correlates across the wire
        // in any case.
        let panes = match attached {
            FrameKind::Attached { snapshot, .. } => snapshot.panes,
            other => panic!("expected Attached, got {other:?}"),
        };
        let spawned = panes.iter().find(|p| p.id == new_id).unwrap_or_else(|| {
            panic!(
                "spawn-1's pane (id={new_id:?}) missing from re-attach snapshot \
                     (got {} panes: ids={:?})",
                panes.len(),
                panes.iter().map(|p| p.id.clone()).collect::<Vec<_>>(),
            )
        });
        assert_eq!(
            spawned.cols, 120,
            "spawn-1's pane must report post-resize cols (120), got {}",
            spawned.cols,
        );
        assert_eq!(
            spawned.rows, 40,
            "spawn-1's pane must report post-resize rows (40), got {}",
            spawned.rows,
        );

        drop(stream_a);
        drop(stream_b);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}

/// Drain until the accumulated TERMINAL_OUTPUT bytes for `pane` contain
/// `needle` (a byte sequence), or the timeout fires. Mirrors
/// `await_echo_on` but matches a multi-byte subsequence.
async fn await_output_contains(
    stream: &mut UnixStream,
    pane: &phux_protocol::ids::TerminalId,
    needle: &[u8],
) -> Vec<u8> {
    let mut acc: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + WIRE_RECV_TIMEOUT;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        let Ok((type_byte, frame)) = timeout(remaining, recv_typed(stream)).await else {
            break;
        };
        if type_byte != TYPE_TERMINAL_OUTPUT {
            continue;
        }
        if let FrameKind::TerminalOutput {
            terminal_id, bytes, ..
        } = frame
            && &terminal_id == pane
        {
            acc.extend_from_slice(&bytes);
            if acc.windows(needle.len()).any(|w| w == needle) {
                return acc;
            }
        }
    }
    acc
}

/// phux-cs6 acceptance: with `defaults.cwd-inheritance = inherit-focused`
/// (the schema default the test server runs with), a `SPAWN_TERMINAL`
/// that leaves `cwd` unset opens the new pane in the *focused* pane's
/// live working directory.
///
/// The focused (pre-seeded) pane is a shell that `cd`s into a fresh temp
/// dir and then blocks. The spawned pane runs `pwd`, whose stdout — the
/// inherited directory — comes back as TERMINAL_OUTPUT. This is the wire-
/// level proof of the `C-a |` cd-to-/tmp scenario in the bead.
#[test]
fn spawn_terminal_inherits_focused_pane_live_cwd() {
    use phux_protocol::wire::frame::TYPE_ATTACHED;

    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");

        // Focused pane: a shell sitting in a known temp dir. Canonicalize
        // so the expected path matches what the kernel CWD query returns
        // (macOS resolves /var → /private/var).
        let cwd_dir = TempDir::new().unwrap();
        let cwd_path = cwd_dir.path().canonicalize().expect("canonicalize cwd");
        let mut seed = CommandBuilder::new("/bin/sh");
        seed.arg("-c");
        // `read _` (a builtin) blocks the shell on the PTY, keeping the pane
        // alive in its cwd. NOT `exec read _` — `exec` needs an external
        // program, so it dies immediately (status 1), which raced the ATTACH
        // and flaked on fast CI.
        seed.arg(format!("cd '{}' && read _", cwd_path.display()));
        let (shutdown_tx, server_handle) =
            spawn_server_with_seed_cmd(socket_path.clone(), "focused", seed);

        let mut stream = wait_for_socket(&socket_path, SOCKET_CONNECT_DEADLINE).await;

        // ATTACH ByName focuses the pre-seeded pane.
        send_frame(&mut stream, &attach_by_name("focused")).await;
        let (type_byte, _attached) = recv_typed(&mut stream).await;
        assert_eq!(type_byte, TYPE_ATTACHED, "expected ATTACHED");
        // Drain the seed pane's TERMINAL_SNAPSHOT.
        let (type_byte, _snap) = recv_typed(&mut stream).await;
        assert_eq!(
            type_byte,
            phux_protocol::wire::frame::TYPE_TERMINAL_SNAPSHOT,
            "expected TERMINAL_SNAPSHOT",
        );

        // Give the seed shell a beat to run its `cd` before we query it.
        tokio::time::sleep(Duration::from_millis(150)).await;

        // SPAWN_TERMINAL with cwd UNSET and a command that prints its
        // CWD. With inherit-focused, the server seeds the new pane's
        // CommandBuilder.cwd from the focused pane's live directory.
        send_frame(
            &mut stream,
            &FrameKind::SpawnTerminal {
                request_id: 1,
                collection: DEFAULT_COLLECTION_ID,
                command: Some(vec![
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    // `read _` blocks (a builtin); `exec read _` would die.
                    "pwd; read _".to_owned(),
                ]),
                cwd: None,
                env: None,
            },
        )
        .await;

        let new_id = match await_terminal_spawned(&mut stream, 1).await {
            SpawnResult::Ok(id) => id,
            other => panic!("SPAWN_TERMINAL did not succeed: {other:?}"),
        };

        let needle = cwd_path.to_str().expect("utf8 cwd").as_bytes();
        let acc = await_output_contains(&mut stream, &new_id, needle).await;
        let body = String::from_utf8_lossy(&acc);
        assert!(
            acc.windows(needle.len()).any(|w| w == needle),
            "spawned pane must inherit the focused pane's live CWD ({}); got output: {body:?}",
            cwd_path.display(),
        );

        drop(stream);
        shutdown_tx.send(()).ok();
        timeout(Duration::from_secs(5), server_handle)
            .await
            .expect("server didn't shut down in time")
            .expect("server join")
            .expect("server run_async ok");
    });
}
