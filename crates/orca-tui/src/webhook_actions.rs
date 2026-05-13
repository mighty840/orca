//! TUI controller helpers for the webhooks view. Each function fetches from
//! the API and writes to `AppState`, keeping the event-loop / lib.rs slim.

use crate::api::ApiClient;
use crate::state::AppState;

/// Fetch the current webhook list and cache it on state. Called when entering
/// the webhooks view, on `r`, and after add/edit/delete actions so the
/// dashboard always reflects the persisted state on disk.
pub(crate) async fn refresh_webhooks(client: &ApiClient, state: &mut AppState) {
    match client.list_webhooks().await {
        Ok(resp) => {
            if state.selected_webhook >= resp.webhooks.len() {
                state.selected_webhook = resp.webhooks.len().saturating_sub(1);
            }
            state.webhooks = resp.webhooks;
        }
        Err(e) => state.error = Some(format!("Webhook list failed: {e}")),
    }
}

/// Fetch the invocation history for one webhook (by service name).
pub(crate) async fn refresh_webhook_invocations(
    client: &ApiClient,
    state: &mut AppState,
    service: &str,
) {
    match client.webhook_invocations(service).await {
        Ok(resp) => state.webhook_invocations = resp.invocations,
        Err(e) => state.error = Some(format!("Invocation history failed: {e}")),
    }
}

/// Delete the webhook on the currently-selected row. No interactive confirm
/// dialog — this matches `x` on the services view (immediate stop). If we
/// later add an `Are you sure?` flow it should apply to both.
pub(crate) async fn delete_selected_webhook(client: &ApiClient, state: &mut AppState) {
    let Some(w) = state.webhooks.get(state.selected_webhook) else {
        return;
    };
    let service = w.service_name.clone();
    match client.remove_webhook(&service).await {
        Ok(()) => {
            state.flash(format!("Removed webhook for {service}"));
            refresh_webhooks(client, state).await;
        }
        Err(e) => state.error = Some(format!("Delete failed: {e}")),
    }
}
