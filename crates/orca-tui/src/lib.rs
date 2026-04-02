pub mod api;
mod commands;
mod keys;
pub mod state;
pub mod ui;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use api::ApiClient;
use state::{AppState, InputMode, View};

/// Run the TUI dashboard against the given API URL.
pub async fn run_tui(api_url: &str) -> anyhow::Result<()> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        anyhow::bail!("TUI requires an interactive terminal. Use `ssh -t` for remote access.");
    }

    let client = ApiClient::new(api_url);
    let mut state = AppState::new();
    state.api_url = client.url().to_string();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &client, &mut state).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &ApiClient,
    state: &mut AppState,
) -> anyhow::Result<()> {
    let mut last_refresh = tokio::time::Instant::now() - Duration::from_secs(2);
    let mut last_log_refresh = tokio::time::Instant::now() - Duration::from_secs(2);

    loop {
        // Global data refresh every 2s.
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            refresh(client, state).await;
            last_refresh = tokio::time::Instant::now();
        }

        // Auto-refresh logs when in Logs view.
        if matches!(state.view, View::Logs { .. })
            && state.auto_refresh_logs
            && last_log_refresh.elapsed() >= Duration::from_secs(2)
        {
            refresh_logs_for_view(client, state).await;
            last_log_refresh = tokio::time::Instant::now();
        }

        state.tick = state.tick.wrapping_add(1);
        state.maybe_clear_flash();

        terminal.draw(|f| ui::draw(f, state))?;

        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }

        match state.input_mode {
            InputMode::Filter => keys::handle_filter_key(state, key.code),
            InputMode::Command => keys::handle_command_key(state, client, key.code).await,
            InputMode::Normal => {
                keys::handle_normal_key(state, client, key.code, &mut last_refresh).await;
            }
        }

        if state.should_quit {
            return Ok(());
        }
    }
}

/// Get the service name from the current view context or selection.
fn current_service_name(state: &AppState) -> Option<String> {
    match &state.view {
        View::Detail { service } | View::Logs { service } => Some(service.clone()),
        View::Services => state.selected_service_name().map(|s| s.to_string()),
        _ => None,
    }
}

async fn refresh(client: &ApiClient, state: &mut AppState) {
    state.error = None;
    match client.status().await {
        Ok(resp) => state.update_status(resp),
        Err(e) => {
            state.mark_disconnected();
            state.error = Some(format!("API error: {e}"));
        }
    }
    if let Ok(info) = client.cluster_info().await {
        state.update_cluster(info);
    }
}

async fn refresh_logs_for_view(client: &ApiClient, state: &mut AppState) {
    if let View::Logs { service } = &state.view {
        let name = service.clone();
        refresh_logs_named(client, state, &name).await;
    }
}

async fn refresh_logs_named(client: &ApiClient, state: &mut AppState, name: &str) {
    match client.logs(name, 50).await {
        Ok(logs) => state.logs = logs,
        Err(e) => state.logs = format!("Failed to fetch logs: {e}"),
    }
}

async fn handle_stop(client: &ApiClient, state: &mut AppState) {
    if let Some(name) = current_service_name(state) {
        match client.stop(&name).await {
            Ok(()) => state.flash(format!("Stopped {name}")),
            Err(e) => state.error = Some(format!("Stop failed: {e}")),
        }
    }
}
