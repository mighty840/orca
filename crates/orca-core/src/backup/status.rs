//! Backup dashboard types and local-disk enumeration.
//!
//! Used by both the master (its own local snapshots) and the agent (reports
//! its snapshots to master via WS). Keeping the enumerator in `orca-core` means
//! both sides see the same data shape regardless of node role.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One backup run on disk — one timestamped directory under `~/.orca/backups/`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupSnapshotSummary {
    /// Unix epoch seconds; also the directory name on disk.
    pub epoch_secs: u64,
    /// Sum of `files[*].size_bytes` — surfaced for the dashboard top row so a
    /// client does not need to re-sum it.
    pub total_size_bytes: u64,
    /// Files inside this snapshot dir, sorted alphabetically.
    pub files: Vec<BackupFileEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFileEntry {
    pub name: String,
    pub size_bytes: u64,
}

/// Walk `<home>/.orca/backups/<epoch>/<file>` and return one summary per
/// timestamped directory, newest first. Top-level entries that are not
/// numerically-named directories are ignored — that filters out stray files
/// (lockfiles, README, etc.) without erroring. A missing base directory
/// returns an empty list rather than an error, since "no backups yet" is a
/// legitimate state on a fresh node.
pub fn enumerate_local_backups(home: &Path) -> Vec<BackupSnapshotSummary> {
    let base = home.join(".orca/backups");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut snapshots: Vec<BackupSnapshotSummary> = entries
        .filter_map(Result::ok)
        .filter_map(|e| snapshot_from_dir(&e.path()))
        .collect();
    snapshots.sort_by(|a, b| b.epoch_secs.cmp(&a.epoch_secs));
    snapshots
}

fn snapshot_from_dir(path: &Path) -> Option<BackupSnapshotSummary> {
    if !path.is_dir() {
        return None;
    }
    let epoch_secs: u64 = path.file_name()?.to_str()?.parse().ok()?;
    let files = enumerate_files(path);
    let total_size_bytes = files.iter().map(|f| f.size_bytes).sum();
    Some(BackupSnapshotSummary {
        epoch_secs,
        total_size_bytes,
        files,
    })
}

fn enumerate_files(dir: &Path) -> Vec<BackupFileEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<BackupFileEntry> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if !path.is_file() {
                return None;
            }
            let name = path.file_name()?.to_str()?.to_string();
            let size_bytes = std::fs::metadata(&path).ok()?.len();
            Some(BackupFileEntry { name, size_bytes })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(dir: &Path, name: &str, contents: &[u8]) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join(name), contents).unwrap();
    }

    /// Fresh node, no backup directory at all — must be empty, not panic.
    #[test]
    fn missing_base_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        assert!(enumerate_local_backups(tmp.path()).is_empty());
    }

    /// One snapshot with two files: totals roll up; files are sorted.
    #[test]
    fn one_snapshot_two_files() {
        let tmp = TempDir::new().unwrap();
        let snap = tmp.path().join(".orca/backups/1700000000");
        write_file(&snap, "b-vol.tar.gz", &[0u8; 100]);
        write_file(&snap, "a-vol.tar.gz", &[0u8; 50]);

        let snaps = enumerate_local_backups(tmp.path());
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].epoch_secs, 1_700_000_000);
        assert_eq!(snaps[0].total_size_bytes, 150);
        assert_eq!(
            snaps[0]
                .files
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a-vol.tar.gz", "b-vol.tar.gz"],
        );
    }

    /// Multiple snapshots: newest epoch comes first so the dashboard "last
    /// run" lookup is just `snaps[0]`.
    #[test]
    fn multiple_snapshots_sorted_newest_first() {
        let tmp = TempDir::new().unwrap();
        for epoch in [1_700_000_000u64, 1_700_086_400, 1_700_172_800] {
            let snap = tmp.path().join(format!(".orca/backups/{epoch}"));
            write_file(&snap, "vol.tar.gz", &[0u8; 10]);
        }
        let snaps = enumerate_local_backups(tmp.path());
        assert_eq!(
            snaps.iter().map(|s| s.epoch_secs).collect::<Vec<_>>(),
            vec![1_700_172_800, 1_700_086_400, 1_700_000_000],
        );
    }

    /// Non-numeric or non-directory entries at the top level are silently
    /// ignored — a stray `README` or `.DS_Store` must not break the listing.
    #[test]
    fn ignores_non_numeric_and_non_dir_entries() {
        let tmp = TempDir::new().unwrap();
        let base = tmp.path().join(".orca/backups");
        fs::create_dir_all(&base).unwrap();
        // Stray file at the backups root.
        fs::write(base.join("README"), b"hi").unwrap();
        // Non-numeric subdir.
        fs::create_dir(base.join("scratch")).unwrap();
        // Real snapshot.
        write_file(&base.join("42"), "vol.tar.gz", &[0u8; 5]);

        let snaps = enumerate_local_backups(tmp.path());
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].epoch_secs, 42);
    }

    /// An empty snapshot dir (run that failed before writing anything) shows
    /// up with zero files and zero bytes rather than being filtered out —
    /// the operator should be able to see that an empty run occurred.
    #[test]
    fn empty_snapshot_dir_is_reported_with_zero_size() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".orca/backups/99")).unwrap();
        let snaps = enumerate_local_backups(tmp.path());
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].total_size_bytes, 0);
        assert!(snaps[0].files.is_empty());
    }
}
