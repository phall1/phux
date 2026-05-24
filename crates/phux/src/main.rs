//! phux binary entry point.
//!
//! Single executable, multiple subcommands. By convention:
//!   phux           → attach to (or auto-spawn) the user's server
//!   phux server    → run a server in the foreground (for `--stdio`, supervisord, etc.)
//!   phux new       → create a new session
//!   phux ls        → list sessions
//!   phux kill      → kill sessions / windows / panes
//!
//! Subcommands are unstable until v0.1.

#![forbid(unsafe_code)]

fn main() {
    eprintln!("phux {} (pre-alpha; see SPEC.md)", env!("CARGO_PKG_VERSION"));
}
