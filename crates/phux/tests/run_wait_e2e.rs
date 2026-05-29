//! Binary-level end-to-end tests for `phux run` and `phux wait` (phux-3rq).
//!
//! Unlike the pure unit tests in `phux-client` (which exercise the
//! sentinel parser and the wait condition in isolation), these drive the
//! REAL `phux` binary — built and handed to us by cargo at
//! `env!("CARGO_BIN_EXE_phux")` — against a REAL PTY-backed server. No
//! tmux, no mocks: a `phux server` child binds a private UDS, and each
//! verb runs as its own subprocess against that socket. They pin the
//! load-bearing process-exit contracts a consumer (an agent, a shell
//! `&&` chain) actually depends on:
//!
//!   * `run` mirrors the command's exit code into the process exit status
//!     (0 for success, 1 for `false`, an arbitrary code for a subshell).
//!   * `run --json` emits the stable `RunResult` contract.
//!   * `wait --until` exits 0 when the marker appears, 124 on timeout.
//!
//! Robustness notes:
//!   * The first server start can be slow (cold caches); we poll for the
//!     socket file with a generous deadline before driving any verb.
//!   * The server child is killed on guard drop, so a panicking assertion
//!     never leaks a daemon.
//!   * `--socket` is passed to EVERY verb so we never touch the user's
//!     real default socket, and the server is never auto-spawned.
//!   * The nonzero-mirror case uses `"(exit 7)"` — a POSIX subshell. This
//!     was verified empirically NOT to kill the session (a bare `exit 7`
//!     would terminate the shell and reap the pane); `phux ls` still
//!     lists the session afterward, and a subsequent `run` succeeds.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Path to the freshly-built `phux` binary, injected by cargo for
/// integration tests in the same crate.
const PHUX: &str = env!("CARGO_BIN_EXE_phux");

/// The pre-seeded session name every test drives against.
const SESSION: &str = "work";

/// How long to wait for the server to bind its socket. Generous: the
/// very first build/start on a cold CI host is the slow case.
const SOCKET_DEADLINE: Duration = Duration::from_secs(20);

/// Poll cadence while waiting for the socket file to appear.
const SOCKET_POLL: Duration = Duration::from_millis(50);

/// Monotonic counter so concurrently-running tests (nextest runs each in
/// its own process, but `cargo test` shares one) never collide on a
/// socket path.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A running `phux server`, killed when the guard drops so a failing
/// assertion never leaks a daemon.
struct ServerGuard {
    child: Child,
    socket: PathBuf,
    // Held to keep the temp dir alive for the guard's lifetime.
    _dir: tempfile::TempDir,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // Best-effort: the OS reaps it; we just stop it leaking.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ServerGuard {
    /// Spawn `phux server --session work --socket <unique>` detached
    /// from any terminal, then block until the socket file appears.
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir for socket");
        // Keep the path short: UDS paths have a ~104-char sun_path cap on
        // macOS, and a `tempdir()` path plus a long name can exceed it.
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket = dir
            .path()
            .join(format!("e2e-{}-{n}.sock", std::process::id()));

        let child = Command::new(PHUX)
            .args(["server", "--session", SESSION, "--socket"])
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn phux server");

        let guard = Self {
            child,
            socket,
            _dir: dir,
        };
        guard.wait_for_socket();
        guard
    }

    /// Poll until the socket file exists or the deadline elapses.
    fn wait_for_socket(&self) {
        let deadline = Instant::now() + SOCKET_DEADLINE;
        while Instant::now() < deadline {
            if self.socket.exists() {
                return;
            }
            std::thread::sleep(SOCKET_POLL);
        }
        panic!(
            "phux server did not bind {} within {SOCKET_DEADLINE:?}",
            self.socket.display()
        );
    }

    /// Build a `phux <verb> --socket <sock> <rest...>` command, where
    /// `args[0]` is the verb. `--socket` is injected right after the verb,
    /// NOT appended: `run`/`wait`/`send-keys` use `trailing_var_arg`, so a
    /// `--socket` placed after the positional command would be swallowed
    /// into that command (and the verb would fall back to the user's real
    /// default socket — verified the hard way). Verb-specific flags
    /// (`--json`, `--until`, `--timeout`) must therefore also precede the
    /// trailing positional in `args`.
    fn cmd(&self, args: &[&str]) -> Command {
        let (verb, rest) = args.split_first().expect("at least a verb");
        let mut c = Command::new(PHUX);
        c.arg(verb)
            .arg("--socket")
            .arg(&self.socket)
            .args(rest)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        c
    }
}

