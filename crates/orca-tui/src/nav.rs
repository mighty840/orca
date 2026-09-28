//! Movement keys: j/k, g/G and PgUp/PgDn in every view.

use std::sync::atomic::Ordering;

use crossterm::event::KeyCode;

use crate::state::{AppState, View};

/// Move the selection or scroll position. Returns false for any other key,
/// which the caller then handles.
pub(crate) fn navigate(state: &mut AppState, code: KeyCode) -> bool {
    match code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => match state.view {
            View::Nodes => {
                if state.selected_node + 1 < state.nodes.len() {
                    state.selected_node += 1;
                }
            }
            View::Secrets => crate::secrets_actions::secret_nav_next(state),
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
                if state.selected_webhook + 1 < state.visible_webhooks().len() {
                    state.selected_webhook += 1;
                }
            }
            View::Networks => state.network_scroll = state.network_scroll.saturating_add(1),
            View::Help => state.help_scroll = state.help_scroll.saturating_add(1),
            View::Alerts => {
                if state.selected_alert + 1 < state.visible_alerts().len() {
                    state.selected_alert += 1;
                }
            }
            View::AlertDetail { .. } => {
                let max = state.alert_detail_max.load(Ordering::Relaxed);
                state.alert_detail_scroll = (state.alert_detail_scroll + 1).min(max);
            }
            _ => state.next_service(),
        },
        KeyCode::Char('k') | KeyCode::Up => match state.view {
            View::Nodes => state.selected_node = state.selected_node.saturating_sub(1),
            View::Secrets => crate::secrets_actions::secret_nav_prev(state),
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
            View::Help => state.help_scroll = state.help_scroll.saturating_sub(1),
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
            View::Secrets => crate::secrets_actions::secret_nav_first(state),
            View::Backups => state.selected_backup_node = 0,
            View::BackupSnapshots { .. } => state.selected_backup_snapshot = 0,
            View::Webhooks => state.selected_webhook = 0,
            View::Networks => state.network_scroll = 0,
            View::Alerts => state.selected_alert = 0,
            View::AlertDetail { .. } => state.alert_detail_scroll = 0,
            _ => state.selected_service = 0,
        },
        KeyCode::Char('G') => match state.view {
            View::Secrets => crate::secrets_actions::secret_nav_last(state),
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
                if !state.visible_webhooks().is_empty() {
                    state.selected_webhook = state.visible_webhooks().len() - 1;
                }
            }
            View::Networks => {
                // Snap to last line; render clamps to the visible window.
                let total = super::ui::networks::rendered_line_count(state);
                state.network_scroll = total.saturating_sub(1);
            }
            View::Alerts => {
                if !state.visible_alerts().is_empty() {
                    state.selected_alert = state.visible_alerts().len() - 1;
                }
            }
            View::AlertDetail { .. } => {
                state.alert_detail_scroll = state.alert_detail_max.load(Ordering::Relaxed);
            }
            _ => {
                let len = state.filtered_services().len();
                if len > 0 {
                    state.selected_service = len - 1;
                }
            }
        },
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
        _ => return false,
    }
    true
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
