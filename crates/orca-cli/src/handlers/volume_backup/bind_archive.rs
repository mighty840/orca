//! Archive host bind-mount sources into the snapshot (#185).
//!
//! #83 only warned that bind mounts were outside the backup. Now every
//! distinct source is classified and, unless it is reproducible or
//! unsuitable, archived into `bind-mounts.tar.gz` next to the volume tarballs,
//! with a manifest saying where each path was mounted. Restore:
//! `tar -xzf bind-mounts.tar.gz -C /` (the manifest is at
//! `bind-mounts.manifest.json` inside the archive).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::bind_mounts::UnbackedMount;

/// Host paths whose contents are the OS or Docker, never service data.
const SYSTEM_PREFIXES: &[&str] = &[
    "/proc",
    "/sys",
    "/dev",
    "/run",
    "/var/run",
    "/var/lib/docker",
    "/etc",
    "/usr",
    "/lib",
    "/bin",
    "/sbin",
    "/boot",
];

pub(crate) const ARCHIVE: &str = "bind-mounts.tar.gz";
pub(crate) const MANIFEST: &str = "bind-mounts.manifest.json";

/// What happens to one bind-mount source.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub(crate) enum Decision {
    /// Goes into the archive.
    Archive,
    /// OS or Docker state (`/`, `/etc/…`, the Docker socket, …).
    System,
    /// Tracked and unmodified in a git work tree: restorable from the repo.
    InGit,
    /// Larger than `bind_mount_max_mb`.
    TooLarge { bytes: u64 },
    /// Missing, or a file in it can't be read by the backup user.
    Unreadable { error: String },
}

impl Decision {
    /// Whether the data is left out of the backup AND not recoverable from
    /// elsewhere, i.e. worth a warning.
    pub(crate) fn is_gap(&self) -> bool {
        matches!(
            self,
            Decision::TooLarge { .. } | Decision::Unreadable { .. }
        )
    }
}

/// Decide what to do with one source path.
pub(crate) fn classify(path: &Path, max_bytes: u64) -> Decision {
    let s = path.to_string_lossy();
    if s == "/"
        || SYSTEM_PREFIXES
            .iter()
            .any(|p| s == *p || s.starts_with(&format!("{p}/")))
    {
        return Decision::System;
    }
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Decision::Unreadable {
                error: e.to_string(),
            };
        }
    };
    if !meta.is_file() && !meta.is_dir() {
        return Decision::System; // socket, device, fifo, dangling symlink
    }
    if in_git(path) {
        return Decision::InGit;
    }
    match size_if_readable(path, max_bytes) {
        Ok(bytes) if bytes > max_bytes => Decision::TooLarge { bytes },
        Ok(_) => Decision::Archive,
        Err(e) => Decision::Unreadable { error: e },
    }
}

/// Tracked AND unmodified AND no untracked or ignored files under it. Ignored
/// files count as not-in-git: `.gitignore`d data next to tracked config is
/// exactly what can't be recreated from the repo.
fn in_git(path: &Path) -> bool {
    let dir = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .arg("--")
            .arg(path)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| o.stdout)
    };
    let tracked = git(&["ls-files"]).is_some_and(|out| !out.is_empty());
    let clean = git(&[
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--ignored",
    ])
    .is_some_and(|out| out.is_empty());
    tracked && clean
}

/// Total size, stopping once it exceeds `cap`. Every file is opened, so a
/// permission problem shows up here instead of as a half-written tar.
fn size_if_readable(path: &Path, cap: u64) -> Result<u64, String> {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(p) = stack.pop() {
        let meta = std::fs::symlink_metadata(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        if meta.is_dir() {
            for entry in std::fs::read_dir(&p).map_err(|e| format!("{}: {e}", p.display()))? {
                stack.push(entry.map_err(|e| e.to_string())?.path());
            }
        } else if meta.is_file() {
            std::fs::File::open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            total += meta.len();
            if total > cap {
                return Ok(total);
            }
        }
    }
    Ok(total)
}

/// One manifest row.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Entry {
    pub host_path: String,
    pub mounted_by: Vec<String>,
    #[serde(flatten)]
    pub decision: Decision,
}

