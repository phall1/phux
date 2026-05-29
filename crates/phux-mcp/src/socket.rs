//! Default UDS path resolution for the MCP adapter.
//!
//! The phux daemon's canonical resolver, `default_socket_path`, lives in
//! `phux-server` (`runtime.rs`) — a heavy crate that drives libghostty and
//! PTYs. To keep this adapter lean we re-implement the small resolution
//! logic here rather than depend on `phux-server`.
//!
//! TODO(phux-93b): de-duplicate this with `phux_server::default_socket_path`
//! by hoisting the resolver into a shared lightweight crate (e.g.
//! `phux-config`), so the daemon and every thin client agree on one
//! definition. Kept in lockstep by hand for now.

use std::path::PathBuf;

/// Resolve the UDS path a tool should connect to.
///
/// Precedence:
/// 1. an explicit `socket` argument (the tool's optional `socket` field);
/// 2. the `PHUX_SOCKET` environment variable;
/// 3. the same default the daemon binds — `$XDG_RUNTIME_DIR/phux/phux.sock`,
///    falling back to `/tmp/phux-$USER/phux.sock`.
#[must_use]
pub(crate) fn resolve(explicit: Option<&str>) -> PathBuf {
    if let Some(path) = explicit {
        return PathBuf::from(path);
    }
    if let Some(env) = std::env::var_os("PHUX_SOCKET") {
        return PathBuf::from(env);
    }
    default_socket_path()
}

/// The daemon's default socket path. Mirrors
/// `phux_server::default_socket_path` (see module TODO).
fn default_socket_path() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut p = PathBuf::from(dir);
        p.push("phux");
        p.push("phux.sock");
        return p;
    }
    let uid_segment = std::env::var("UID")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "default".to_owned());
    let mut p = PathBuf::from("/tmp");
    p.push(format!("phux-{uid_segment}"));
    p.push("phux.sock");
    p
}
