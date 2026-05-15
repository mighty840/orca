//! Key event handlers — filter, command, and normal mode input.

use crossterm::event::KeyCode;

use crate::api::ApiClient;
use crate::state::{AppState, InputMode, View};

pub fn handle_filter_key(state: &mut AppState, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            state.filter.clear();
            state.input_mode = InputMode::Normal;
            state.selected_service = 0;
        }
        KeyCode::Enter => {
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Backspace => {
            state.filter.pop();
            state.selected_service = 0;
        }
        KeyCode::Char(c) => {
            state.filter.push(c);
            state.selected_service = 0;
        }
        _ => {}
    }
}

pub async fn handle_command_key(state: &mut AppState, client: &ApiClient, code: KeyCode) {
    match code {
        KeyCode::Esc => {
            state.command_input.clear();
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Enter => {
            let cmd = state.command_input.trim().to_string();
            state.command_input.clear();
            state.input_mode = InputMode::Normal;
            crate::commands::execute_command(state, client, &cmd).await;
        }
        KeyCode::Backspace => {
            state.command_input.pop();
            if state.command_input.is_empty() {
                state.input_mode = InputMode::Normal;
            }
        }
        KeyCode::Char(c) => {
            state.command_input.push(c);
        }
        _ => {}
    }
}

pub async fn handle_normal_key(
    state: &mut AppState,
    client: &ApiClient,
    code: KeyCode,
    last_refresh: &mut tokio::time::Instant,
) {
    if matches!(state.view, View::Chat) {
        crate::chat_input::handle_chat_key(state, client, code).await;
        return;
    }
    match code {
        KeyCode::Char('q') => state.should_quit = true,
        KeyCode::Char(':') => {
            state.input_mode = InputMode::Command;
            state.command_input.clear();
        }
        KeyCode::Char('/') => {
            if matches!(state.view, View::Services) {
                state.input_mode = InputMode::Filter;
            }
        }
        KeyCode::Char('?') => state.push_view(View::Help),
        KeyCode::Esc => handle_esc(state),

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => match state.view {
            View::Secrets => secret_nav_next(state),
            View::Backups => {
                let len = state.backups.as_ref().map(|b| b.nodes.len()).unwrap_or(0);
                if len > 0 && state.selected_backup_node + 1 < len {
                    state.selected_backup_node += 1;
                }
            }
            View::BackupSnapshots { node_idx } => {
                let len = snapshot_count(state, node_idx);
                if len > 0 && state.selected_backup_snapshot + 1 < len {
                    state.selected_backup_snapshot += 1;
                }
            }
            View::Webhooks => {
                if !state.webhooks.is_empty() && state.selected_webhook + 1 < state.webhooks.len() {
                    state.selected_webhook += 1;
                }
            }
            View::Networks => state.network_scroll = state.network_scroll.saturating_add(1),
            View::Alerts => {
                if !state.alerts.is_empty() && state.selected_alert + 1 < state.alerts.len() {
                    state.selected_alert += 1;
                }
            }
            View::AlertDetail { .. } => {
                state.alert_detail_scroll = state.alert_detail_scroll.saturating_add(1);
            }
            _ => state.next_service(),
        },
        KeyCode::Char('k') | KeyCode::Up => match state.view {
            View::Secrets => secret_nav_prev(state),
            View::Backups => {
                if state.selected_backup_node > 0 {
                    state.selected_backup_node -= 1;
                }
            }
            View::BackupSnapshots { .. } => {
                if state.selected_backup_snapshot > 0 {
                    state.selected_backup_snapshot -= 1;
                }
            }
            View::Webhooks => {
                if state.selected_webhook > 0 {
                    state.selected_webhook -= 1;
                }
            }
            View::Networks => state.network_scroll = state.network_scroll.saturating_sub(1),
            View::Alerts => {
                if state.selected_alert > 0 {
                    state.selected_alert -= 1;
                }
            }
            View::AlertDetail { .. } => {
                state.alert_detail_scroll = state.alert_detail_scroll.saturating_sub(1);
            }
            _ => state.prev_service(),
        },
        KeyCode::Char('g') => match state.view {
            View::Secrets => secret_nav_first(state),
            View::Backups => state.selected_backup_node = 0,
            View::BackupSnapshots { .. } => state.selected_backup_snapshot = 0,
            View::Webhooks => state.selected_webhook = 0,
            View::Networks => state.network_scroll = 0,
            View::Alerts => state.selected_alert = 0,
            View::AlertDetail { .. } => state.alert_detail_scroll = 0,
            _ => state.selected_service = 0,
        },
        KeyCode::Char('G') => match state.view {
            View::Secrets => secret_nav_last(state),
            View::Backups => {
                let len = state.backups.as_ref().map(|b| b.nodes.len()).unwrap_or(0);
                if len > 0 {
                    state.selected_backup_node = len - 1;
                }
            }
            View::BackupSnapshots { node_idx } => {
                let len = snapshot_count(state, node_idx);
                if len > 0 {
                    state.selected_backup_snapshot = len - 1;
                }
            }
            View::Webhooks => {
                if !state.webhooks.is_empty() {
                    state.selected_webhook = state.webhooks.len() - 1;
                }
            }
            View::Networks => {
                // Snap to last line; render clamps to the visible window.
                let total = super::ui::networks::rendered_line_count(state);
                state.network_scroll = total.saturating_sub(1);
            }
            View::Alerts => {
                if !state.alerts.is_empty() {
                    state.selected_alert = state.alerts.len() - 1;
                }
            }
            View::AlertDetail { .. } => {
                state.alert_detail_scroll = state.alert_detail_scroll.saturating_add(100);
            }
            _ => {
                let len = state.filtered_services().len();
                if len > 0 {
                    state.selected_service = len - 1;
                }
            }
        },
        // Collapse / expand the project group containing the selected
        // service. Bound to `c` because space otherwise conflicts with
        // list scrolling semantics some users expect.
        KeyCode::Char('c') => {
            if matches!(state.view, View::Services)
                && let Some(svc) = state.selected_service_data()
                && let Some(proj) = svc.project.clone()
            {
                state.toggle_collapse_project(&proj);
            }
        }

        // Enter detail view
        KeyCode::Enter => handle_enter(state, client).await,

        // Full-screen logs
        KeyCode::Char('l') => handle_logs(state, client).await,

        // Refresh — when in the backups view, refetch the backup status too
        // (we don't auto-refresh it every 2s since it's an expensive
        // fan-out RPC and the data changes infrequently).
        KeyCode::Char('r') => {
            super::refresh(client, state).await;
            match &state.view {
                View::Backups => super::refresh_backups(client, state).await,
                View::Webhooks => super::refresh_webhooks(client, state).await,
                View::WebhookInvocations { service } => {
                    let s = service.clone();
                    super::refresh_webhook_invocations(client, state, &s).await;
                }
                View::Secrets | View::SecretRefs { .. } => {
                    super::refresh_secrets_usage(client, state).await
                }
                View::Networks => super::refresh_networks(client, state).await,
                View::Alerts | View::AlertDetail { .. } => {
                    super::refresh_alerts(client, state).await;
                }
                _ => {}
            }
            *last_refresh = tokio::time::Instant::now();
            state.flash("Refreshed".into());
        }

        // View shortcuts
        KeyCode::Char('0') => {
            state.view_stack.clear();
            state.view = View::Chat;
        }
        KeyCode::Char('1') => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        KeyCode::Char('2') | KeyCode::Char('n') if !matches!(state.view, View::Nodes) => {
            state.push_view(View::Nodes);
        }
        KeyCode::Char('2') | KeyCode::Char('n') => {}
        KeyCode::Char('3') if !matches!(state.view, View::Secrets) => {
            super::refresh_secrets_usage(client, state).await;
            secret_nav_first(state);
            state.push_view(View::Secrets);
        }
        KeyCode::Char('3') => {}
        KeyCode::Char('4') if !matches!(state.view, View::Backups) => {
            super::refresh_backups(client, state).await;
            state.selected_backup_node = 0;
            state.push_view(View::Backups);
        }
        KeyCode::Char('4') => {}
        KeyCode::Char('5') if !matches!(state.view, View::Webhooks) => {
            super::refresh_webhooks(client, state).await;
            state.selected_webhook = 0;
            state.push_view(View::Webhooks);
        }
        KeyCode::Char('5') => {}
        KeyCode::Char('6') if !matches!(state.view, View::Networks) => {
            super::refresh_networks(client, state).await;
            state.network_scroll = 0;
            state.push_view(View::Networks);
        }
        KeyCode::Char('6') => {}
        KeyCode::Char('7') if !matches!(state.view, View::Alerts) => {
            super::refresh_alerts(client, state).await;
            state.selected_alert = 0;
            state.push_view(View::Alerts);
        }
        KeyCode::Char('7') => {}
        // Toggle active vs all in the Alerts view. The `a` key is already
        // used by Webhooks for "add"; the matches!() guard keeps them apart.
        KeyCode::Char('a') if matches!(state.view, View::Alerts) => {
            state.alerts_show_all = !state.alerts_show_all;
            super::refresh_alerts(client, state).await;
            state.selected_alert = 0;
        }
        // Dismiss / resolve the currently-targeted alert (selected row in
        // list, or the conversation in detail). Lowercase `d` for dismiss,
        // capital `R` for resolve — same convention as everywhere else.
        KeyCode::Char('d') if matches!(state.view, View::Alerts | View::AlertDetail { .. }) => {
            crate::chat_dispatch::alert_action(
                state,
                client,
                crate::chat_dispatch::AlertAction::Dismiss,
            )
            .await;
        }
        KeyCode::Char('R') if matches!(state.view, View::Alerts | View::AlertDetail { .. }) => {
            crate::chat_dispatch::alert_action(
                state,
                client,
                crate::chat_dispatch::AlertAction::Resolve,
            )
            .await;
        }
        // `a` adds a webhook (claimed only in the Webhooks view to avoid
        // colliding with future global shortcuts).
        KeyCode::Char('a') if matches!(state.view, View::Webhooks) => {
            state.input_mode = InputMode::Command;
            state.command_input = "webhook-add ".into();
        }
        // `e` edits — opens command mode pre-filled with the current row's
        // identity so the user only types the changed field(s).
        KeyCode::Char('e') if matches!(state.view, View::Webhooks) => {
            if let Some(w) = state.webhooks.get(state.selected_webhook) {
                state.input_mode = InputMode::Command;
                state.command_input =
                    format!("webhook-edit {} {} {} ", w.repo, w.branch, w.service_name);
            }
        }
        // `b` triggers a manual backup on the selected node when in the
        // backups view. In other views the keybind is unused so it's safe to
        // claim here without surprising existing muscle memory.
        KeyCode::Char('b') if matches!(state.view, View::Backups) => {
            super::trigger_backup_for_selected(client, state).await;
        }

        // Actions
        KeyCode::Char('d') => {
            state.flash("Use `orca deploy` from CLI to redeploy".into());
        }
        KeyCode::Char('x') if matches!(state.view, View::Webhooks) => {
            super::delete_selected_webhook(client, state).await;
        }
        KeyCode::Char('x') => super::handle_stop(client, state).await,
        KeyCode::Char('s') => handle_scale_prompt(state),
        KeyCode::Char('p') => handle_project_filter(state),
        KeyCode::Char('w') => {
            if matches!(state.view, View::Logs { .. } | View::Detail { .. }) {
                state.word_wrap = !state.word_wrap;
                let mode = if state.word_wrap { "on" } else { "off" };
                state.flash(format!("Word wrap {mode}"));
            }
        }
        KeyCode::PageUp => match state.view {
            View::Logs { .. } => {
                state.service_scroll = state.service_scroll.saturating_add(20);
                state.auto_refresh_logs = false;
            }
            View::Networks => state.network_scroll = state.network_scroll.saturating_sub(10),
            _ => {}
        },
        KeyCode::PageDown => match state.view {
            View::Logs { .. } => {
                state.service_scroll = state.service_scroll.saturating_sub(20);
                if state.service_scroll == 0 {
                    state.auto_refresh_logs = true;
                }
            }
            View::Networks => state.network_scroll = state.network_scroll.saturating_add(10),
            _ => {}
        },
        _ => {}
    }
}

fn handle_esc(state: &mut AppState) {
    if !state.filter.is_empty() {
        state.filter.clear();
        state.selected_service = 0;
    } else if state.project_filter.is_some() {
        state.project_filter = None;
        state.selected_service = 0;
        state.flash("Project filter cleared".into());
    } else {
        state.pop_view();
    }
}

/// Navigation helpers for the secrets organizer. The view is a flat list of
/// group-headers interleaved with key rows, but selection should only land on
/// key rows. Going through `selectable_indices` keeps the contract in one
/// place (see `ui::secrets::flatten`).
fn secret_nav_first(state: &mut AppState) {
    let rows = crate::ui::secrets::flatten(&state.secrets_usage);
    if let Some(&i) = crate::ui::secrets::selectable_indices(&rows).first() {
        state.selected_secret = i;
    }
}

fn secret_nav_last(state: &mut AppState) {
    let rows = crate::ui::secrets::flatten(&state.secrets_usage);
    if let Some(&i) = crate::ui::secrets::selectable_indices(&rows).last() {
        state.selected_secret = i;
    }
}

fn secret_nav_next(state: &mut AppState) {
    let rows = crate::ui::secrets::flatten(&state.secrets_usage);
    let sel = crate::ui::secrets::selectable_indices(&rows);
    if let Some(next) = sel.iter().find(|&&i| i > state.selected_secret) {
        state.selected_secret = *next;
    }
}

fn secret_nav_prev(state: &mut AppState) {
    let rows = crate::ui::secrets::flatten(&state.secrets_usage);
    let sel = crate::ui::secrets::selectable_indices(&rows);
    if let Some(prev) = sel.iter().rev().find(|&&i| i < state.selected_secret) {
        state.selected_secret = *prev;
    }
}

/// Number of snapshots for the given node index, or 0 if the backup state
/// hasn't been fetched or the index is stale.
fn snapshot_count(state: &AppState, node_idx: usize) -> usize {
    state
        .backups
        .as_ref()
        .and_then(|b| b.nodes.get(node_idx))
        .map(|n| n.snapshots.len())
        .unwrap_or(0)
}

async fn handle_enter(state: &mut AppState, client: &ApiClient) {
    match state.view {
        View::Services => {
            if let Some(name) = state.selected_service_name() {
                let name = name.to_string();
                super::refresh_logs_named(client, state, &name).await;
                state.push_view(View::Detail { service: name });
            }
        }
        View::Backups => {
            let node_idx = state.selected_backup_node;
            let exists = state
                .backups
                .as_ref()
                .is_some_and(|b| node_idx < b.nodes.len());
            if exists {
                state.selected_backup_snapshot = 0;
                state.push_view(View::BackupSnapshots { node_idx });
            }
        }
        View::Webhooks => {
            if let Some(w) = state.webhooks.get(state.selected_webhook) {
                let service = w.service_name.clone();
                super::refresh_webhook_invocations(client, state, &service).await;
                state.push_view(View::WebhookInvocations { service });
            }
        }
        View::Secrets => {
            if let Some(u) = crate::ui::secrets::selected_key(state) {
                let key = u.key.clone();
                state.push_view(View::SecretRefs { key });
            }
        }
        View::Alerts => {
            if let Some(a) = state.alerts.get(state.selected_alert) {
                let id = a.id.to_string();
                state.alert_detail_scroll = 0;
                state.push_view(View::AlertDetail { id });
            }
        }
        _ => {}
    }
}

async fn handle_logs(state: &mut AppState, client: &ApiClient) {
    let svc_name = super::current_service_name(state);
    if let Some(name) = svc_name {
        super::refresh_logs_named(client, state, &name).await;
        state.service_scroll = 0;
        state.auto_refresh_logs = true;
        state.push_view(View::Logs { service: name });
    }
}

/// Prompt user for scale command via command mode.
fn handle_scale_prompt(state: &mut AppState) {
    if let Some(name) = super::current_service_name(state) {
        state.input_mode = InputMode::Command;
        state.command_input = format!("scale {name} ");
    }
}

/// Filter services by the project of the selected service.
fn handle_project_filter(state: &mut AppState) {
    if !matches!(state.view, View::Services) {
        return;
    }
    if let Some(svc) = state.selected_service_data() {
        if let Some(proj) = &svc.project {
            let proj = proj.clone();
            state.flash(format!("Filtered to project: {proj}"));
            state.project_filter = Some(proj);
            state.selected_service = 0;
        } else {
            state.flash("Service has no project".into());
        }
    }
}
