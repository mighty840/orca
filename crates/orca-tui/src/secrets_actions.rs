//! Inline actions for the secrets organizer (#69): delete with y/N
//! confirmation and the `p` scope-filter cycle. The add/edit flows reuse the
//! command bar (`:set`) — see the `a`/`e` arms in `keys.rs`.

use crate::api::ApiClient;
use crate::state::AppState;

/// Confirmed delete: remove the key on the master, refresh, and clamp the
/// selection back onto a selectable row (mirrors `:rm`).
pub(crate) async fn delete_secret(client: &ApiClient, state: &mut AppState, key: &str) {
    match client.remove_secret(key).await {
        Ok(()) => {
            state.flash(format!("Secret {key} removed"));
            crate::refresh_secrets_usage(client, state).await;
            clamp_secret_selection(state);
        }
        Err(e) => state.error = Some(format!("Remove secret failed: {e}")),
    }
}

/// Cycle the scope filter: all groups → each group label in display order →
/// all. Selection resets to the first selectable row so the cursor never
/// points at a row the filter just hid.
pub(crate) fn cycle_scope_filter(state: &mut AppState) {
    let labels = crate::ui::secrets::group_labels(&state.secrets_usage);
    if labels.is_empty() {
        return;
    }
    state.secrets_scope_filter = match &state.secrets_scope_filter {
        None => Some(labels[0].clone()),
        Some(current) => labels
            .iter()
            .position(|l| l == current)
            .and_then(|i| labels.get(i + 1))
            .cloned(),
    };
    let shown = state
        .secrets_scope_filter
        .as_deref()
        .unwrap_or("all scopes");
    state.flash(format!("Secrets filter: {shown}"));
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    state.selected_secret = crate::ui::secrets::selectable_indices(&rows)
        .first()
        .copied()
        .unwrap_or(0);
}

/// Clamp `selected_secret` into the current selectable set.
pub(crate) fn clamp_secret_selection(state: &mut AppState) {
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    let sel = crate::ui::secrets::selectable_indices(&rows);
    if !sel.contains(&state.selected_secret) {
        state.selected_secret = sel.last().copied().unwrap_or(0);
    }
}

/// j/k/g/G navigation over the flattened secrets list — skips group
/// headers, honors the active scope filter.
pub(crate) fn secret_nav_first(state: &mut AppState) {
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    if let Some(&i) = crate::ui::secrets::selectable_indices(&rows).first() {
        state.selected_secret = i;
    }
}

pub(crate) fn secret_nav_last(state: &mut AppState) {
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    if let Some(&i) = crate::ui::secrets::selectable_indices(&rows).last() {
        state.selected_secret = i;
    }
}

pub(crate) fn secret_nav_next(state: &mut AppState) {
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    let sel = crate::ui::secrets::selectable_indices(&rows);
    if let Some(next) = sel.iter().find(|&&i| i > state.selected_secret) {
        state.selected_secret = *next;
    }
}

pub(crate) fn secret_nav_prev(state: &mut AppState) {
    let rows =
        crate::ui::secrets::flatten(&state.secrets_usage, state.secrets_scope_filter.as_deref());
    let sel = crate::ui::secrets::selectable_indices(&rows);
    if let Some(prev) = sel.iter().rev().find(|&&i| i < state.selected_secret) {
        state.selected_secret = *prev;
    }
}
