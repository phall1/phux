//! phux-p4vp — end-to-end: the sidebar's VCS branch line derives from
//! REAL `ATTACHED` snapshot cwds, not client-side injection.
//!
//! Prior client coverage fed the branch machinery from the client side:
//! `vcs` tests derive branches from fixture repos given a cwd directly,
//! and the sidebar tests hand `WindowInfo` a pre-derived branch string.
//! Nothing proved that a server-populated `TerminalInfo::cwd` actually
//! flows through `handle_server_frame` -> `VcsIndex` -> `window_infos`
//! -> the painted branch row. This test closes that seam with the full
//! path, one process end to end:
//!
//! 1. A real `ServerRuntime` on a UDS with PTY-backed attach-create.
//! 2. `AttachTarget::CreateIfMissing` seeds a pane whose shell `cd`s
//!    into a fixture git repo and blocks (`read _` keeps the child
//!    alive on the PTY).
//! 3. The server's `ATTACHED` snapshot carries the pane's kernel cwd
//!    (spawn-time stamp + attach-time `refresh_registry_cwds`, the
//!    phux-p4vp server fix).
//! 4. `run_headless_rendered` replays that snapshot through the real
//!    client frame handler and composites the sidebar.
//! 5. The branch name appears on a sidebar row of the rendered frame.
//!
//! The fixture repo is a hand-written `.git/HEAD` (no `git` subprocess —
//! matching how `phux_client::vcs` reads it). The branch name is chosen
//! so it appears nowhere else in the scenario (not in any path, command
//! line, or shell echo), so finding it in the frame proves the
//! derivation ran against the wire-carried cwd.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use phux_client::attach::run_headless_rendered;
use phux_client::snapshot::RenderedFrame;
use phux_protocol::wire::frame::AttachTarget;
use phux_server::{ServerConfig, ServerError, ServerRuntime};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// Branch name written into the fixture `HEAD`. Deliberately distinctive:
/// it must not be a substring of any temp path, command line, or shell
/// prompt the composited panes could echo. Short enough (8 cells) to
/// survive the sidebar's width-based truncation at the default width 20.
const BRANCH: &str = "p4vp-e2e";

/// Session name for the attach-created session.
const SESSION: &str = "branch-e2e";

/// Composite viewport. Sidebar (default width 20, left) + panes.
const VIEW: (u16, u16) = (80, 24);

/// Overall deadline for the shell's `cd` to land and a re-attach to
/// observe it. The kernel-cwd refresh runs at attach time, so each retry
/// is a fresh attach; generous for slow CI.
const BRANCH_DEADLINE: Duration = Duration::from_secs(10);

/// Spawn a `ServerRuntime` on `socket_path` with PTY-backed
/// attach-create (`seed_with_pty` mirrors into
/// `attach_create_seeds_pty`, so `CreateIfMissing` honors its wire
/// `command` and spawns a real PTY child).
fn spawn_server(
    socket_path: PathBuf,
) -> (oneshot::Sender<()>, JoinHandle<Result<(), ServerError>>) {
    let (tx, rx) = oneshot::channel::<()>();
    let cfg = ServerConfig {
        socket_path,
        pre_seeded_session: None,
        seed_with_pty: true,
        seed_command: None,
        ..ServerConfig::with_default_socket()
    };
    let handle = tokio::task::spawn_local(async move {
        let server = ServerRuntime::new(cfg);
        server
            .run_async(async move {
                let _ = rx.await;
            })
            .await
    });
    (tx, handle)
}

/// Wait for the server's UDS to become connectable.
async fn wait_for_socket(path: &Path) {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if tokio::net::UnixStream::connect(path).await.is_ok() {
            return;
        }
        sleep(Duration::from_millis(5)).await;
    }
    panic!("socket {} never became connectable", path.display());
}

/// Run `fut` inside a `current_thread` runtime + `LocalSet` (the server's
/// per-pane actors are `!Send`, ADR-0014).
fn run_local<F>(fut: F)
where
    F: std::future::Future<Output = ()>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, fut);
}

