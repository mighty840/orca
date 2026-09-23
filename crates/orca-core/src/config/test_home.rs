//! Isolation for tests that reach `$HOME/.orca`.
//!
//! `SecretStore::open` always reads `$HOME/.orca/master.key`, wherever the
//! secrets file itself lives, and creates it if it is missing.
//! `ClusterConfig::load` reaches it too, through the default store. So every
//! test that opens a store or loads a cluster config touches `$HOME`, and must
//! run under [`TempHome`]. Otherwise it races the other such tests: two of them
//! each generate a different `master.key` in the same directory, and one then
//! decrypts with the wrong key. It would also read and write the real
//! `~/.orca` of whoever runs `cargo test`.

use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

static HOME_LOCK: Mutex<()> = Mutex::new(());

/// Holds the lock, points `$HOME` at a fresh temporary directory, and restores
/// the previous value on drop, including when the test panics.
pub(super) struct TempHome {
    dir: tempfile::TempDir,
    prev: Option<OsString>,
    // Declared last so it is released last, after `drop` restores HOME.
    _lock: MutexGuard<'static, ()>,
}

impl TempHome {
    pub(super) fn new() -> Self {
        // A panicking test poisons the lock. The next test should still run,
        // not fail on a PoisonError that buries the original failure.
        let lock = HOME_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temporary HOME");
        let prev = std::env::var_os("HOME");
        // SAFETY: every test that reads or writes HOME holds HOME_LOCK.
        unsafe { std::env::set_var("HOME", dir.path()) };
        Self {
            dir,
            prev,
            _lock: lock,
        }
    }

    pub(super) fn path(&self) -> &Path {
        self.dir.path()
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        // SAFETY: HOME_LOCK is still held; the guard field drops after this.
        unsafe {
            match &self.prev {
                Some(home) => std::env::set_var("HOME", home),
                None => std::env::remove_var("HOME"),
            }
        }
    }
}