/// Run a verb to completion, returning its raw exit code (the value a
/// shell would see in `$?`). Panics if the process was killed by a
/// signal rather than exiting normally.
fn run_status(server: &ServerGuard, args: &[&str]) -> i32 {
    let status = server
        .cmd(args)
        .stdout(Stdio::null())
        .status()
        .expect("run phux verb");
    status
        .code()
        .unwrap_or_else(|| panic!("phux {args:?} terminated by signal: {status:?}"))
}

/// Run a verb capturing stdout, asserting it exited 0, and return stdout.
fn run_stdout(server: &ServerGuard, args: &[&str]) -> String {
    let out = server
        .cmd(args)
        .output()
        .expect("run phux verb with output");
    assert!(
        out.status.success(),
        "phux {args:?} exited {:?}; stdout={}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn run_mirrors_zero_exit_for_true() {
    let server = ServerGuard::start();
    assert_eq!(
        run_status(&server, &["run", SESSION, "true"]),
        0,
        "`phux run work true` should exit 0"
    );
}

#[test]
fn run_mirrors_one_exit_for_false() {
    let server = ServerGuard::start();
    assert_eq!(
        run_status(&server, &["run", SESSION, "false"]),
        1,
        "`phux run work false` should mirror false's exit 1"
    );
}

#[test]
fn run_mirrors_arbitrary_nonzero_exit() {
    // `(exit 7)` runs in a SUBSHELL, so the exit does not terminate the
    // pane's interactive shell — verified empirically: `phux ls` still
    // lists the session and a follow-up `run` succeeds. A bare `exit 7`
    // would kill the shell and reap the session, breaking the test.
    let server = ServerGuard::start();
    assert_eq!(
        run_status(&server, &["run", SESSION, "(exit 7)"]),
        7,
        "`phux run work '(exit 7)'` should mirror the subshell's exit 7"
    );
    // The session must have survived the subshell exit.
    assert_eq!(
        run_status(&server, &["run", SESSION, "true"]),
        0,
        "session should survive a subshell `(exit 7)` and still run commands"
    );
}

#[test]
fn run_json_reports_output_and_clean_exit() {
    let server = ServerGuard::start();
    // `--json` MUST precede the trailing command, or it is swallowed into
    // it (documented in the `run` help text).
    let stdout = run_stdout(&server, &["run", "--json", SESSION, "echo HELLO_E2E"]);
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("`phux run --json` should emit valid JSON");
    assert_eq!(v["exit_code"], 0, "echo should exit 0; got {stdout}");
    assert_eq!(
        v["truncated"], false,
        "single-line echo output should not be truncated; got {stdout}"
    );
    let output = v["output"]
        .as_str()
        .expect("`output` field should be a string");
    assert!(
        output.contains("HELLO_E2E"),
        "captured output should contain the echoed marker; got {output:?}"
    );
}

#[test]
fn wait_until_succeeds_when_marker_appears() {
    let server = ServerGuard::start();
    // Inject a marker into the pane, then wait for it. `--until` also
    // matches the command echo, which is fine for asserting exit 0.
    assert_eq!(
        run_status(
            &server,
            &["send-keys", SESSION, "echo WAIT_MARKER_XYZ", "Enter"],
        ),
        0,
        "send-keys should succeed"
    );
    assert_eq!(
        run_status(
            &server,
            &[
                "wait",
                SESSION,
                "--until",
                "WAIT_MARKER_XYZ",
                "--timeout",
                "5"
            ],
        ),
        0,
        "`phux wait --until` should exit 0 once the marker is visible"
    );
}

#[test]
fn wait_until_times_out_when_marker_never_appears() {
    let server = ServerGuard::start();
    assert_eq!(
        run_status(
            &server,
            &[
                "wait",
                SESSION,
                "--until",
                "STRING_THAT_NEVER_APPEARS_QZX",
                "--timeout",
                "1",
            ],
        ),
        124,
        "`phux wait` should exit 124 on timeout"
    );
}
