//! Restoring backups, locally or from S3 (#200).
//!
//! Before: the S3 listing wasn't recursive, so no backup could be found;
//! downloads went to a relative path that didn't exist; volumes couldn't be
//! restored from S3; and config files were copied into the current directory
//! instead of where the server reads them. Here every artifact is fetched
//! into a staging directory, decrypted if it is `.age` (#199), and installed
//! at its real location. An existing file is moved aside, never overwritten.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use orca_core::backup::{BackupConfig, BackupTarget, encrypt};

/// Config artifacts in the order a restore must apply them: the key first,
/// or the restored secrets can't be opened.
pub(crate) const RESTORE_ORDER: &[&str] = &[
    "master-key",
    "secrets",
    "cluster",
    "cluster-db",
    "webhooks",
    "backup-config",
    "acme-account",
    "certs",
];

/// A backup artifact name parsed as `<name>_<timestamp>.<ext>[.age]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Parsed {
    pub name: String,
    pub timestamp: String,
    pub ext: String,
    pub encrypted: bool,
}

pub(crate) fn parse(file_name: &str) -> Option<Parsed> {
    let base = file_name.rsplit('/').next()?;
    let (base, encrypted) = match base.strip_suffix(encrypt::AGE_SUFFIX) {
        Some(b) => (b, true),
        None => (base, false),
    };
    let (name, rest) = base.split_once('_')?;
    let (timestamp, ext) = rest.split_once('.')?;
    (!name.is_empty() && !timestamp.is_empty()).then(|| Parsed {
        name: name.into(),
        timestamp: timestamp.into(),
        ext: ext.into(),
        encrypted,
    })
}

/// Where a config artifact goes on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Destination {
    /// Replace this file.
    File(PathBuf),
    /// Extract this tar.gz into the directory (`certs/` → `~/.orca/certs`).
    ExtractInto(PathBuf),
}

pub(crate) fn destination(name: &str, home: &Path) -> Option<Destination> {
    let orca = home.join(".orca");
    let file = |f: &str| Some(Destination::File(orca.join(f)));
    match name {
        "master-key" => file("master.key"),
        "secrets" => file("secrets.json"),
        // The server runs from the config checkout (~/orca) and reads
        // ./cluster.toml there; without a checkout, the ~/.orca fallback.
        "cluster" if home.join("orca").is_dir() => {
            Some(Destination::File(home.join("orca/cluster.toml")))
        }
        "cluster" => file("cluster.toml"),
        "cluster-db" => file("cluster.db"),
        "webhooks" => file("webhooks.json"),
        "backup-config" => file("backup_config.json"),
        "acme-account" => file("acme-account.json"),
        "certs" => Some(Destination::ExtractInto(orca)),
        _ => None,
    }
}

/// One artifact found on a backup target.
#[derive(Debug, Clone)]
pub(crate) struct Found {
    pub parsed: Parsed,
    /// Local path, or the S3 key relative to the target's prefix.
    pub location: String,
    pub target: BackupTarget,
}

/// Every parseable artifact on every configured target.
pub(crate) fn collect(config: &BackupConfig) -> Vec<Found> {
    let mut found = Vec::new();
    for target in &config.targets {
        let names: Vec<String> = match target {
            BackupTarget::Local { path } => std::fs::read_dir(path)
                .map(|d| {
                    d.flatten()
                        .map(|e| e.path().display().to_string())
                        .collect()
                })
                .unwrap_or_default(),
            BackupTarget::S3 { .. } => match orca_core::backup::s3::list_objects(target) {
                Ok(keys) => keys,
                Err(e) => {
                    eprintln!("Cannot list {}: {e:#}", describe(target));
                    Vec::new()
                }
            },
        };
        for location in names {
            if let Some(parsed) = parse(&location) {
                found.push(Found {
                    parsed,
                    location,
                    target: target.clone(),
                });
            }
        }
    }
    found
}

/// The newest artifact of each name (timestamps are `%Y%m%dT%H%M%SZ`, so a
/// string comparison is chronological).
pub(crate) fn latest_per_name(found: Vec<Found>) -> BTreeMap<String, Found> {
    let mut latest: BTreeMap<String, Found> = BTreeMap::new();
    for f in found {
        let newer = latest
            .get(&f.parsed.name)
            .is_none_or(|cur| f.parsed.timestamp > cur.parsed.timestamp);
        if newer {
            latest.insert(f.parsed.name.clone(), f);
        }
    }
    latest
}

