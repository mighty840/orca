//! `orca secrets` — CLI surface over the configured secret store
//! (legacy machine-local AES file, or the [secrets] SOPS/age-encrypted
//! file when cluster.toml configures one — #109).

use crate::commands::SecretsAction;

fn open_secrets() -> orca_core::secrets::SecretStore {
    orca_core::secrets::open_configured().unwrap_or_else(|e| {
        tracing::error!("Failed to open secrets store: {e}");
        std::process::exit(1);
    })
}

pub fn handle_secrets(action: SecretsAction) {
    // Best-effort cluster.toml load so a configured [secrets] section
    // switches this CLI to the encrypted backend (#109). Without one the
    // legacy machine-local store is used — which doubles as the offline
    // recovery path when run next to the config repo.
    let cluster_toml = std::path::Path::new("cluster.toml");
    if cluster_toml.exists() {
        let _ = orca_core::config::ClusterConfig::load(cluster_toml);
    }
    match action {
        SecretsAction::Set { key, value } => {
            let mut store = open_secrets();
            store.set(&key, &value).expect("failed to set secret");
            println!("Secret '{key}' set.");
        }
        SecretsAction::Get { key } => {
            let store = open_secrets();
            match store.get(&key) {
                Some(value) => println!("{value}"),
                None => {
                    eprintln!("Secret '{key}' not found.");
                    std::process::exit(1);
                }
            }
        }
        SecretsAction::Remove { key } => {
            let mut store = open_secrets();
            match store.remove(&key) {
                Ok(true) => println!("Secret '{key}' removed."),
                Ok(false) => println!("Secret '{key}' not found."),
                Err(e) => tracing::error!("Failed to remove: {e}"),
            }
        }
        SecretsAction::List => {
            let store = open_secrets();
            let keys = store.list();
            if keys.is_empty() {
                println!("No secrets configured.");
            } else {
                for key in keys {
                    println!("  {key}");
                }
            }
        }
        SecretsAction::Import { file } => {
            let mut store = open_secrets();
            let content = std::fs::read_to_string(&file).unwrap_or_else(|e| {
                tracing::error!("Failed to read '{file}': {e}");
                std::process::exit(1);
            });
            let mut count = 0u32;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    store.set(key.trim(), value.trim()).expect("failed to set");
                    count += 1;
                }
            }
            println!("Imported {count} secrets from {file}.");
        }
        SecretsAction::Migrate => {
            if !orca_core::secrets::sops_configured() {
                eprintln!(
                    "No [secrets] section configured — add encrypted_file/age_recipients \
                     to cluster.toml (run from the directory containing it) first."
                );
                std::process::exit(1);
            }
            let legacy_path = orca_core::secrets::default_path();
            let legacy = orca_core::secrets::SecretStore::open(&legacy_path).unwrap_or_else(|e| {
                tracing::error!("Failed to open legacy store: {e}");
                std::process::exit(1);
            });
            let mut store = open_secrets();
            let count = store.import_from(&legacy).unwrap_or_else(|e| {
                tracing::error!("Migration failed: {e}");
                std::process::exit(1);
            });
            let backup = legacy_path.with_extension("json.bak");
            match std::fs::rename(&legacy_path, &backup) {
                Ok(()) => println!(
                    "Migrated {count} secret(s) into the encrypted store. \
                     Legacy store moved to {}.",
                    backup.display()
                ),
                Err(e) => println!(
                    "Migrated {count} secret(s), but could not move the legacy store aside: {e}. \
                     Remove {} manually.",
                    legacy_path.display()
                ),
            }
        }
    }
}
