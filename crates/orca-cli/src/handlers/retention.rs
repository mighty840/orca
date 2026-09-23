//! Retention after a backup run (#204): local snapshot directories, local
//! config artifacts, and (with `prune_s3`) the S3 targets. The per-artifact
//! decisions live in `orca_core::backup::retention`.

use orca_core::backup::{BackupConfig, BackupManager, BackupTarget, retention, s3};

use super::backup_report::BackupReport;

/// Prune only after a successful run. A failed run keeps everything, so a
/// streak of failures can't eat the last good backups: the age rule and the
/// `keep_min` floor only ever act on top of a fresh snapshot.
pub(crate) fn apply(cfg: &BackupConfig, mgr: &BackupManager, report: &mut BackupReport) {
    if !report.ok() {
        report.retention = Some("retention skipped (this run failed, nothing was pruned)".into());
        return;
    }
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let dirs = super::volume_backup::prune_old_backup_dirs(cfg.retention_days, cfg.keep_min, now);
    let files = mgr.prune_local_files(now);
    let s3_part = if cfg.prune_s3 {
        let (deleted, errors) = prune_s3(cfg, now);
        if errors > 0 {
            // Not a backup failure (nothing was lost), but visible.
            format!("{deleted} S3 object(s) pruned, {errors} delete(s) FAILED")
        } else {
            format!("{deleted} S3 object(s) pruned")
        }
    } else if cfg
        .targets
        .iter()
        .any(|t| matches!(t, BackupTarget::S3 { .. }))
    {
        "S3 not pruned (prune_s3 = false)".into()
    } else {
        String::new()
    };
    let mut text = format!(
        "retention {}d/keep {}: {dirs} snapshot(s), {files} file(s) pruned",
        cfg.retention_days, cfg.keep_min
    );
    if !s3_part.is_empty() {
        text.push_str(", ");
        text.push_str(&s3_part);
    }
    report.retention = Some(text);
}

/// Delete what `retention::s3_prune_plan` selects on every S3 target.
/// Returns (deleted, failed deletes).
fn prune_s3(cfg: &BackupConfig, now: u64) -> (usize, usize) {
    let (mut deleted, mut errors) = (0, 0);
    for target in cfg
        .targets
        .iter()
        .filter(|t| matches!(t, BackupTarget::S3 { .. }))
    {
        let keys = match s3::list_objects(target) {
            Ok(k) => k,
            Err(e) => {
                eprintln!("S3 retention: cannot list objects: {e:#}");
                errors += 1;
                continue;
            }
        };
        for key in retention::s3_prune_plan(&keys, now, cfg.retention_days, cfg.keep_min as usize) {
            match s3::delete(target, &key) {
                Ok(()) => {
                    tracing::info!("S3 retention: deleted {key}");
                    deleted += 1;
                }
                Err(e) => {
                    eprintln!("S3 retention: {e:#}");
                    errors += 1;
                }
            }
        }
    }
    (deleted, errors)
}
