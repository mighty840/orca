//! #197: `orca backup` exits non-zero when the backup fails, and its last
//! stdout line is the run's real summary. The scheduler and agents record
//! both, so before this every failed night showed up as a success.
//!
//! Uses `backup basic` (config files only), so no Docker is needed.

use std::path::Path;
use std::process::Command;

/// Run `orca backup basic` against a fake $HOME with one local target.
fn backup_basic(home: &Path, target: &Path) -> (bool, String) {
    let cfg = serde_json::json!({
        "targets": [{ "type": "local", "path": target.display().to_string() }]
    });
    let out = Command::new(env!("CARGO_BIN_EXE_orca"))
        .args(["backup", "basic"])
        .current_dir(home)
        .env("HOME", home)
        .env("ORCA_BACKUP_CONFIG_JSON", cfg.to_string())
        .output()
        .expect("run orca");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_string();
    (out.status.success(), last)
}

fn home_with_config_files() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let orca = home.path().join(".orca");
    std::fs::create_dir_all(&orca).unwrap();
    std::fs::write(orca.join("secrets.json"), r#"{"secrets":{}}"#).unwrap();
    std::fs::write(orca.join("cluster.db"), "db").unwrap();
    home
}

#[test]
fn a_successful_backup_exits_zero_and_summarizes() {
    let home = home_with_config_files();
    let target = home.path().join("backups");
    let (ok, last) = backup_basic(home.path(), &target);
    assert!(ok, "exit status should be success; last line: {last}");
    assert!(
        last.starts_with("Backup OK: config files 2 stored"),
        "{last}"
    );
}

#[test]
fn a_failed_backup_exits_nonzero_and_says_so() {
    let home = home_with_config_files();
    // The target "directory" is a file, so every store fails.
    let target = home.path().join("not-a-dir");
    std::fs::write(&target, "x").unwrap();
    let (ok, last) = backup_basic(home.path(), &target);
    assert!(!ok, "a failed backup must exit non-zero; last line: {last}");
    assert!(last.starts_with("Backup FAILED (2)"), "{last}");
}
