pub mod api;
pub mod state;
pub mod ui;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use api::ApiClient;
use state::{AppState, InputMode, Panel};

/// Run the TUI dashboard against the given API URL.
pub async fn run_tui(api_url: &str) -> anyhow::Result<()> {
    let client = ApiClient::new(api_url);
    let mut state = AppState::new();

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

    loop {
        if last_refresh.elapsed() >= Duration::from_secs(2) {
            refresh(client, state).await;
            last_refresh = tokio::time::Instant::now();
        }

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

        // Clear transient status messages on any keypress.
        state.status_msg = None;

        match state.input_mode {
            InputMode::Filter => handle_filter_key(state, key.code),
            InputMode::Normal => {
                handle_normal_key(state, client, key.code, &mut last_refresh).await;
            }
        }

        if state.should_quit {
            return Ok(());
        }
    }
}

fn handle_filter_key(state: &mut AppState, code: KeyCode) {
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

async fn handle_normal_key(
    state: &mut AppState,
    client: &ApiClient,
    code: KeyCode,
    last_refresh: &mut tokio::time::Instant,
) {
    // Help overlay dismisses on any key.
    if state.show_help {
        state.show_help = false;
        return;
    }

    match code {
        KeyCode::Char('q') => state.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => state.next_service(),
        KeyCode::Char('k') | KeyCode::Up => state.prev_service(),
        KeyCode::Tab => state.next_panel(),
        KeyCode::Char('1') => state.panel = Panel::Services,
        KeyCode::Char('2') => {
            state.panel = Panel::Logs;
            refresh_logs(client, state).await;
        }
        KeyCode::Char('3') => state.panel = Panel::Nodes,
        KeyCode::Char('l') => {
            state.panel = Panel::Logs;
            refresh_logs(client, state).await;
        }
        KeyCode::Char('n') => state.panel = Panel::Nodes,
        KeyCode::Char('/') => {
            state.input_mode = InputMode::Filter;
        }
        KeyCode::Char('?') => {
            state.show_help = true;
        }
        KeyCode::Char('r') => {
            refresh(client, state).await;
            *last_refresh = tokio::time::Instant::now();
            state.status_msg = Some("Refreshed".into());
        }
        KeyCode::Char('d') => {
            handle_deploy(client, state).await;
        }
        KeyCode::Char('x') => {
            handle_stop(client, state).await;
        }
        KeyCode::Char('s') => {
            // Show current scale info.
            if let Some(svc) = state.selected_service_data() {
                state.status_msg = Some(format!(
                    "{}: {}/{} replicas",
                    svc.name, svc.running_replicas, svc.desired_replicas
                ));
            }
        }
        KeyCode::Enter => {
            if state.panel == Panel::Detail {
                state.panel = state.prev_panel;
            } else {
                state.prev_panel = state.panel;
                state.panel = Panel::Detail;
                refresh_logs(client, state).await;
            }
        }
        KeyCode::Esc => {
            if state.panel == Panel::Detail {
                state.panel = state.prev_panel;
            } else if !state.filter.is_empty() {
                state.filter.clear();
                state.selected_service = 0;
            }
        }
        _ => {}
    }
}

async fn refresh(client: &ApiClient, state: &mut AppState) {
    state.error = None;
    match client.status().await {
        Ok(resp) => state.update_status(resp),
        Err(e) => state.error = Some(format!("API error: {e}")),
    }
    if let Ok(info) = client.cluster_info().await {
        state.update_cluster(info);
    }
}

async fn refresh_logs(client: &ApiClient, state: &mut AppState) {
    if let Some(name) = state.selected_service_name() {
        let name = name.to_string();
        match client.logs(&name, 50).await {
            Ok(logs) => state.logs = logs,
            Err(e) => state.logs = format!("Failed to fetch logs: {e}"),
        }
    }
}

async fn handle_deploy(client: &ApiClient, state: &mut AppState) {
    if let Some(name) = state.selected_service_name() {
        let name = name.to_string();
        match client.deploy(&name).await {
            Ok(()) => state.status_msg = Some(format!("Deployed {name}")),
            Err(e) => state.error = Some(format!("Deploy failed: {e}")),
        }
    }
}

async fn handle_stop(client: &ApiClient, state: &mut AppState) {
    if let Some(name) = state.selected_service_name() {
        let name = name.to_string();
        match client.stop(&name).await {
            Ok(()) => state.status_msg = Some(format!("Stopped {name}")),
            Err(e) => state.error = Some(format!("Stop failed: {e}")),
        }
    }
}
