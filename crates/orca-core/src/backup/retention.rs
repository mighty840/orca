//! Which backups to delete (#204).
//!
//! Two rules, applied per group of like artifacts (one volume's tarballs,
//! one config file's copies): the newest `keep_min` are always kept,
//! whatever their age, and of the rest only those older than
//! `retention_days` go. Before, retention was age-only with no floor, so a
//! streak of failed nights longer than `retention_days` deleted every good
//! backup; and S3 was never pruned. Anything whose time can't be determined
//! is never deleted.

use std::collections::BTreeMap;

use chrono::{NaiveDate, NaiveDateTime};

/// From `(item, unix time)` pairs, the items to delete.
pub fn to_prune<T: Clone>(
    items: &[(T, u64)],
    now: u64,
    retention_days: u32,
    keep_min: usize,
) -> Vec<T> {
    let cutoff = now.saturating_sub(u64::from(retention_days) * 86_400);
    let mut sorted: Vec<&(T, u64)> = items.iter().collect();
    sorted.sort_by_key(|(_, t)| std::cmp::Reverse(*t)); // newest first
    sorted
        .into_iter()
        .skip(keep_min)
        .filter(|(_, t)| *t < cutoff)
        .map(|(item, _)| item.clone())
        .collect()
}

/// Where an S3 key belongs, and when it was written, from the layout orca
/// uploads with:
/// - `master/<date>/<name>_<YYYYmmddTHHMMSSZ>.<ext>[.age]` → group `master/<name>`
/// - `agents/<host>/<date>/<file>[.age]` → group `agents/<host>/<file>` (dated
///   by `<date>`). A volume's encrypted and plaintext tarballs share a group
///   (#231), so once encryption is on, the old plaintext copies age out
///   instead of `keep_min` keeping the last few forever.
///
/// Returns `None` for anything else, which is then never pruned.
pub fn s3_group(key: &str) -> Option<(String, u64)> {
    let parts: Vec<&str> = key.split('/').collect();
    match parts.as_slice() {
        ["master", _date, file] => {
            let (name, t) = artifact_time(file)?;
            Some((format!("master/{name}"), t))
        }
        ["agents", host, date, file] => {
            let d = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
            let t = d.and_hms_opt(0, 0, 0)?.and_utc().timestamp();
            let file = file
                .strip_suffix(super::encrypt::AGE_SUFFIX)
                .unwrap_or(file);
            Some((format!("agents/{host}/{file}"), t.try_into().ok()?))
        }
        _ => None,
    }
}

/// `<name>_<YYYYmmddTHHMMSSZ>.<ext>[.age]` → (`name`, unix time): the config
/// artifacts `BackupManager::backup_file` writes. `None` for anything else.
pub fn artifact_time(file_name: &str) -> Option<(String, u64)> {
    let (name, rest) = file_name.split_once('_')?;
    let ts = rest.split('.').next()?;
    let t = NaiveDateTime::parse_from_str(ts, "%Y%m%dT%H%M%SZ").ok()?;
    Some((name.to_string(), t.and_utc().timestamp().try_into().ok()?))
}

/// The S3 keys to delete, applying [`to_prune`] per [`s3_group`].
pub fn s3_prune_plan(
    keys: &[String],
    now: u64,
    retention_days: u32,
    keep_min: usize,
) -> Vec<String> {
    let mut groups: BTreeMap<String, Vec<(String, u64)>> = BTreeMap::new();
    for key in keys {
        if let Some((group, t)) = s3_group(key) {
            groups.entry(group).or_default().push((key.clone(), t));
        }
    }
    groups
        .values()
        .flat_map(|items| to_prune(items, now, retention_days, keep_min))
        .collect()
}

#[cfg(test)]
#[path = "retention_tests.rs"]
mod tests;
