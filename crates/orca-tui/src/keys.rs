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
        KeyCode::Enter => handle_enter(state, client).await,

        // Full-screen logs
        KeyCode::Char('l') => handle_logs(state, client).await,

        // Refresh
        KeyCode::Char('r') => {
            super::refresh(client, state).await;
            *last_refresh = tokio::time::Instant::now();
            state.flash("Refreshed".into());
        }

        // View shortcuts
        KeyCode::Char('1') => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        KeyCode::Char('2') | KeyCode::Char('n') => {
            if !matches!(state.view, View::Nodes) {
                state.push_view(View::Nodes);
            }
        }
        KeyCode::Char('3') | KeyCode::Char('m') => {
            if !matches!(state.view, View::Metrics) {
                if let Ok(text) = client.metrics().await {
                    state.metrics_text = text;
                }
                state.push_view(View::Metrics);
            }
        }

        // Actions
        KeyCode::Char('d') => {
            state.flash("Use `orca deploy` from CLI to redeploy".into());
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

async fn handle_enter(state: &mut AppState, client: &ApiClient) {
    if matches!(state.view, View::Services)
        && let Some(name) = state.selected_service_name()
    {
        let name = name.to_string();
        super::refresh_logs_named(client, state, &name).await;
        state.push_view(View::Detail { service: name });
    }
}

async fn handle_logs(state: &mut AppState, client: &ApiClient) {
    let svc_name = super::current_service_name(state);
    if let Some(name) = svc_name {
        super::refresh_logs_named(client, state, &name).await;
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
