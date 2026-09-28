//! Key handling for the non-normal input modes: the `/` filter line and
//! the `:` command bar. Normal-mode dispatch lives in `keys.rs`.

use crossterm::event::KeyCode;

use crate::api::ApiClient;
use crate::state::{AppState, InputMode, View};

/// The text `/` edits in the current view, if the view has one.
pub fn filter_text(state: &mut AppState) -> Option<&mut String> {
    match state.view {
        View::Services => Some(&mut state.filter),
        View::Logs { .. } => Some(&mut state.log_search),
        View::Secrets => Some(&mut state.secret_filter),
        View::Webhooks => Some(&mut state.webhook_filter),
        View::Alerts => Some(&mut state.alert_filter),
        _ => None,
    }
}

/// Read-only [`filter_text`], for the command bar.
pub fn filter_value(state: &AppState) -> &str {
    match state.view {
        View::Logs { .. } => &state.log_search,
        View::Secrets => &state.secret_filter,
        View::Webhooks => &state.webhook_filter,
        View::Alerts => &state.alert_filter,
        _ => &state.filter,
    }
}

pub fn handle_filter_key(state: &mut AppState, code: KeyCode) {
    let Some(text) = filter_text(state) else {
        state.input_mode = InputMode::Normal;
        return;
    };
    match code {
        KeyCode::Esc => {
            text.clear();
            state.input_mode = InputMode::Normal;
        }
        KeyCode::Enter => state.input_mode = InputMode::Normal,
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) => text.push(c),
        _ => return,
    }
    // The list changed under the cursor: start from its first row.
    match state.view {
        View::Services => state.selected_service = 0,
        View::Secrets => crate::secrets_actions::secret_nav_first(state),
        View::Webhooks => state.selected_webhook = 0,
        View::Alerts => state.selected_alert = 0,
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
