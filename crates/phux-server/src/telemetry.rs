//! Process-global telemetry bootstrap.
//!
//! Wires up `tracing` so that the existing `tracing::{info,debug,warn}!`
//! call sites in this crate actually emit. Without this, every tracing
//! macro is a silent no-op (no subscriber installed).
//!
//! Layer composition:
//!
//! * Always-on: a `tracing_subscriber::fmt` layer writing to **stderr**
//!   (the binary's stdout is reserved for protocol/PTY traffic on the
//!   `--stdio` future path; never pollute it with log lines). The
//!   filter is read from `RUST_LOG`, defaulting to `phux=info,warn`.
//! * Opt-in: when this crate is built with `--features tokio-console`
//!   (and Tokio is built with `--cfg tokio_unstable`), a
//!   `console_subscriber` layer is also installed so an operator can
//!   attach the `tokio-console` CLI for live actor introspection
//!   (broadcast lag, task stalls, poll counts).
//!
//! The function is **idempotent at the type level only** — `init()`
//! must be called at most once per process. Subsequent calls will
//! panic via `try_init`'s error path; callers should not call it from
//! tests.
//!
//! ## Why factor this out
//!
//! A follow-up agent will add a `dhat-heap` feature that swaps the
//! global allocator. Allocator setup happens *outside* this function
//! (it requires a `#[global_allocator]` static + a `dhat::Profiler`
//! guard owned by `main`), so this module deliberately stays
//! allocator-agnostic and additive.

use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, fmt};

/// Default tracing filter applied when `RUST_LOG` is unset.
///
/// `phux=info` keeps server-side `info!` lines visible without
/// drowning the operator in `tokio`/`hyper`/etc. The trailing `warn`
/// fallback ensures genuinely surprising events from any crate still
/// surface.
const DEFAULT_FILTER: &str = "phux=info,warn";

/// Install the process-global `tracing` subscriber.
///
/// Call this from the binary entry point **before** building the
/// tokio runtime. Calling it more than once will panic.
///
/// Returns `Err` if a subscriber was already installed (e.g. by a
/// test harness or a buggy second call); callers in `main` can treat
/// this as fatal and exit non-zero, or simply log and continue.
pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    // Build the always-on stderr fmt layer. `with_writer(io::stderr)`
    // is the documented way to redirect output; the default writes to
    // stdout, which we must avoid.
    let fmt_layer = fmt::layer().with_writer(std::io::stderr);

    let registry = tracing_subscriber::registry().with(filter).with(fmt_layer);

    // The `tokio-console` integration is purely additive: it adds a
    // second layer that publishes runtime task instrumentation to a
    // gRPC server (default 127.0.0.1:6669) that the `tokio-console`
    // CLI connects to. Requires `--cfg tokio_unstable` to be set when
    // building Tokio itself; see this module's docs.
    #[cfg(feature = "tokio-console")]
    {
        let console_layer = console_subscriber::ConsoleLayer::builder()
            .with_default_env()
            .spawn();
        registry.with(console_layer).try_init()?;
    }

    #[cfg(not(feature = "tokio-console"))]
    {
        registry.try_init()?;
    }

    Ok(())
}
