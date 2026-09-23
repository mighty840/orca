//! Durable webhook registry: `~/.orca/webhooks.json` (#183).

use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::error;

use crate::webhook::WebhookConfig;

/// Shared webhook config store, stored in [`AppState`] extension.
pub type WebhookStore = Arc<RwLock<Vec<WebhookConfig>>>;

/// Path to the on-disk webhook config file (under `~/.orca`).
///
/// Honors `ORCA_WEBHOOKS_PATH` as an override for tests.
fn webhooks_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("ORCA_WEBHOOKS_PATH") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    std::path::PathBuf::from(home).join(".orca/webhooks.json")
}

/// Load persisted webhooks from disk, returning an empty list on first run.
pub fn new_store() -> WebhookStore {
    let configs = load_from(&webhooks_path());
    for wh in configs.iter().filter(|w| w.effective_secret().is_none()) {
        error!(
            repo = %wh.repo,
            branch = %wh.branch,
            service = %wh.service_name,
            "Webhook has no secret and will reject every push. \
             Re-register it with `orca webhooks add`."
        );
    }
    Arc::new(RwLock::new(configs))
}

/// Read the registry at `path` (#183). A missing file is a first run, and
/// empty is correct. A file that exists but can't be read or parsed is not
/// empty: it's moved aside to `<path>.corrupt-<unix time>` and the error
/// says where. Otherwise the next `orca webhooks add` would persist the
/// empty list over it and silently drop every other registration.
pub(crate) fn load_from(path: &std::path::Path) -> Vec<WebhookConfig> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            error!(
                "Cannot read webhook registry {}: {e}. Starting with no webhooks; \
                 pushes will be ignored until it is readable again.",
                path.display()
            );
            return Vec::new();
        }
    };
    match serde_json::from_str(&raw) {
        Ok(configs) => configs,
        Err(e) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or_default();
            let aside = std::path::PathBuf::from(format!("{}.corrupt-{stamp}", path.display()));
            let kept = match std::fs::rename(path, &aside) {
                Ok(()) => format!("moved to {}", aside.display()),
                Err(re) => format!("could not be moved aside ({re})"),
            };
            error!(
                "Webhook registry {} is unparseable ({e}); it was {kept}. Starting with \
                 no webhooks: pushes are ignored until they are re-registered or the \
                 file is repaired and restored.",
                path.display()
            );
            Vec::new()
        }
    }
}

/// Persist the current webhook list to disk, atomically and owner-only (it
/// holds the HMAC secrets). Errors are logged, not returned, so they don't
/// fail the request that triggered the change.
pub(crate) async fn persist(store: &WebhookStore) {
    let snapshot = store.read().await.clone();
    let path = webhooks_path();
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => {
            if let Err(e) = orca_core::fsutil::write_private(&path, json.as_bytes()) {
                error!("Failed to persist webhooks to {}: {e}", path.display());
            }
        }
        Err(e) => error!("Failed to serialize webhooks: {e}"),
    }
}

#[cfg(test)]
#[path = "webhook_store_tests.rs"]
mod tests;
