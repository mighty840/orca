//! `orca backup create`: the master's config and state files (#199).
//!
//! Beyond `cluster.db`, `secrets.json` and `cluster.toml`, a restore also
//! needs the key that decrypts `secrets.json`, the webhook registry, and the
//! TLS state (re-issuing every certificate at once runs into Let's Encrypt
//! rate limits). Those files hold key material, so they are only backed up
//! when `[backup] age_recipients` encrypts the artifacts (#117). Without it
//! they are left out with a warning, never stored in the clear.

use std::path::{Path, PathBuf};

use orca_core::backup::BackupManager;

/// How an artifact may be stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Policy {
    /// Safe to store as-is (e.g. `secrets.json`, which is itself ciphertext).
    Plain,
    /// Holds credentials but has always been backed up: keep doing so, but
    /// warn when it goes out unencrypted.
    WarnIfPlain,
    /// Key material: store only encrypted.
    EncryptedOnly,
}

/// One backup artifact: the first existing path wins.
#[derive(Debug, Clone)]
pub(crate) struct Artifact {
    pub name: &'static str,
    pub paths: Vec<PathBuf>,
    pub is_dir: bool,
    pub policy: Policy,
}

/// Everything the config backup considers, in restore order.
pub(crate) fn artifacts(home: Option<&Path>) -> Vec<Artifact> {
    let orca = |rel: &str| home.map(|h| h.join(".orca").join(rel));
    let file = |name, paths: Vec<Option<PathBuf>>, policy| Artifact {
        name,
        paths: paths.into_iter().flatten().collect(),
        is_dir: false,
        policy,
    };
    vec![
        // The key first: without it the backed-up secrets are unreadable.
        file(
            "master-key",
            vec![orca("master.key")],
            Policy::EncryptedOnly,
        ),
        file(
            "secrets",
            vec![orca("secrets.json"), Some("secrets.json".into())],
            Policy::Plain,
        ),
        file(
            "cluster",
            vec![
                home.map(|h| h.join("orca/cluster.toml")),
                orca("cluster.toml"),
                Some("cluster.toml".into()),
            ],
            Policy::WarnIfPlain,
        ),
        file("cluster-db", vec![orca("cluster.db")], Policy::Plain),
        file(
            "webhooks",
            vec![orca("webhooks.json")],
            Policy::EncryptedOnly,
        ),
        file(
            "backup-config",
            vec![orca("backup_config.json")],
            Policy::EncryptedOnly,
        ),
        file(
            "acme-account",
            vec![orca("acme-account.json")],
            Policy::EncryptedOnly,
        ),
        Artifact {
            name: "certs",
            paths: orca("certs").into_iter().collect(),
            is_dir: true,
            policy: Policy::EncryptedOnly,
        },
    ]
}

/// What a run did, for the summary and for tests.
#[derive(Debug, Default)]
pub(crate) struct Outcome {
    pub stored: Vec<String>,
    pub skipped_unencrypted: Vec<String>,
    pub stored_plain_with_credentials: Vec<String>,
    pub failed: Vec<String>,
}

/// Back up every artifact that exists, honouring its policy.
pub(crate) fn run(mgr: &BackupManager, home: Option<&Path>, prefix: &str) -> Outcome {
    let encrypted = mgr.encrypts();
    let mut out = Outcome::default();
    for a in artifacts(home) {
        let Some(path) = a.paths.iter().find(|p| p.exists()) else {
            tracing::debug!("Skipping {}: not found", a.name);
            continue;
        };
        if a.policy == Policy::EncryptedOnly && !encrypted {
            out.skipped_unencrypted.push(path.display().to_string());
            continue;
        }
        // A directory is stored as one tar.gz, so it restores as a unit.
        let tarball;
        let source: &Path = if a.is_dir {
            match tar_dir(path) {
                Ok(t) => {
                    tarball = t;
                    tarball.path()
                }
                Err(e) => {
                    tracing::error!("Failed to archive {}: {e:#}", path.display());
                    out.failed.push(a.name.to_string());
                    continue;
                }
            }
        } else {
            path
        };
        match mgr.backup_file(a.name, source, prefix) {
            Ok(()) => {
                out.stored.push(path.display().to_string());
                if a.policy == Policy::WarnIfPlain && !encrypted {
                    out.stored_plain_with_credentials
                        .push(path.display().to_string());
                }
            }
            Err(e) => {
                tracing::error!("Failed to back up {}: {e:#}", a.name);
                out.failed.push(a.name.to_string());
            }
        }
    }
    out
}

/// `tar -czf` a directory into a temp file named `*.tar.gz`, entries
/// relative to its parent (`certs/…`).
fn tar_dir(dir: &Path) -> anyhow::Result<tempfile::NamedTempFile> {
    let parent = dir.parent().unwrap_or(Path::new("."));
    let name = dir
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", dir.display()))?;
    let tmp = tempfile::Builder::new().suffix(".tar.gz").tempfile()?;
    let status = std::process::Command::new("tar")
        .arg("-czf")
        .arg(tmp.path())
        .arg("-C")
        .arg(parent)
        .arg(name)
        .status()?;
    anyhow::ensure!(status.success(), "tar exited with {status}");
    Ok(tmp)
}

/// Print what happened, loudly for anything left out or stored in the clear.
pub(crate) fn report(out: &Outcome) {
    for p in &out.stored {
        println!("Backed up: {p}");
    }
    if !out.skipped_unencrypted.is_empty() {
        eprintln!(
            "WARNING: not backed up, because these hold key material and backups are \
             not encrypted: {}. Set [backup] age_recipients in cluster.toml to include \
             them.",
            out.skipped_unencrypted.join(", ")
        );
    }
    if !out.stored_plain_with_credentials.is_empty() {
        eprintln!(
            "WARNING: stored unencrypted although they contain credentials: {}. Set \
             [backup] age_recipients to encrypt backups.",
            out.stored_plain_with_credentials.join(", ")
        );
    }
    for name in &out.failed {
        eprintln!("ERROR: backup of {name} failed (see log above)");
    }
    if out.stored.is_empty() {
        println!("No files found to backup.");
    } else {
        println!("Backup complete: {} file(s).", out.stored.len());
    }
}

#[cfg(test)]
#[path = "backup_files_tests.rs"]
mod tests;
