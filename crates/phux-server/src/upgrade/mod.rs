//! Graceful server upgrade (ADR-0032).
//!
//! Re-exec the binary in place while the PTYs, their children, and the
//! listening socket survive on inherited descriptors, then rebuild the session
//! tree from a replayed VT snapshot.
//!
//! This module is built in slices:
//! - [`blob`] — the versioned state blob handed old image → new image.
//!
//! Subsequent slices add the producer (reading live [`ServerState`] +
//! per-pane handles into a [`blob::StateBlob`]), the `--resume` consumer that
//! adopts the inherited descriptors via `portable-pty-adopt`, the re-exec
//! orchestration with its never-strand-a-child fallback, and the
//! `phux upgrade` control command.
//!
//! [`ServerState`]: crate::state::ServerState

pub mod blob;
