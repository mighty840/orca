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
            execute_command(state, client, &cmd).await;
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

async fn execute_command(state: &mut AppState, client: &ApiClient, cmd: &str) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some("q" | "quit") => state.should_quit = true,
        Some("services" | "svc") => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        Some("nodes") => {
            state.push_view(View::Nodes);
        }
        Some("logs") => {
            let svc_name = if let Some(name) = parts.get(1) {
                (*name).to_string()
            } else if let Some(name) = state.selected_service_name() {
                name.to_string()
            } else {
                state.flash("Usage: :logs <service>".into());
                return;
            };
            super::refresh_logs_named(client, state, &svc_name).await;
            state.push_view(View::Logs { service: svc_name });
        }
        Some("help") => {
            state.push_view(View::Help);
        }
        Some(other) => {
            state.flash(format!("Unknown command: {other}"));
        }
        None => {}
    }
}

pub async fn handle_normal_key(
    state: &mut AppState,
    client: &ApiClient,
    code: KeyCode,
    last_refresh: &mut tokio::time::Instant,
) {
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
        KeyCode::Char('?') => {
            state.push_view(View::Help);
        }
        KeyCode::Esc => {
            if !state.filter.is_empty() {
                state.filter.clear();
                state.selected_service = 0;
            } else {
                state.pop_view();
            }
        }

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => state.next_service(),
        KeyCode::Char('k') | KeyCode::Up => state.prev_service(),
        KeyCode::Char('g') => state.selected_service = 0,
        KeyCode::Char('G') => {
            let len = state.filtered_services().len();
            if len > 0 {
                state.selected_service = len - 1;
            }
        }

        // Enter detail view
        KeyCode::Enter => {
            if matches!(state.view, View::Services)
                && let Some(name) = state.selected_service_name()
            {
                let name = name.to_string();
                super::refresh_logs_named(client, state, &name).await;
                state.push_view(View::Detail { service: name });
            }
        }

        // Full-screen logs
        KeyCode::Char('l') => {
            let svc_name = super::current_service_name(state);
            if let Some(name) = svc_name {
                super::refresh_logs_named(client, state, &name).await;
                state.push_view(View::Logs { service: name });
            }
        }

        // Refresh
        KeyCode::Char('r') => {
            super::refresh(client, state).await;
            *last_refresh = tokio::time::Instant::now();
            state.flash("Refreshed".into());
        }

        // Actions
        KeyCode::Char('d') => super::handle_deploy(client, state).await,
        KeyCode::Char('x') => super::handle_stop(client, state).await,
        KeyCode::Char('s') => {
            if let Some(svc) = super::current_service_data(state) {
                state.flash(format!(
                    "{}: {}/{} replicas",
                    svc.name, svc.running_replicas, svc.desired_replicas
                ));
            }
        }
        KeyCode::Char('w') => {
            if matches!(state.view, View::Logs { .. } | View::Detail { .. }) {
                state.word_wrap = !state.word_wrap;
                let mode = if state.word_wrap { "on" } else { "off" };
                state.flash(format!("Word wrap {mode}"));
            }
        }
        _ => {}
    }
}
