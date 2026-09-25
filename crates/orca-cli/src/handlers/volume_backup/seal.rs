//! Encrypting volume tarballs (#231).
//!
//! With `[backup] age_recipients` set, config files, secrets and bind-mount
//! archives were encrypted, but volume tarballs, including every database,
//! were stored and uploaded in plaintext. Each tarball is now sealed right
//! after it is written: encrypted to `<volume>.tar.gz.age`, plaintext removed.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use orca_core::backup::encrypt;

/// `<volume>.tar.gz` in `dir`: the name the backup container writes and the
/// restore container reads.
pub(super) fn plain_path(dir: &Path, volume: &str) -> PathBuf {
    dir.join(format!("{volume}.tar.gz"))
}

/// The same path with the `.age` suffix.
pub(super) fn sealed_path(dir: &Path, volume: &str) -> PathBuf {
    dir.join(format!("{volume}.tar.gz{}", encrypt::AGE_SUFFIX))
}

/// Encrypt `volume`'s tarball in `dir` when recipients are configured. The
/// plaintext is removed whether or not encryption succeeds: with encryption
/// on, a volume is stored encrypted or not at all, never silently in clear.
pub(super) fn seal(recipients: &[String], dir: &Path, volume: &str) -> Result<()> {
    if recipients.is_empty() {
        return Ok(());
    }
    let plain = plain_path(dir, volume);
    let result = encrypt::encrypt_file(&plain, &sealed_path(dir, volume), recipients);
    let removed = std::fs::remove_file(&plain)
        .with_context(|| format!("remove plaintext {}", plain.display()));
    result.and(removed)
}

/// The tarball to upload for `volume`: the sealed one if present, else the
/// plaintext one (encryption off), else `None`.
pub(super) fn tarball(dir: &Path, volume: &str) -> Option<PathBuf> {
    [sealed_path(dir, volume), plain_path(dir, volume)]
        .into_iter()
        .find(|p| p.exists())
}

/// Make `<volume>.tar.gz` available for the restore container: `dir` itself
/// when the plaintext is there, otherwise a private staging directory holding
/// the decryption of `<volume>.tar.gz.age`.
pub(super) fn plaintext_dir(dir: &Path, volume: &str, identity: Option<&str>) -> Result<PathBuf> {
    if plain_path(dir, volume).exists() {
        return Ok(dir.to_path_buf());
    }
    let sealed = sealed_path(dir, volume);
    anyhow::ensure!(
        sealed.exists(),
        "no backup of {volume} in {}",
        dir.display()
    );
    let identity = identity.ok_or_else(|| {
        anyhow::anyhow!(
            "{} is age-encrypted; pass --identity <age key file>",
            sealed.display()
        )
    })?;
    let staging = staging_dir(volume)?;
    encrypt::decrypt_to_file(&sealed, identity, &plain_path(&staging, volume))?;
    Ok(staging)
}

/// A fresh owner-only directory under `~/.orca/restore/`.
pub(super) fn staging_dir(volume: &str) -> Result<PathBuf> {
    let home = dirs_next::home_dir().ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    let stamp = chrono::Utc::now().timestamp();
    let dir = home.join(format!(".orca/restore/{stamp}-{volume}"));
    orca_core::fsutil::create_private_dir(&dir)?;
    Ok(dir)
}

#[cfg(test)]
#[path = "seal_tests.rs"]
mod tests;
