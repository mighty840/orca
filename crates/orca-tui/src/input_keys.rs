//! Key handling for the non-normal input modes: the `/` filter line and
//! the `:` command bar. Normal-mode dispatch lives in `keys.rs`.

use crossterm::event::KeyCode;

use crate::api::ApiClient;
use crate::state::{AppState, InputMode, View};

pub fn handle_filter_key(state: &mut AppState, code: KeyCode) {
    // In the Logs view `/` searches the log; elsewhere it filters services.
    let logs = matches!(state.view, View::Logs { .. });
    let text = if logs {
        &mut state.log_search
    } else {
        &mut state.filter
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
    if !logs {
        state.selected_service = 0;
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
