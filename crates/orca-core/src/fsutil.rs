//! Filesystem helpers for files that hold secrets: TLS private keys, the ACME
//! account key, cached credentials.
//!
//! The rule these enforce: a secret file is readable by its owner only, from
//! the moment it exists, and a reader never sees it half-written.

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

/// Write `contents` to `path` so only its owner can read it, atomically.
///
/// The data goes to a temporary file in the same directory, created with
/// `create_new` and mode 0600, then is fsynced and renamed over `path`. The
/// rename is atomic, so a reader sees either the old file or the new one,
/// never a truncated one. The new inode is 0600 whatever mode the old file
/// had, so a previously world-readable file is replaced rather than rewritten
/// in place. Missing parent directories are created.
pub fn write_private(path: &Path, contents: &[u8]) -> io::Result<()> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;
    let tmp = dir.join(format!(
        ".{}.{}.tmp",
        name.to_string_lossy(),
        unique_suffix()
    ));

    let written = (|| {
        let mut file = open_private_new(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    written
}

/// Create `dir` (and parents) and restrict it to its owner (0700).
pub fn create_private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    restrict(dir, 0o700).map(|_| ())
}

/// Tighten `path` to `mode` (e.g. 0o600 or 0o700) if it grants any
/// permission bit outside `mode`. A path that is already as strict or
/// stricter is left alone. Returns whether the mode changed.
#[cfg(unix)]
pub fn restrict(path: &Path, mode: u32) -> io::Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
    if current & !mode == 0 {
        return Ok(false);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(true)
}

/// No permission bits to tighten on this platform.
#[cfg(not(unix))]
pub fn restrict(_path: &Path, _mode: u32) -> io::Result<bool> {
    Ok(false)
}

#[cfg(unix)]
fn open_private_new(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn open_private_new(path: &Path) -> io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

/// Distinct per call within a process, and across processes via the pid, so
/// concurrent writers of the same file never share a temporary.
fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
#[path = "fsutil_tests.rs"]
mod tests;
