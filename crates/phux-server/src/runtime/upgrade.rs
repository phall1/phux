//! Graceful-upgrade orchestration (ADR-0032): build the handoff blob, clear
//! `FD_CLOEXEC` on the descriptors the new image must inherit, validate the
//! on-disk binary, and re-exec it as `server --resume <fd>`.
//!
//! Split into [`prepare_upgrade`] (everything reversible — if it fails the old
//! image keeps serving and no child is stranded) and [`UpgradePlan::exec`]
//! (the irreversible re-exec). The caller acks the client between the two.

use std::collections::HashMap;
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, BorrowedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use tokio::sync::oneshot;

use crate::state::SharedState;
use crate::terminal_actor::UpgradeHandleRequest;

/// Errors preparing a graceful upgrade. Any of these leaves the running server
/// untouched (the children are never stranded — see the module docs).
#[derive(Debug, thiserror::Error)]
pub(super) enum UpgradeError {
    /// The server hasn't captured its upgrade context yet (not serving).
    #[error("server not ready for upgrade (no listener context)")]
    NoContext,
    /// The handoff blob could not be serialized.
    #[error("serialize handoff blob: {0}")]
    Blob(#[from] crate::upgrade::blob::BlobError),
    /// A descriptor / temp-file operation failed.
    #[error("upgrade io: {0}")]
    Io(#[from] std::io::Error),
    /// The on-disk binary failed its pre-commit validation, so the upgrade is
    /// aborted before anything irreversible happens.
    #[error("new binary failed validation: {0}")]
    Validation(String),
}

/// A validated, ready-to-`exec` upgrade. Holds the open blob temp file so its
/// fd stays valid until the re-exec consumes it.
pub(super) struct UpgradePlan {
    current_exe: PathBuf,
    blob_fd: RawFd,
    socket_path: PathBuf,
    _blob_file: std::fs::File,
}

/// Do everything reversible: snapshot the tree into a handoff blob, stage it in
/// an inheritable temp file, clear `FD_CLOEXEC` on the blob / listener / every
/// pane master, and validate the on-disk binary. Returns a [`UpgradePlan`] the
/// caller execs *after* acking the client.
pub(super) async fn prepare_upgrade(state: &SharedState) -> Result<UpgradePlan, UpgradeError> {
    let (listener_fd, socket_path) = state
        .with(|s| {
            s.upgrade_context()
                .map(|(fd, path)| (fd, path.to_path_buf()))
        })
        .ok_or(UpgradeError::NoContext)?;

    // Gather each pane's handoff out of lock (the state lock can't be held
    // across the await), then assemble the blob back under the lock.
    let handles = state.with(crate::state::ServerState::upgrade_handles);
    let mut handoffs = HashMap::new();
    for (tid, handle) in handles {
        let (reply, rx) = oneshot::channel();
        if handle
            .upgrade
            .send(UpgradeHandleRequest { reply })
            .await
            .is_ok()
            && let Ok(handoff) = rx.await
        {
            handoffs.insert(tid, handoff);
        }
    }
    let blob = state.with(|s| s.assemble_upgrade_blob(listener_fd, &handoffs));

    // Stage the blob in an anonymous temp file (auto-removed on close), rewound
    // so the resumed image reads from the start.
    let mut blob_file = tempfile::tempfile()?;
    blob_file.write_all(&blob.to_bytes()?)?;
    blob_file.seek(SeekFrom::Start(0))?;
    let blob_fd = blob_file.as_raw_fd();

    // Everything the re-exec'd image must inherit needs FD_CLOEXEC cleared.
    clear_cloexec(blob_fd)?;
    clear_cloexec(listener_fd)?;
    for pane in &blob.panes {
        if let Some(master_fd) = pane.master_fd {
            clear_cloexec(master_fd)?;
        }
    }

    // Pre-commit safety: refuse to re-exec a binary that can't even print its
    // version, so a half-written `cargo install` can't strand the session.
    let current_exe = std::env::current_exe()?;
    validate_binary(&current_exe)?;

    Ok(UpgradePlan {
        current_exe,
        blob_fd,
        socket_path,
        _blob_file: blob_file,
    })
}

impl UpgradePlan {
    /// Re-exec the new binary as `server --resume <blob_fd> --socket <path>`,
    /// replacing this process in place. Returns only on failure — and a
    /// failure is harmless: nothing was closed, so the old image keeps serving
    /// and the children stay attached.
    pub(super) fn exec(self) -> std::io::Error {
        Command::new(&self.current_exe)
            .arg("server")
            .arg("--resume")
            .arg(self.blob_fd.to_string())
            .arg("--socket")
            .arg(&self.socket_path)
            .exec()
    }
}

/// Clear `FD_CLOEXEC` so `fd` survives the `execve`.
fn clear_cloexec(fd: RawFd) -> std::io::Result<()> {
    use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
    // SAFETY: `fd` is open and owned by this process for the duration of the
    // two fcntl calls; we only borrow it, never take ownership.
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let flags = fcntl_getfd(borrowed)?;
    fcntl_setfd(borrowed, flags.difference(FdFlags::CLOEXEC))?;
    Ok(())
}

/// Validate the on-disk binary runs by probing `--version`.
fn validate_binary(exe: &Path) -> Result<(), UpgradeError> {
    let output = Command::new(exe).arg("--version").output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(UpgradeError::Validation(format!(
            "`{} --version` exited with {}",
            exe.display(),
            output.status
        )))
    }
}
