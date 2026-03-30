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
use state::{AppState, Panel};

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

        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('j') | KeyCode::Down => state.next_service(),
                KeyCode::Char('k') | KeyCode::Up => state.prev_service(),
                KeyCode::Tab => state.next_panel(),
                KeyCode::Char('l') => {
                    state.panel = Panel::Logs;
                    refresh_logs(client, state).await;
                }
                KeyCode::Char('n') => state.panel = Panel::Nodes,
                KeyCode::Enter => refresh_logs(client, state).await,
                _ => {}
            }
        }
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
