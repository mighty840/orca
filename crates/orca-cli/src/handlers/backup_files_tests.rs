//! #199: the config backup covers everything a restore needs, and never
//! stores key material in the clear.

use std::path::Path;

use orca_core::backup::{BackupConfig, BackupManager, BackupTarget};

use super::*;

/// A fake `$HOME` with every file the backup looks for.
fn home_with_state() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    let orca = home.path().join(".orca");
    std::fs::create_dir_all(orca.join("certs/example.com")).unwrap();
    for (name, body) in [
        ("master.key", "k".repeat(32)),
        ("secrets.json", r#"{"secrets":{}}"#.into()),
        ("cluster.toml", "[[token]]\nvalue = \"tok\"\n".into()),
        ("cluster.db", "db".into()),
        ("webhooks.json", "[]".into()),
        ("backup_config.json", "{}".into()),
        ("acme-account.json", "{}".into()),
    ] {
        std::fs::write(orca.join(name), body).unwrap();
    }
    std::fs::write(orca.join("certs/example.com/key.pem"), "PRIVATE").unwrap();
    home
}

fn manager(dest: &Path, recipients: Vec<String>) -> BackupManager {
    BackupManager::new(BackupConfig {
        age_recipients: recipients,
        schedule: None,
        retention_days: 30,
        targets: vec![BackupTarget::Local {
            path: dest.display().to_string(),
        }],
    })
}

fn stored_names(dest: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dest)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn restore_order_puts_the_master_key_first() {
    let names: Vec<_> = artifacts(Some(Path::new("/h")))
        .iter()
        .map(|a| a.name)
        .collect();
    assert_eq!(names[0], "master-key");
    assert!(names.contains(&"webhooks"));
    assert!(names.contains(&"certs"));
    assert!(names.contains(&"acme-account"));
}

#[test]
fn without_encryption_key_material_is_skipped_not_stored_in_the_clear() {
    let home = home_with_state();
    let dest = tempfile::tempdir().unwrap();
    let out = run(&manager(dest.path(), vec![]), Some(home.path()), "");

    let names = stored_names(dest.path());
    for plain in ["secrets_", "cluster_", "cluster-db_"] {
        assert!(
            names.iter().any(|n| n.starts_with(plain)),
            "{plain} missing: {names:?}"
        );
    }
    for secret in [
        "master-key_",
        "webhooks_",
        "backup-config_",
        "acme-account_",
        "certs_",
    ] {
        assert!(
            !names.iter().any(|n| n.starts_with(secret)),
            "{secret} must not be stored unencrypted: {names:?}"
        );
    }
    assert_eq!(out.skipped_unencrypted.len(), 5, "{out:?}");
    assert_eq!(
        out.stored_plain_with_credentials.len(),
        1,
        "cluster.toml warns"
    );
    assert!(out.failed.is_empty());
}

#[test]
fn with_encryption_everything_is_stored_encrypted_and_decrypts() {
    let home = home_with_state();
    let dest = tempfile::tempdir().unwrap();
    let id = age::x25519::Identity::generate();
    let private = age::secrecy::ExposeSecret::expose_secret(&id.to_string()).to_string();

    let out = run(
        &manager(dest.path(), vec![id.to_public().to_string()]),
        Some(home.path()),
        "",
    );
    assert!(out.failed.is_empty(), "{out:?}");
    assert!(out.skipped_unencrypted.is_empty());
    assert!(out.stored_plain_with_credentials.is_empty());

    let names = stored_names(dest.path());
    assert_eq!(names.len(), 8, "{names:?}");
    assert!(names.iter().all(|n| n.ends_with(".age")), "{names:?}");

    let key = names.iter().find(|n| n.starts_with("master-key_")).unwrap();
    let plain = orca_core::backup::encrypt::decrypt_file(&dest.path().join(key), &private).unwrap();
    assert_eq!(plain, "k".repeat(32).as_bytes());

    // certs/ is one tarball containing the private key, and it's encrypted.
    let certs = names.iter().find(|n| n.starts_with("certs_")).unwrap();
    assert!(certs.ends_with(".gz.age"), "{certs}");
    let raw = std::fs::read(dest.path().join(certs)).unwrap();
    assert!(!raw.windows(7).any(|w| w == b"PRIVATE"));
}
