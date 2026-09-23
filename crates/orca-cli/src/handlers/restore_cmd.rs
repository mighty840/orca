//! `orca backup restore …` commands (#200). Each returns `false` on failure
//! so the process exits non-zero.

use std::path::{Path, PathBuf};

use orca_core::backup::BackupConfig;

use super::restore::{
    Destination, Found, RESTORE_ORDER, collect, destination, extract_into, fetch, install_file,
    latest_per_name, plaintext, read_identity, server_running,
};

const API_PORT: u16 = 6880;

/// A fresh `~/.orca/restore/<unix time>/` for downloads and decryption.
fn staging_dir(home: &Path) -> anyhow::Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let dir = home.join(".orca/restore").join(stamp.to_string());
    orca_core::fsutil::create_private_dir(&dir)?;
    Ok(dir)
}

/// Fetch, decrypt and install one config artifact. Returns a one-line result.
fn restore_one(
    found: &Found,
    home: &Path,
    staging: &Path,
    identity: Option<&str>,
) -> anyhow::Result<String> {
    let name = &found.parsed.name;
    let dest = destination(name, home)
        .ok_or_else(|| anyhow::anyhow!("don't know where '{name}' is restored to"))?;
    let path = fetch(found, staging)?;
    let bytes = plaintext(&path, found.parsed.encrypted, identity)?;
    Ok(match dest {
        Destination::File(d) => match install_file(&d, &bytes)? {
            Some(old) => format!("{} (previous kept at {})", d.display(), old.display()),
            None => d.display().to_string(),
        },
        Destination::ExtractInto(dir) => {
            let aside = extract_into(&dir, &bytes, staging)?;
            format!(
                "extracted into {} ({} existing entr(y/ies) kept aside)",
                dir.display(),
                aside.len()
            )
        }
    })
}

fn load_identity(identity: Option<&Path>) -> Result<Option<String>, ()> {
    match identity.map(read_identity).transpose() {
        Ok(id) => Ok(id),
        Err(e) => {
            eprintln!("{e:#}");
            Err(())
        }
    }
}

/// Restore the newest backup of every config artifact, key first.
pub(crate) fn restore_basic(config: &BackupConfig, identity: Option<&Path>, force: bool) -> bool {
    let Some(home) = dirs_next::home_dir() else {
        eprintln!("Cannot determine the home directory.");
        return false;
    };
    if !force && server_running(API_PORT) {
        eprintln!(
            "An orca server is answering on port {API_PORT}. Stop it first \
             (`sudo systemctl stop orca`) or pass --force: it would overwrite \
             restored state, and cluster.db must not change under it."
        );
        return false;
    }
    let Ok(identity) = load_identity(identity) else {
        return false;
    };
    let staging = match staging_dir(&home) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("Cannot create a staging directory: {e:#}");
            return false;
        }
    };

    let latest = latest_per_name(collect(config));
    let (mut restored, mut failed) = (0u32, 0u32);
    for name in RESTORE_ORDER {
        let Some(found) = latest.get(*name) else {
            println!("  {name}: no backup found");
            continue;
        };
        match restore_one(found, &home, &staging, identity.as_deref()) {
            Ok(msg) => {
                println!("  {name} ({}): {msg}", found.parsed.timestamp);
                restored += 1;
            }
            Err(e) => {
                eprintln!("  {name}: FAILED: {e:#}");
                failed += 1;
            }
        }
    }
    println!(
        "Restore: {restored} restored, {failed} failed. Downloads kept in {}.",
        staging.display()
    );
    failed == 0 && restored > 0
}

/// Restore the one config artifact whose name or key contains `id`.
pub(crate) fn restore_by_id(config: &BackupConfig, id: &str, identity: Option<&Path>) -> bool {
    let Some(home) = dirs_next::home_dir() else {
        return false;
    };
    let Ok(identity) = load_identity(identity) else {
        return false;
    };
    let matches: Vec<Found> = collect(config)
        .into_iter()
        .filter(|f| f.location.contains(id) && destination(&f.parsed.name, &home).is_some())
        .collect();
    match matches.as_slice() {
        [] => {
            eprintln!(
                "No config backup matches '{id}'. `orca backup list` shows what exists; \
                 volume tarballs are restored with `orca backup restore-volume`."
            );
            false
        }
        [found] => {
            let Ok(staging) = staging_dir(&home) else {
                return false;
            };
            match restore_one(found, &home, &staging, identity.as_deref()) {
                Ok(msg) => {
                    println!("Restored {} → {msg}", found.location);
                    true
                }
                Err(e) => {
                    eprintln!("Restore of {} failed: {e:#}", found.location);
                    false
                }
            }
        }
        many => {
            eprintln!("'{id}' matches {} backups; be more specific:", many.len());
            for f in many {
                eprintln!("  {}", f.location);
            }
            false
        }
    }
}

// Tested end to end through the binary in tests/restore_e2e_test.rs: these
// functions read $HOME, which tests must not change in-process.
