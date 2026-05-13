pub mod api;
mod commands;
mod keys;
mod persist;
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

    // Optimistically apply the last project filter so the user sees their
    // saved view immediately. The first successful refresh validates that the
    // project still exists; if not, refresh() clears it.
    let persisted = persist::load();
    if let Some(p) = persisted.last_project {
        state.project_filter = Some(p.clone());
        state.pending_restore_project = Some(p);
    }

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

        // Snapshot the project filter so we can detect changes and persist
        // them exactly once below, regardless of which handler mutated it.
        let prev_project_filter = state.project_filter.clone();

        match state.input_mode {
            InputMode::Filter => keys::handle_filter_key(state, key.code),
            InputMode::Command => keys::handle_command_key(state, client, key.code).await,
            InputMode::Normal => {
                keys::handle_normal_key(state, client, key.code, &mut last_refresh).await;
            }
        }

        if state.project_filter != prev_project_filter {
            persist::save(state);
        }

        // If a :sh / :exec command left a pending shell request on the
        // state, suspend the ratatui alternate-screen, run the child
        // command with inherited stdio, then rebuild the screen.
        if let Some((service, node, cmd)) = state.pending_shell.take() {
            if let Err(e) = run_container_shell(terminal, &service, node.as_deref(), &cmd) {
                state.error = Some(format!("Exec failed: {e}"));
            } else {
                state.flash(format!("Shell in {service} exited"));
            }
        }

        if state.should_quit {
            return Ok(());
        }
    }
}

/// Suspend ratatui and run `docker exec -it` (or `ssh <node> docker exec`
/// for remote services), blocking until the child exits.
fn run_container_shell(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    service: &str,
    node: Option<&str>,
    cmd: &[String],
) -> anyhow::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Remote services: delegate to `orca exec` which connects via the master WS exec channel.
    // Local services: direct docker exec.
    let mut child = if node.is_some() {
        let mut c = std::process::Command::new("orca");
        c.arg("exec").arg(service);
        for a in cmd {
            c.arg(a);
        }
        c
    } else {
        let container = format!("orca-{service}");
        let mut c = std::process::Command::new("docker");
        c.args(["exec", "-it", &container]);
        for a in cmd {
            c.arg(a);
        }
        c
    };
    let status = child.status()?;

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    if !status.success() {
        anyhow::bail!("exit status {status}");
    }
    Ok(())
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
        Ok(resp) => {
            state.update_status(resp);
            try_restore_project_filter(state);
        }
        Err(e) => {
            state.mark_disconnected();
            state.error = Some(format!("API error: {e}"));
        }
    }
    if let Ok(info) = client.cluster_info().await {
        state.update_cluster(info);
    }
}

/// Validate the project loaded from disk against the freshly-fetched service
/// list. Runs at most once per session (the pending value is consumed). If the
/// project still exists, flash a confirmation. If it does not, drop the filter
/// and persist the cleared state so the next launch starts clean.
pub(crate) fn try_restore_project_filter(state: &mut AppState) {
    let Some(target) = state.pending_restore_project.take() else {
        return;
    };
    let exists = state
        .services
        .iter()
        .any(|s| s.project.as_deref() == Some(target.as_str()));
    if exists {
        state.flash(format!("Restored filter: {target}"));
    } else {
        state.project_filter = None;
        persist::save(state);
        state.flash(format!(
            "Last project '{target}' no longer exists — filter cleared"
        ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::ServiceStatus;

    fn svc_with_project(name: &str, project: Option<&str>) -> ServiceStatus {
        ServiceStatus {
            name: name.into(),
            image: String::new(),
            runtime: "container".into(),
            desired_replicas: 1,
            running_replicas: 1,
            status: "running".into(),
            domain: None,
            project: project.map(String::from),
            memory_usage: None,
            cpu_percent: None,
            node: None,
            memory_limit_bytes: None,
        }
    }

    /// With nothing pending, the function must be a no-op: the user is just
    /// navigating, not relaunching, so we don't want to clobber state or fire
    /// a stale flash.
    #[test]
    fn no_pending_restore_is_noop() {
        let mut state = AppState::new();
        state.project_filter = Some("compliance".into());
        state.services = vec![svc_with_project("api", Some("compliance"))];

        try_restore_project_filter(&mut state);

        assert_eq!(state.project_filter.as_deref(), Some("compliance"));
        assert!(state.pending_restore_project.is_none());
        assert!(state.status_msg.is_none());
    }

    /// When the persisted project still has at least one service, the filter
    /// stays active and the user gets a confirmation flash. The pending slot
    /// is consumed so the next refresh doesn't re-trigger.
    #[test]
    fn pending_with_matching_project_flashes_restored() {
        let mut state = AppState::new();
        state.project_filter = Some("compliance".into());
        state.pending_restore_project = Some("compliance".into());
        state.services = vec![
            svc_with_project("api", Some("compliance")),
            svc_with_project("web", Some("frontend")),
        ];

        try_restore_project_filter(&mut state);

        assert_eq!(state.project_filter.as_deref(), Some("compliance"));
        assert!(state.pending_restore_project.is_none());
        assert_eq!(
            state.status_msg.as_deref(),
            Some("Restored filter: compliance"),
        );
    }

    /// If the persisted project has been deleted between sessions, drop the
    /// filter and tell the user. Without this, the TUI would show an empty
    /// list with no explanation and the stale value would survive the next
    /// launch too.
    #[test]
    fn pending_with_missing_project_clears_filter() {
        let mut state = AppState::new();
        state.project_filter = Some("compliance".into());
        state.pending_restore_project = Some("compliance".into());
        state.services = vec![svc_with_project("web", Some("frontend"))];

        try_restore_project_filter(&mut state);

        assert!(state.project_filter.is_none());
        assert!(state.pending_restore_project.is_none());
        let msg = state.status_msg.as_deref().unwrap_or("");
        assert!(
            msg.contains("compliance") && msg.contains("no longer exists"),
            "expected explanatory flash, got: {msg:?}"
        );
    }

    /// An empty service list (cluster just booted, nothing deployed) counts as
    /// "project missing" — we don't have any way to know it'll come back, so
    /// clear cleanly rather than holding the user in a stuck filtered view.
    #[test]
    fn pending_with_empty_service_list_clears_filter() {
        let mut state = AppState::new();
        state.project_filter = Some("compliance".into());
        state.pending_restore_project = Some("compliance".into());

        try_restore_project_filter(&mut state);

        assert!(state.project_filter.is_none());
        assert!(state.pending_restore_project.is_none());
    }
}