/// The leftmost `width` columns of `frame` row `row`, joined as text —
/// the sidebar strip occupies exactly those cells (left edge, default).
fn sidebar_row_text(frame: &RenderedFrame, row: u16, width: u16) -> String {
    let cols = usize::from(frame.cols);
    let base = usize::from(row) * cols;
    frame.cells[base..base + usize::from(width.min(frame.cols))]
        .iter()
        .map(|c| c.grapheme.as_str())
        .collect()
}

/// `true` when any sidebar row of `frame` carries the fixture branch.
fn frame_shows_branch(frame: &RenderedFrame) -> bool {
    (0..frame.rows).any(|row| sidebar_row_text(frame, row, 20).contains(BRANCH))
}

#[test]
fn sidebar_branch_line_derives_from_attached_snapshot_cwd() {
    // `run_headless_rendered` reads `[sidebar]` via the canonical config
    // path; point XDG at a temp home that enables it. Must happen before
    // any async machinery spins up.
    let cfg_home = TempDir::new().unwrap();
    let phux_cfg_dir = cfg_home.path().join("phux");
    std::fs::create_dir_all(&phux_cfg_dir).unwrap();
    std::fs::write(
        phux_cfg_dir.join("config.toml"),
        "[sidebar]\nenabled = true\n",
    )
    .unwrap();
    // SAFETY: process-global env mutation before any thread exists (the
    // tokio runtime is built below). This file holds a single test, so no
    // sibling test races it under `cargo test` either; nextest isolates
    // per-process regardless. Same pattern as `phux-server/tests/ws_attach.rs`.
    unsafe {
        std::env::set_var("XDG_CONFIG_HOME", cfg_home.path());
    }

    // Fixture repo: a hand-written `.git/HEAD` on a branch, exactly the
    // shape `phux_client::vcs` derives from. Canonicalize so the shell's
    // `cd` target and the kernel-reported cwd agree in spelling (macOS
    // resolves /var -> /private/var).
    let repo = TempDir::new().unwrap();
    let repo_path = repo.path().canonicalize().expect("canonicalize repo");
    std::fs::create_dir_all(repo_path.join(".git")).unwrap();
    std::fs::write(
        repo_path.join(".git/HEAD"),
        format!("ref: refs/heads/{BRANCH}\n"),
    )
    .unwrap();

    run_local(async {
        let tmp = TempDir::new().unwrap();
        let socket_path = tmp.path().join("phux.sock");
        let (shutdown_tx, server_handle) = spawn_server(socket_path.clone());
        wait_for_socket(&socket_path).await;

        // The seed shell `cd`s into the fixture repo and blocks on the
        // PTY. The snapshot cwd must come from the wire (kernel query at
        // attach time) — nothing client-side knows this path.
        let target = || AttachTarget::CreateIfMissing {
            name: SESSION.to_owned(),
            command: Some(vec![
                "/bin/sh".to_owned(),
                "-c".to_owned(),
                format!("cd '{}' && read _", repo_path.display()),
            ]),
            cwd: None,
        };

        // The kernel-cwd refresh runs once per attach, and the first
        // attach can race the shell's `cd` (it may still be in the spawn
        // directory). Re-attach until the branch row lands or the
        // deadline passes — each iteration is the FULL path: server
        // snapshot -> wire -> client frame handler -> VcsIndex -> paint.
        let start = Instant::now();
        // `Some(frame)` = the deadline passed without a branch row; the
        // frame is kept for the failure dump. `None` = success.
        let failed: Option<RenderedFrame> = loop {
            let frame = run_headless_rendered(&socket_path, target(), VIEW.0, VIEW.1)
                .await
                .expect("headless rendered attach");
            if frame_shows_branch(&frame) {
                break None;
            }
            if start.elapsed() >= BRANCH_DEADLINE {
                break Some(frame);
            }
            sleep(Duration::from_millis(150)).await;
        };

        if let Some(frame) = failed {
            let dump: Vec<String> = (0..frame.rows)
                .map(|r| sidebar_row_text(&frame, r, frame.cols))
                .collect();
            panic!(
                "sidebar never showed branch {BRANCH:?} within {BRANCH_DEADLINE:?}; \
                 last composited frame:\n{}",
                dump.join("\n"),
            );
        }

        shutdown_tx.send(()).ok();
        server_handle.await.unwrap().unwrap();
    });
}
