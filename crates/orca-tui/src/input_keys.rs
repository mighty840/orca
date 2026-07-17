//! Key handling for the non-normal input modes: the `/` filter line and
//! the `:` command bar. Normal-mode dispatch lives in `keys.rs`.

use crossterm::event::KeyCode;

use crate::api::ApiClient;
use crate::state::{AppState, InputMode};

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