pub(crate) fn describe(target: &BackupTarget) -> String {
    match target {
        BackupTarget::Local { path } => format!("local {path}"),
        BackupTarget::S3 { bucket, .. } => format!("s3://{bucket}"),
    }
}

/// Put the artifact into `staging` (download from S3, or reference the local
/// file) and return its local path.
pub(crate) fn fetch(found: &Found, staging: &Path) -> Result<PathBuf> {
    match &found.target {
        BackupTarget::Local { .. } => Ok(PathBuf::from(&found.location)),
        BackupTarget::S3 { .. } => {
            let dest = staging.join(&found.location);
            orca_core::backup::s3::download(&found.target, &found.location, &dest)?;
            Ok(dest)
        }
    }
}

/// Read the `AGE-SECRET-KEY-…` line from an identity file (age-keygen's output
/// has comment lines above it).
pub(crate) fn read_identity(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    text.lines()
        .map(str::trim)
        .find(|l| l.starts_with("AGE-SECRET-KEY-"))
        .map(String::from)
        .with_context(|| format!("no AGE-SECRET-KEY line in {}", path.display()))
}

/// The artifact's plaintext bytes, decrypting `.age` with `identity`.
pub(crate) fn plaintext(path: &Path, encrypted: bool, identity: Option<&str>) -> Result<Vec<u8>> {
    if !encrypted {
        return std::fs::read(path).with_context(|| format!("read {}", path.display()));
    }
    let Some(identity) = identity else {
        bail!(
            "{} is age-encrypted; pass --identity <age key file>",
            path.display()
        );
    };
    encrypt::decrypt_file(path, identity)
}

/// Install `bytes` at `dest` (owner-only). An existing file is renamed to
/// `<dest>.pre-restore-<unix time>` first and that path is returned.
pub(crate) fn install_file(dest: &Path, bytes: &[u8]) -> Result<Option<PathBuf>> {
    let aside = move_aside(dest)?;
    orca_core::fsutil::write_private(dest, bytes)
        .with_context(|| format!("write {}", dest.display()))?;
    Ok(aside)
}

/// Extract a tar.gz (given as bytes) into `dir`. Every top-level entry that
/// already exists there is moved aside first.
pub(crate) fn extract_into(dir: &Path, tar_gz: &[u8], staging: &Path) -> Result<Vec<PathBuf>> {
    let archive = staging.join("extract.tar.gz");
    std::fs::write(&archive, tar_gz)?;
    let list = std::process::Command::new("tar")
        .arg("-tzf")
        .arg(&archive)
        .output()?;
    anyhow::ensure!(list.status.success(), "not a readable tar.gz");
    let mut tops: Vec<String> = String::from_utf8_lossy(&list.stdout)
        .lines()
        .filter_map(|l| {
            l.trim_start_matches("./")
                .split('/')
                .next()
                .map(String::from)
        })
        .filter(|t| !t.is_empty())
        .collect();
    tops.sort();
    tops.dedup();
    let mut aside = Vec::new();
    for top in &tops {
        if let Some(p) = move_aside(&dir.join(top))? {
            aside.push(p);
        }
    }
    std::fs::create_dir_all(dir)?;
    let out = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&archive)
        .arg("-C")
        .arg(dir)
        .output()?;
    anyhow::ensure!(
        out.status.success(),
        "tar: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(aside)
}

fn move_aside(path: &Path) -> Result<Option<PathBuf>> {
    if std::fs::symlink_metadata(path).is_err() {
        return Ok(None);
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let aside = PathBuf::from(format!("{}.pre-restore-{stamp}", path.display()));
    std::fs::rename(path, &aside).with_context(|| format!("move {} aside", path.display()))?;
    Ok(Some(aside))
}

/// Whether an orca server answers on the local API port. Restoring its state
/// files underneath a running server would be overwritten by the server
/// again (or corrupt cluster.db).
pub(crate) fn server_running(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
