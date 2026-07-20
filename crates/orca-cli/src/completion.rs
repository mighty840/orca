//! Dynamic shell completion for orca internals (service names, secret keys,
//! webhook service names, alert ids) via clap_complete's `unstable-dynamic`
//! engine. Enable in a shell with: `source <(COMPLETE=bash orca)`.
//!
//! Completer closures are synchronous but need to query the master, so each
//! runs a one-shot fetch on a fresh thread with its own current-thread
//! runtime — safe whether or not a tokio runtime is already active, and any
//! failure (master down, no token) yields no candidates rather than an error.

use clap_complete::engine::CompletionCandidate;

use crate::client::OrcaClient;

fn default_client() -> OrcaClient {
    OrcaClient::new("http://127.0.0.1:6880".to_string())
}

/// Run `fut` to completion on an isolated thread+runtime; `None` on any error.
fn fetch<T, F>(fut: F) -> Option<T>
where
    T: Send + 'static,
    F: std::future::Future<Output = anyhow::Result<T>> + Send + 'static,
{
    std::thread::spawn(move || {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .ok()?
            .block_on(fut)
            .ok()
    })
    .join()
    .ok()
    .flatten()
}

fn candidates(values: Vec<String>) -> Vec<CompletionCandidate> {
    values.into_iter().map(CompletionCandidate::new).collect()
}

pub fn services() -> Vec<CompletionCandidate> {
    let names = fetch(async {
        let c = default_client();
        c.status().await.map(|s| {
            s.services
                .into_iter()
                .map(|svc| svc.name)
                .collect::<Vec<_>>()
        })
    })
    .unwrap_or_default();
    candidates(names)
}

pub fn secret_keys() -> Vec<CompletionCandidate> {
    // Secret keys come from the same store the master serves; `orca secrets`
    // reads it locally, so use the store directly rather than the API.
    let keys = orca_core::secrets::open_configured()
        .map(|s| s.list())
        .unwrap_or_default();
    candidates(keys)
}

pub fn webhook_services() -> Vec<CompletionCandidate> {
    let names = fetch(async {
        let c = default_client();
        c.list_webhooks().await.map(|v| {
            v["webhooks"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|h| h["service_name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        })
    })
    .unwrap_or_default();
    candidates(names)
}

pub fn alert_ids() -> Vec<CompletionCandidate> {
    let ids = fetch(async {
        let c = default_client();
        c.alerts_list(true)
            .await
            .map(|a| a.into_iter().map(|x| x.id.to_string()).collect::<Vec<_>>())
    })
    .unwrap_or_default();
    candidates(ids)
}
