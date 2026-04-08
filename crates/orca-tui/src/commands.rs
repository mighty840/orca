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
        Some("exec") => cmd_exec(state, &parts),
        Some("sh") => cmd_sh(state, &parts),
        Some("drain") => cmd_drain(state, client, &parts).await,
        Some("undrain") => cmd_undrain(state, client, &parts).await,
        Some("secrets") => {
            if let Ok(keys) = client.list_secrets().await {
                state.secret_keys = keys;
                state.selected_secret = 0;
            }
            state.push_view(View::Secrets);
        }
        Some("set") => cmd_secret_set(state, client, &parts).await,
        Some("rm") => cmd_secret_rm(state, client, &parts).await,
        Some(other) => state.flash(format!("Unknown command: {other}")),
        None => {}
    }
}

async fn cmd_secret_set(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    // `:set KEY VALUE...` — value may contain spaces, so re-join the tail.
    if parts.len() < 3 {
        state.flash("Usage: :set <KEY> <value...>".into());
        return;
    }
    let key = parts[1];
    let value = parts[2..].join(" ");
    match client.set_secret(key, &value).await {
        Ok(()) => {
            state.flash(format!("Secret {key} set"));
            if let Ok(keys) = client.list_secrets().await {
                state.secret_keys = keys;
            }
        }
        Err(e) => state.error = Some(format!("Set secret failed: {e}")),
    }
}

/// `:sh [service]` — open an interactive `/bin/sh` inside the selected
/// service's container. Stores a pending shell request on the state; the
/// event loop handles the actual suspend/resume of the TUI.
fn cmd_sh(state: &mut AppState, parts: &[&str]) {
    let (name, node) = match resolve_service(state, parts) {
        Some(v) => v,
        None => return,
    };
    state.pending_shell = Some((name, node, vec!["/bin/sh".to_string()]));
}

/// `:exec <service> <cmd...>` — run an arbitrary command in a container.
fn cmd_exec(state: &mut AppState, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :exec <service> <cmd...>".into());
        return;
    }
    // Detect whether the second arg is a service name — if so, use it;
    // otherwise default to the selected row and treat everything after
    // `:exec` as the command.
    let (name, node, cmd): (String, Option<String>, Vec<String>) = {
        let by_name = state.services.iter().find(|s| s.name == parts[1]);
        if let Some(svc) = by_name {
            let cmd: Vec<String> = if parts.len() >= 3 {
                parts[2..].iter().map(|s| s.to_string()).collect()
            } else {
                vec!["/bin/sh".to_string()]
            };
            (svc.name.clone(), svc.node.clone(), cmd)
        } else if let Some(svc) = state.selected_service_data() {
            let cmd: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            (svc.name.clone(), svc.node.clone(), cmd)
        } else {
            state.flash("Usage: :exec <service> <cmd...>".into());
            return;
        }
    };
    state.pending_shell = Some((name, node, cmd));
}

/// Common resolver for `:sh` — picks a service by name if given, falls
/// back to the selected row otherwise.
fn resolve_service(state: &AppState, parts: &[&str]) -> Option<(String, Option<String>)> {
    if let Some(name) = parts.get(1)
        && let Some(svc) = state.services.iter().find(|s| s.name == *name)
    {
        return Some((svc.name.clone(), svc.node.clone()));
    }
    state
        .selected_service_data()
        .map(|s| (s.name.clone(), s.node.clone()))
}

async fn cmd_secret_rm(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :rm <KEY>".into());
        return;
    }
    let key = parts[1];
    match client.remove_secret(key).await {
        Ok(()) => {
            state.flash(format!("Secret {key} removed"));
            if let Ok(keys) = client.list_secrets().await {
                state.secret_keys = keys;
                if state.selected_secret >= state.secret_keys.len() && !state.secret_keys.is_empty()
                {
                    state.selected_secret = state.secret_keys.len() - 1;
                }
            }
        }
        Err(e) => state.error = Some(format!("Remove secret failed: {e}")),
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
