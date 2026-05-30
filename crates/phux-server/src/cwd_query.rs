//! Kernel-side current-working-directory query for a live PTY child
//! (phux-cs6).
//!
//! `defaults.cwd-inheritance = inherit-focused` opens a freshly-spawned
//! pane in the focused pane's *live* working directory — the directory
//! the shell is in *now*, after any `cd`. There is no portable VT signal
//! for this: OSC 7 only works when the shell is configured to emit it,
//! and the bundled libghostty build surfaces OSC 7 as an opaque
//! `ReportPwd` command without exposing the announced path, so
//! `Terminal::pwd` never populates from the byte stream. We therefore ask
//! the kernel directly for the child process's CWD.
//!
//! * **Linux** reads the `/proc/<pid>/cwd` symlink (`readlink`) — pure
//!   safe std I/O.
//! * **macOS** calls `proc_pidinfo(PROC_PIDVNODEPATHINFO)` and reads the
//!   current-directory vnode path (`pvi_cdir.vip_path`) — one `unsafe`
//!   FFI block, isolated here.
//! * **Other targets** return `None`; the caller falls back to a
//!   non-inherited default.
//!
//! The query is best-effort: a dead child, a permission error, or an
//! unsupported platform all yield `None`, never an error the spawn path
//! has to handle.

use std::path::PathBuf;

/// Best-effort current working directory of process `pid`.
///
/// Returns `None` when the pid is unknown, the process has exited, the
/// query is denied, or the platform is unsupported. The caller treats
/// `None` as "do not override the spawn CWD."
#[must_use]
pub fn process_cwd(pid: u32) -> Option<PathBuf> {
    platform::process_cwd(pid)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::PathBuf;

    pub(super) fn process_cwd(pid: u32) -> Option<PathBuf> {
        // The kernel maintains `/proc/<pid>/cwd` as a symlink to the
        // process's current directory; resolving it reflects any `cd`
        // since spawn. `read_link` returns the resolved target.
        std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::CStr;
    use std::os::raw::c_void;
    use std::path::PathBuf;

    pub(super) fn process_cwd(pid: u32) -> Option<PathBuf> {
        // `pid` is the PTY child (a shell). pid 0 is never a real child
        // here; reject it so we never query the kernel for the calling
        // process by accident.
        let pid = i32::try_from(pid).ok().filter(|p| *p > 0)?;

        let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>();
        // SAFETY: `proc_pidinfo` fills at most `size` bytes into `&mut
        // info`, which is a zeroed, correctly-aligned, owned
        // `proc_vnodepathinfo` of exactly that size. `pid` is validated
        // positive above. The call only reads kernel state for `pid` and
        // writes into our buffer; it has no other side effects. A return
        // value other than the full struct size (including 0 on a dead
        // pid or EPERM) is treated as "no answer."
        let size_i32 = i32::try_from(size).ok()?;
        let written = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDVNODEPATHINFO,
                0,
                std::ptr::addr_of_mut!(info).cast::<c_void>(),
                size_i32,
            )
        };
        // `proc_pidinfo` returns the number of bytes filled (the full
        // struct size on success) or <= 0 on failure (dead pid, EPERM).
        // A short write means the struct was not fully populated.
        if written < size_i32 {
            return None;
        }

        // `pvi_cdir.vip_path` is a NUL-terminated path in a fixed
        // `[[c_char; 32]; 32]` buffer (libc models MAXPATHLEN this way).
        // Reinterpret it as a flat byte slice and read up to the NUL.
        let raw = &info.pvi_cdir.vip_path;
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(raw.as_ptr().cast::<u8>(), std::mem::size_of_val(raw))
        };
        // SAFETY: the slice is bounded by the struct field's own size and
        // `vip_path` is documented to be NUL-terminated by the kernel; if
        // a NUL is somehow absent, `CStr::from_bytes_until_nul` returns an
        // error rather than reading out of bounds.
        let cstr = CStr::from_bytes_until_nul(bytes).ok()?;
        let path = cstr.to_str().ok()?;
        if path.is_empty() {
            return None;
        }
        Some(PathBuf::from(path))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod platform {
    use std::path::PathBuf;

    pub(super) fn process_cwd(_pid: u32) -> Option<PathBuf> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::process_cwd;

    /// The current process's own CWD is queryable and matches
    /// `std::env::current_dir` on supported platforms. On unsupported
    /// platforms the query returns `None` and the assertion is skipped.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn process_cwd_of_self_matches_current_dir() {
        let pid = std::process::id();
        let got = process_cwd(pid).expect("self CWD should be queryable");
        let expected = std::env::current_dir().expect("current_dir");
        // Both paths come from the kernel's view; compare canonicalized
        // forms so a symlinked temp/working dir does not spuriously
        // differ.
        let got = got.canonicalize().unwrap_or(got);
        let expected = expected.canonicalize().unwrap_or(expected);
        assert_eq!(got, expected);
    }

    /// A pid that cannot be a live child (0) yields `None`, never a
    /// bogus path.
    #[test]
    fn process_cwd_of_pid_zero_is_none() {
        assert_eq!(process_cwd(0), None);
    }

    /// An almost-certainly-dead pid yields `None` rather than panicking.
    #[test]
    fn process_cwd_of_unlikely_pid_is_none() {
        // 2^31-1 is past any real pid on the supported platforms.
        assert_eq!(process_cwd(u32::MAX >> 1), None);
    }
}
