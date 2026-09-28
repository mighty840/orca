//! `:webhook-add`, `:webhook-edit` and `:webhook-rm`.

use crate::api::ApiClient;
use crate::state::AppState;

/// `:webhook-add <repo> <branch> <service> [--secret X] [--infra]` — register
/// a new webhook. Pre-filled from the `a` keybind on the Webhooks view; can
/// also be typed manually.
pub(crate) async fn cmd_webhook_add(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 4 {
        state.flash("Usage: :webhook-add <repo> <branch> <service> [--secret X] [--infra]".into());
        return;
    }
    let body = build_webhook_body(parts[1], parts[2], parts[3], &parts[4..]);
    match client.add_webhook(body).await {
        Ok(()) => {
            state.flash(format!("Registered webhook for {}", parts[3]));
            crate::refresh_webhooks(client, state).await;
        }
        Err(e) => state.error = Some(format!("Add failed: {e}")),
    }
}

/// `:webhook-edit <repo> <branch> <service> [--secret X] [--infra]` — re-runs
/// `:webhook-add` which dedupes by (repo, branch, service) and replaces the
/// matching entry. The TUI's `e` keybind pre-fills the identity fields so
/// the user only types the new optional flags.
pub(crate) async fn cmd_webhook_edit(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 4 {
        state.flash("Usage: :webhook-edit <repo> <branch> <service> [--secret X] [--infra]".into());
        return;
    }
    let body = build_webhook_body(parts[1], parts[2], parts[3], &parts[4..]);
    match client.add_webhook(body).await {
        Ok(()) => {
            state.flash(format!("Updated webhook for {}", parts[3]));
            crate::refresh_webhooks(client, state).await;
        }
        Err(e) => state.error = Some(format!("Edit failed: {e}")),
    }
}

pub(crate) async fn cmd_webhook_rm(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :webhook-rm <service>".into());
        return;
    }
    let service = parts[1];
    match client.remove_webhook(service).await {
        Ok(()) => {
            state.flash(format!("Removed webhook for {service}"));
            crate::refresh_webhooks(client, state).await;
        }
        Err(e) => state.error = Some(format!("Remove failed: {e}")),
    }
}

/// Build the `WebhookConfig` JSON body the server expects from positional +
/// flag CLI arguments. Centralized so `add` and `edit` share the same parser.
pub(crate) fn build_webhook_body(
    repo: &str,
    branch: &str,
    service: &str,
    flags: &[&str],
) -> serde_json::Value {
    let mut infra = false;
    let mut secret: Option<String> = None;
    let mut i = 0;
    while i < flags.len() {
        match flags[i] {
            "--infra" => {
                infra = true;
                i += 1;
            }
            "--secret" => {
                if i + 1 < flags.len() {
                    secret = Some(flags[i + 1].to_string());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    let mut body = serde_json::json!({
        "repo": repo,
        "branch": branch,
        "service_name": service,
        "infra": infra,
    });
    if let Some(s) = secret {
        body["secret"] = serde_json::Value::String(s);
    }
    body
}
