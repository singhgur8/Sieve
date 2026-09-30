//! User-facing errors for file access (Phase 8 error states). Every place that reads an
//! original or writes a sidecar/export maps `std::io::Error`s through here so the message
//! says what happened and what to do, with a specific [`ErrorKind`] (IPC v13):
//! - original missing / moved / unmounted volume -> `file_missing` ([`missing_original`]);
//! - read-only volume or no permission -> `read_only` ("... is read-only" / "permission denied");
//! - disk full -> `disk_full` ("... disk is full"), see [`is_disk_full`];
//! - decoder failure on an existing original -> `decode_failed` ([`decode_failed`]);
//! - anything else -> `io` with the OS message.

use std::io;
use std::path::Path;

use crate::ipc::error::{AppError, ErrorKind};

/// Prefix of every missing-original message (also used to recognize them in per-file
/// reports, e.g. export failures and thumbnail failure reasons).
pub const MISSING_PREFIX: &str = "Original file is missing";

/// ENOSPC / EDQUOT (macOS + Linux).
const ENOSPC: i32 = 28;
#[cfg(target_os = "macos")]
const EDQUOT: i32 = 69;
#[cfg(not(target_os = "macos"))]
const EDQUOT: i32 = 122;
/// EROFS.
const EROFS: i32 = 30;

/// `file_missing` for an original that is not at its catalogued path.
pub fn missing_original(path: &Path) -> AppError {
    AppError::new(ErrorKind::FileMissing, missing_message(path))
}

/// True if a per-file failure `reason` (a plain string in reports) is a missing original.
pub fn is_missing_message(reason: &str) -> bool {
    reason.starts_with(MISSING_PREFIX)
}

/// Message of [`missing_original`] (per-file reports carry plain strings).
pub fn missing_message(path: &Path) -> String {
    format!(
        "{MISSING_PREFIX} or was moved: {}. Reconnect the drive or move the file back, then try again.",
        path.display()
    )
}