/// Classify every distinct source, write the manifest, and tar the archivable
/// ones (plus the manifest) into `<backup_dir>/bind-mounts.tar.gz`. Returns
/// the manifest rows and whether an archive was written.
pub(crate) fn archive(
    mounts: &[UnbackedMount],
    backup_dir: &Path,
    max_bytes: u64,
) -> anyhow::Result<(Vec<Entry>, Option<PathBuf>)> {
    let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in mounts {
        by_source
            .entry(m.host_path.clone())
            .or_default()
            .push(format!("{}:{}", m.service, m.container_path));
    }
    let entries: Vec<Entry> = by_source
        .into_iter()
        .map(|(host_path, mounted_by)| Entry {
            decision: classify(Path::new(&host_path), max_bytes),
            host_path,
            mounted_by,
        })
        .collect();

    let manifest = backup_dir.join(MANIFEST);
    std::fs::write(&manifest, serde_json::to_vec_pretty(&entries)?)?;

    let sources: Vec<&str> = entries
        .iter()
        .filter(|e| e.decision == Decision::Archive)
        .map(|e| e.host_path.trim_start_matches('/'))
        .collect();
    if sources.is_empty() {
        return Ok((entries, None));
    }
    let out = backup_dir.join(ARCHIVE);
    let result = Command::new("tar")
        .arg("-czf")
        .arg(&out)
        .arg("-C")
        .arg("/")
        .args(&sources)
        .arg("-C")
        .arg(backup_dir)
        .arg(MANIFEST)
        .output()?;
    anyhow::ensure!(
        result.status.success(),
        "tar failed: {}",
        String::from_utf8_lossy(&result.stderr).trim()
    );
    Ok((entries, Some(out)))
}

/// Encrypt the archive when `age_recipients` is set (it can hold key
/// material, e.g. Harbor's signing keys), then upload it next to the volume
/// tarballs. A failed encryption stores nothing rather than plaintext.
/// Returns the stored local file.
pub(crate) fn store(
    cfg: &orca_core::backup::BackupConfig,
    archive: &Path,
    hostname: &str,
) -> anyhow::Result<PathBuf> {
    use orca_core::backup::{BackupTarget, encrypt};

    let local = if cfg.age_recipients.is_empty() {
        println!(
            "WARNING: {ARCHIVE} is stored unencrypted; bind mounts can hold keys. \
             Set [backup] age_recipients to encrypt it."
        );
        archive.to_path_buf()
    } else {
        let enc = encrypt::encrypt_to_temp(archive, &cfg.age_recipients)?;
        let dest = PathBuf::from(format!("{}{}", archive.display(), encrypt::AGE_SUFFIX));
        std::fs::copy(enc.path(), &dest)?;
        std::fs::remove_file(archive)?;
        dest
    };
    let file_name = local
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let date = chrono::Utc::now().format("%Y-%m-%d");
    for target in cfg
        .targets
        .iter()
        .filter(|t| matches!(t, BackupTarget::S3 { .. }))
    {
        let key = format!("agents/{hostname}/{date}/{file_name}");
        if let Err(e) = orca_core::backup::s3::upload(&local, target, &key) {
            tracing::error!("S3 upload failed for {file_name}: {e}");
        }
    }
    Ok(local)
}

/// Summary line (printed last: scheduled runs relay the final stdout line).
pub(crate) fn summary(entries: &[Entry]) -> String {
    let count = |f: &dyn Fn(&Decision) -> bool| entries.iter().filter(|e| f(&e.decision)).count();
    let archived = count(&|d| *d == Decision::Archive);
    let in_git = count(&|d| *d == Decision::InGit);
    let system = count(&|d| *d == Decision::System);
    let gaps = count(&|d| d.is_gap());
    let head = if gaps > 0 { "WARNING: " } else { "" };
    format!(
        "{head}Bind mounts: {archived} archived, {in_git} in git, {system} system, \
         {gaps} NOT backed up (too large or unreadable)"
    )
}

#[cfg(test)]
#[path = "bind_archive_tests.rs"]
mod tests;
