//! Command-mode handlers for `:` commands.

use crate::api::ApiClient;
use crate::state::{AppState, View};

pub async fn execute_command(state: &mut AppState, client: &ApiClient, cmd: &str) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some("q" | "quit") => state.should_quit = true,
        Some("services" | "svc") => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        Some("nodes") => state.push_view(View::Nodes),
        Some("metrics") => {
            if let Ok(text) = client.metrics().await {
                state.metrics_text = text;
            }
            state.push_view(View::Metrics);
        }
        Some("logs") => cmd_logs(state, client, &parts).await,
        Some("help") => state.push_view(View::Help),
        Some("scale") => cmd_scale(state, client, &parts).await,
        Some("stop") => cmd_stop(state, client, &parts).await,
        Some("stop-project") => cmd_stop_project(state, client, &parts).await,
        Some("deploy") => {
            state.flash("Use `orca deploy` from CLI to redeploy all services".into());
        }
        Some("filter" | "f") => cmd_filter(state, &parts),
        Some("project") => cmd_project(state, &parts),
        Some("exec") => {
            if parts.len() >= 2 {
                let rest = parts[1..].join(" ");
                state.flash(format!("Use: orca exec {rest}"));
            } else {
                state.flash("Usage: :exec <service> <cmd...>".into());
            }
        }
        Some("drain") => cmd_drain(state, client, &parts).await,
        Some("undrain") => cmd_undrain(state, client, &parts).await,
        Some(other) => state.flash(format!("Unknown command: {other}")),
        None => {}
    }
}

async fn cmd_logs(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    let svc_name = if let Some(name) = parts.get(1) {
        (*name).to_string()
    } else if let Some(name) = state.selected_service_name() {
        name.to_string()
    } else {
        state.flash("Usage: :logs <service>".into());
        return;
    };
    crate::refresh_logs_named(client, state, &svc_name).await;
    state.push_view(View::Logs { service: svc_name });
}

async fn cmd_scale(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 3 {
        state.flash("Usage: :scale <service> <count>".into());
        return;
    }
    let name = parts[1];
    let count: u32 = match parts[2].parse() {
        Ok(n) => n,
        Err(_) => {
            state.flash("Invalid replica count".into());
            return;
        }
    };
    match client.scale(name, count).await {
        Ok(()) => state.flash(format!("Scaled {name} to {count}")),
        Err(e) => state.error = Some(format!("Scale failed: {e}")),
    }
}

async fn cmd_stop(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    let name = if let Some(n) = parts.get(1) {
        (*n).to_string()
    } else if let Some(n) = state.selected_service_name() {
        n.to_string()
    } else {
        state.flash("Usage: :stop <service>".into());
        return;
    };
    match client.stop(&name).await {
        Ok(()) => state.flash(format!("Stopped {name}")),
        Err(e) => state.error = Some(format!("Stop failed: {e}")),
    }
}

async fn cmd_stop_project(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :stop-project <project>".into());
        return;
    }
    let project = parts[1];
    match client.stop_project(project).await {
        Ok(()) => state.flash(format!("Stopped project {project}")),
        Err(e) => state.error = Some(format!("Stop project failed: {e}")),
    }
}

fn cmd_filter(state: &mut AppState, parts: &[&str]) {
    if parts.len() < 2 {
        state.filter.clear();
        state.selected_service = 0;
        state.flash("Filter cleared".into());
    } else {
        state.filter = parts[1..].join(" ");
        state.selected_service = 0;
    }
}

fn cmd_project(state: &mut AppState, parts: &[&str]) {
    if parts.len() < 2 {
        state.project_filter = None;
        state.selected_service = 0;
        state.flash("Project filter cleared".into());
    } else {
        let proj = parts[1].to_string();
        state.flash(format!("Filtered to project: {proj}"));
        state.project_filter = Some(proj);
        state.selected_service = 0;
    }
}

async fn cmd_drain(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :drain <node_id>".into());
        return;
    }
    let node_id: u64 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => {
            state.flash("Invalid node ID".into());
            return;
        }
    };
    match client.drain(node_id).await {
        Ok(()) => state.flash(format!("Draining node {node_id}")),
        Err(e) => state.error = Some(format!("Drain failed: {e}")),
    }
}

async fn cmd_undrain(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :undrain <node_id>".into());
        return;
    }
    let node_id: u64 = match parts[1].parse() {
        Ok(n) => n,
        Err(_) => {
            state.flash("Invalid node ID".into());
            return;
        }
    };
    match client.undrain(node_id).await {
        Ok(()) => state.flash(format!("Undrained node {node_id}")),
        Err(e) => state.error = Some(format!("Undrain failed: {e}")),
    }
}