/// `Ok` if `path` (an original) exists; else [`missing_original`]. A volume that is
/// present but unreadable yields an `io` error explaining the permission problem.
pub fn require_original(path: &Path) -> Result<(), AppError> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => Err(AppError::invalid(format!("{} is not a file", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(missing_original(path)),
        Err(e) => Err(io_error(path, "read", &e)),
    }
}

/// True for "no space left" / quota errors.
pub fn is_disk_full(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::StorageFull || matches!(e.raw_os_error(), Some(ENOSPC) | Some(EDQUOT))
}

/// True for read-only volume / permission errors.
pub fn is_read_only(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::PermissionDenied
        || e.kind() == io::ErrorKind::ReadOnlyFilesystem
        || e.raw_os_error() == Some(EROFS)
}

/// Actionable message for an I/O failure while `verb`-ing (`"read"`, `"write"`) `path`.
pub fn io_message(path: &Path, verb: &str, e: &io::Error) -> String {
    let p = path.display();
    if is_disk_full(e) {
        format!("Could not {verb} {p}: the disk is full. Free up space or choose another destination.")
    } else if e.kind() == io::ErrorKind::ReadOnlyFilesystem || e.raw_os_error() == Some(EROFS) {
        format!("Could not {verb} {p}: the volume is read-only. Choose a writable location.")
    } else if e.kind() == io::ErrorKind::PermissionDenied {
        format!(
            "Could not {verb} {p}: permission denied. Check the folder's permissions (Finder > Get Info) \
             or choose another location."
        )
    } else if e.kind() == io::ErrorKind::NotFound && verb == "read" {
        missing_message(path)
    } else if e.kind() == io::ErrorKind::NotFound {
        format!("Could not {verb} {p}: the folder no longer exists (moved or its drive was disconnected).")
    } else {
        format!("Could not {verb} {p}: {e}")
    }
}

/// The [`ErrorKind`] of an I/O failure while `verb`-ing `path` (see the module docs).
pub fn io_kind(verb: &str, e: &io::Error) -> ErrorKind {
    if is_disk_full(e) {
        ErrorKind::DiskFull
    } else if is_read_only(e) {
        ErrorKind::ReadOnly
    } else if e.kind() == io::ErrorKind::NotFound && verb == "read" {
        ErrorKind::FileMissing
    } else {
        ErrorKind::Io
    }
}

/// [`io_message`] as an [`AppError`] of kind [`io_kind`].
pub fn io_error(path: &Path, verb: &str, e: &io::Error) -> AppError {
    AppError::new(io_kind(verb, e), io_message(path, verb, e))
}

/// `decode_failed`: wording for a decoder failure on an original that exists.
pub fn decode_failed(path: &Path, detail: &str) -> AppError {
    AppError::new(
        ErrorKind::DecodeFailed,
        format!(
            "Could not decode {}: {detail}. The file may be damaged, still copying, or from an unsupported camera.",
            path.display()
        ),
    )
}

/// Tests: pretend the volume under a directory prefix has this many bytes free.
#[cfg(test)]
pub(crate) static FREE_BYTES_OVERRIDE: std::sync::Mutex<Vec<(std::path::PathBuf, u64)>> =
    std::sync::Mutex::new(Vec::new());

/// Free bytes available to this user on the volume holding `dir` (`None` if unknown).
pub fn available_bytes(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    #[cfg(test)]
    if let Some((_, v)) =
        FREE_BYTES_OVERRIDE.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(p, _)| dir.starts_with(p))
    {
        return Some(*v);
    }
    let c = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills a zeroed struct we own; `c` is a valid NUL-terminated path.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// `disk_full` "not enough disk space" error unless the volume of `dir` has `needed` bytes free
/// (unknown free space passes: the write itself then reports ENOSPC).
pub fn ensure_space(dir: &Path, needed: u64) -> Result<(), AppError> {
    match available_bytes(dir) {
        Some(free) if free < needed => Err(AppError::new(
            ErrorKind::DiskFull,
            format!(
                "Not enough disk space in {}: {} MB free, about {} MB needed. Free up space or choose another \
                 destination.",
                dir.display(),
                free / (1 << 20),
                needed.div_ceil(1 << 20)
            ),
        )),
        _ => Ok(()),
    }
}

/// True if `message` is one of this module's disk-full messages.
pub fn is_disk_full_message(message: &str) -> bool {
    message.contains("the disk is full") || message.starts_with("Not enough disk space")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_and_words_errors() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("DSC0001.ARW");
        let e = require_original(&gone).unwrap_err();
        assert_eq!(e.kind, ErrorKind::FileMissing);
        assert!(is_missing_message(&e.message));
        assert!(e.message.starts_with(MISSING_PREFIX) && e.message.contains("DSC0001.ARW"), "{}", e.message);
        std::fs::write(&gone, b"x").unwrap();
        require_original(&gone).unwrap();
        assert_eq!(require_original(dir.path()).unwrap_err().kind, ErrorKind::InvalidArgument);

        let full = io::Error::from_raw_os_error(ENOSPC);
        assert!(is_disk_full(&full));
        assert!(io_message(Path::new("/o/a.jpg"), "write", &full).contains("disk is full"));
        let ro = io::Error::from_raw_os_error(EROFS);
        assert!(is_read_only(&ro));
        assert!(io_message(Path::new("/o/a.xmp"), "write", &ro).contains("read-only"));
        let denied = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(io_message(Path::new("/o/a.xmp"), "write", &denied).contains("permission denied"));
        assert_eq!(io_error(Path::new("/o/a.xmp"), "write", &denied).kind, ErrorKind::ReadOnly);
        assert_eq!(io_error(Path::new("/o/a.xmp"), "write", &ro).kind, ErrorKind::ReadOnly);
        assert_eq!(io_error(Path::new("/o/a.jpg"), "write", &full).kind, ErrorKind::DiskFull);
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(io_error(Path::new("/o/a.arw"), "read", &nf).kind, ErrorKind::FileMissing);
        assert_eq!(io_error(Path::new("/o/x/a.jpg"), "write", &nf).kind, ErrorKind::Io);
        assert_eq!(decode_failed(Path::new("/o/a.arw"), "bad").kind, ErrorKind::DecodeFailed);
        assert!(io_message(Path::new("/o/x/a.jpg"), "write", &nf).contains("no longer exists"));

        let free = available_bytes(dir.path()).unwrap();
        assert!(free > 0);
        ensure_space(dir.path(), 1).unwrap();
        let e = ensure_space(dir.path(), u64::MAX).unwrap_err();
        assert_eq!(e.kind, ErrorKind::DiskFull);
        assert!(is_disk_full_message(&e.message), "{}", e.message);
        assert!(is_disk_full_message(&io_message(Path::new("/o/a.jpg"), "write", &full)));
    }
}
