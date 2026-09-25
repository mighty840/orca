//! Resolving `${secrets.X}` references in `cluster.toml` fields.
//!
//! An unresolved reference must never become a credential (#226). A
//! `[[token]]` or `api_tokens` value that doesn't resolve used to be loaded
//! as the literal `${secrets.X}`, a valid admin token for anyone who can read
//! `cluster.toml`. Such tokens are now dropped with an error. Other fields
//! (AI key, SMTP password, S3 keys, setup key) keep their value and log a
//! warning: they don't grant access, and they fail visibly where they're used.

use std::collections::HashMap;

use tracing::{error, warn};

use super::{BackupTarget, ClusterConfig};
use crate::secrets::SecretStore;

const PREFIX: &str = "${secrets.";

/// Resolve every `${secrets.X}` in `value`, or explain why it can't be.
fn resolve(store: &Result<SecretStore, String>, value: &str) -> Result<String, String> {
    if !value.contains(PREFIX) {
        return Ok(value.to_string());
    }
    let store = store
        .as_ref()
        .map_err(|e| format!("secrets store unavailable: {e}"))?;
    let map = HashMap::from([(String::new(), value.to_string())]);
    let mut resolved = store
        .resolve_env_checked(&map, None)
        .map_err(|e| e.to_string())?;
    Ok(resolved.remove("").unwrap_or_default())
}

/// A field that is not a credential: resolve it, or keep it and warn.
fn resolve_or_warn(store: &Result<SecretStore, String>, field: &str, value: &mut String) {
    match resolve(store, value) {
        Ok(v) => *value = v,
        Err(e) => warn!("cluster.toml {field}: {e}; left unresolved"),
    }
}

fn resolve_opt(store: &Result<SecretStore, String>, field: &str, value: &mut Option<String>) {
    if let Some(v) = value {
        resolve_or_warn(store, field, v);
    }
}

/// A token: keep it only if it resolves to a non-empty value.
fn resolve_token(store: &Result<SecretStore, String>, what: &str, value: &mut String) -> bool {
    match resolve(store, value) {
        Ok(v) if !v.is_empty() => {
            *value = v;
            true
        }
        Ok(_) => {
            error!("cluster.toml {what}: empty value; token disabled");
            false
        }
        Err(e) => {
            error!("cluster.toml {what}: {e}; token disabled");
            false
        }
    }
}

impl ClusterConfig {
    /// Resolve `${secrets.X}` in the fields that may hold secrets.
    pub(super) fn resolve_secrets(&mut self) {
        let store = crate::secrets::open_configured().map_err(|e| e.to_string());

        // Tokens first: these are credentials, so they fail closed.
        self.token.retain_mut(|t| {
            let what = format!("[[token]] {:?}", t.name);
            resolve_token(&store, &what, &mut t.value)
        });
        self.api_tokens
            .retain_mut(|t| resolve_token(&store, "api_tokens entry", t));

        if let Some(ai) = &mut self.ai {
            resolve_opt(&store, "ai.api_key", &mut ai.api_key);
            resolve_opt(&store, "ai.endpoint", &mut ai.endpoint);
            if let Some(alerts) = &mut ai.alerts
                && let Some(channels) = &mut alerts.channels
                && let Some(email) = &mut channels.email
            {
                resolve_or_warn(&store, "ai.alerts email password", &mut email.password);
            }
        }
        if let Some(net) = &mut self.network {
            resolve_opt(&store, "network.setup_key", &mut net.setup_key);
        }
        if let Some(backup) = &mut self.backup {
            for target in &mut backup.targets {
                if let BackupTarget::S3 {
                    access_key,
                    secret_key,
                    ..
                } = target
                {
                    resolve_opt(&store, "backup S3 access_key", access_key);
                    resolve_opt(&store, "backup S3 secret_key", secret_key);
                }
            }
        }
    }
}
