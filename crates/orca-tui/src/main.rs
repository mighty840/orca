mod api;
mod state;
mod ui;

use std::io;
use std::time::Duration;

use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use api::ApiClient;
use state::{AppState, Panel};

#[derive(Parser)]
#[command(name = "orca-tui", about = "Orca cluster dashboard")]
struct Args {
    /// API server address
    #[arg(long, default_value = "http://127.0.0.1:6880")]
    api: String,
    /// Refresh interval in seconds
    #[arg(long, default_value = "2")]
    interval: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let client = ApiClient::new(&args.api);
    let mut state = AppState::new();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run(&mut terminal, &client, &mut state, args.interval).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    client: &ApiClient,
    state: &mut AppState,
    interval_secs: u64,
) -> anyhow::Result<()> {
    let mut last_refresh = tokio::time::Instant::now() - Duration::from_secs(interval_secs); // force immediate refresh

    loop {
        // Refresh data periodically
        if last_refresh.elapsed() >= Duration::from_secs(interval_secs) {
            refresh(client, state).await;
            last_refresh = tokio::time::Instant::now();
        }

        // Draw
        terminal.draw(|f| ui::draw(f, state))?;

        // Handle input (non-blocking, 100ms timeout)
        if event::poll(Duration::from_millis(100))?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            match key.code {
                KeyCode::Char('q') => state.should_quit = true,
                KeyCode::Char('j') | KeyCode::Down => state.next_service(),
                KeyCode::Char('k') | KeyCode::Up => state.prev_service(),
                KeyCode::Tab => state.next_panel(),
                KeyCode::Char('l') => {
                    state.panel = Panel::Logs;
                    refresh_logs(client, state).await;
                }
                KeyCode::Char('n') => state.panel = Panel::Nodes,
                KeyCode::Enter => {
                    refresh_logs(client, state).await;
                }
                _ => {}
            }
        }

        if state.should_quit {
            return Ok(());
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
