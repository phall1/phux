//! Binary-level end-to-end test for the ADR-0040 agent-identity record
//! (`phux-3ert`): `phux agent set` writes `phux.agent/v1` through the real
//! L3 `SET_METADATA` path, `phux agent show` reports from the record with
//! `agent_record` provenance (no heuristics), and `phux agent clear`
//! deletes it so the report falls back to the heuristic sources.
//!
//! Same harness discipline as `run_wait_e2e.rs`: a real `phux server`
//! child on a private UDS, each verb its own subprocess, guard-killed on
//! drop. Kept in its own file so the `just e2e` lane lists it explicitly.

#![allow(clippy::expect_used, reason = "tests")]
#![allow(clippy::unwrap_used, reason = "tests")]
#![allow(clippy::panic, reason = "tests")]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Path to the freshly-built `phux` binary, injected by cargo.
const PHUX: &str = env!("CARGO_BIN_EXE_phux");

/// The pre-seeded session name the test drives against.
const SESSION: &str = "work";

/// How long to wait for the server to bind its socket (cold-start bound).
const SOCKET_DEADLINE: Duration = Duration::from_secs(30);

/// Poll cadence while waiting for the socket file to appear.
const SOCKET_POLL: Duration = Duration::from_millis(50);

/// Monotonic counter so concurrent tests never collide on a socket path.
static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A running `phux server`, killed when the guard drops.
struct ServerGuard {
    child: Child,
    socket: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ServerGuard {
    fn start() -> Self {
        let dir = tempfile::tempdir().expect("create temp dir for socket");
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket = dir
            .path()
            .join(format!("agent-{}-{n}.sock", std::process::id()));
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

    /// Run `phux agent <args...> --socket <sock>` capturing stdout.
    /// `agent`'s subcommands take `--socket` as a per-verb flag (no
    /// trailing positional swallows it), so appending is safe here.
    fn agent(&self, args: &[&str]) -> String {
        let out = Command::new(PHUX)
            .arg("agent")
            .args(args)
            .arg("--socket")
            .arg(&self.socket)
            .stdin(Stdio::null())
            .output()
            .expect("run phux agent verb");
        assert!(
            out.status.success(),
            "phux agent {args:?} exited {:?}; stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }
}

/// The full declare/report/clear loop against a real server: the record
/// outranks heuristics while present and disappears cleanly on `clear`.
#[test]
#[ignore = "spawns a real phux server; starves in the full parallel pool. Run via `just e2e`."]
fn agent_record_set_show_clear_roundtrip() {
    let server = ServerGuard::start();

    // Declare identity on the session's (single) pane.
    let confirmed = server.agent(&[
        "set",
        SESSION,
        "--name",
        "reviewer",
        "--kind",
        "claude",
        "--state",
        "blocked",
        "--session",
        "wave1",
    ]);
    assert!(
        confirmed.contains("\"name\":\"reviewer\""),
        "set must echo the confirmed record: {confirmed}"
    );

    // The report comes straight from the record — structured provenance,
    // no substring heuristics.
    let shown = server.agent(&["show", SESSION, "--json"]);
    let json: serde_json::Value = serde_json::from_str(&shown).expect("agent show JSON");
    let agent = &json["agents"][0];
    assert_eq!(agent["agent"]["label"], "reviewer", "label from record");
    assert_eq!(agent["agent"]["kind"], "claude", "kind slug mapped");
    assert_eq!(agent["state"], "blocked", "state from record");
    assert_eq!(
        agent["sources"][0]["kind"], "agent_record",
        "provenance must be the structured record: {shown}"
    );
    assert_eq!(
        agent["sources"].as_array().map(Vec::len),
        Some(1),
        "no heuristic source may run while a record is declared"
    );

    // Clear: the record is deleted and the report falls back to the
    // heuristic sources (whatever they infer, provenance is not the record).
    let cleared = server.agent(&["clear", SESSION]);
    assert!(
        cleared.trim_end().ends_with("\t-"),
        "clear must confirm the tombstone: {cleared:?}"
    );
    let shown = server.agent(&["show", SESSION, "--json"]);
    let json: serde_json::Value = serde_json::from_str(&shown).expect("agent show JSON");
    let sources = json["agents"][0]["sources"]
        .as_array()
        .expect("sources array");
    assert!(
        sources
            .iter()
            .all(|source| source["kind"] != "agent_record"),
        "after clear no source may claim the record: {shown}"
    );
}
