//! Command-mode handlers for `:` commands.

use crate::api::{ApiClient, ServiceAction};
use crate::confirm::Confirm;
use crate::state::{AppState, View};

pub async fn execute_command(state: &mut AppState, client: &ApiClient, cmd: &str) {
    let parts: Vec<&str> = cmd.split_whitespace().collect();
    match parts.first().copied() {
        Some("q" | "quit") => state.should_quit = true,
        Some("chat") => {
            state.view_stack.clear();
            state.view = View::Chat;
        }
        Some("services" | "svc") => {
            state.view_stack.clear();
            state.view = View::Services;
        }
        Some("nodes") => state.push_view(View::Nodes),
        Some("backups") => {
            crate::refresh_backups(client, state);
            state.selected_backup_node = 0;
            state.push_view(View::Backups);
        }
        Some("logs") => cmd_logs(state, client, &parts).await,
        Some("help") => state.push_view(View::Help),
        Some("scale") => cmd_scale(state, client, &parts).await,
        Some("stop") => cmd_stop(state, &parts),
        Some(verb @ ("start" | "redeploy" | "rollback" | "promote")) => {
            let action = match verb {
                "start" => ServiceAction::Start,
                "redeploy" => ServiceAction::Redeploy,
                "rollback" => ServiceAction::Rollback,
                _ => ServiceAction::Promote,
            };
            crate::service_actions::request(client, state, action, parts.get(1).copied()).await;
        }
        Some("stop-project") => cmd_stop_project(state, &parts),
        Some("deploy") => {
            state.flash("Use `orca deploy` from CLI to redeploy all services".into());
        }
        Some("filter" | "f") => cmd_filter(state, &parts),
        Some("project") => cmd_project(state, &parts),
        Some("exec") => cmd_exec(state, &parts),
        Some("sh") => cmd_sh(state, &parts),
        Some("drain") => cmd_drain(state, &parts),
        Some("undrain") => cmd_undrain(state, client, &parts).await,
        Some("secrets") => {
            crate::refresh_secrets_usage(client, state).await;
            state.selected_secret = 0;
            state.push_view(View::Secrets);
        }
        Some("set") => cmd_secret_set(state, client, cmd).await,
        Some("rm") => cmd_secret_rm(state, &parts),
        Some("webhooks") => {
            crate::refresh_webhooks(client, state).await;
            state.selected_webhook = 0;
            state.push_view(View::Webhooks);
        }
        Some("networks") => {
            crate::refresh_networks(client, state).await;
            state.push_view(View::Networks);
        }
        Some("alerts") => {
            crate::refresh_alerts(client, state).await;
            state.selected_alert = 0;
            state.push_view(View::Alerts);
        }
        Some("reply") => cmd_alert_reply(state, client, &parts).await,
        Some("dismiss") => cmd_alert_action(state, client, "dismiss").await,
        Some("resolve") => cmd_alert_action(state, client, "resolve").await,
        Some("webhook-add") => {
            crate::webhook_commands::cmd_webhook_add(state, client, &parts).await
        }
        Some("webhook-edit") => {
            crate::webhook_commands::cmd_webhook_edit(state, client, &parts).await
        }
        Some("webhook-rm") => crate::webhook_commands::cmd_webhook_rm(state, client, &parts).await,
        Some(other) => state.flash(format!("Unknown command: {other}")),
        None => {}
    }
}

/// `:reply <message...>` — operator follow-up turn on the currently-targeted
/// alert conversation. The engine appends the operator message, re-runs the
/// LLM with the full history + current context, and the AI's response shows
/// up on the next state refresh.
async fn cmd_alert_reply(state: &mut AppState, client: &ApiClient, parts: &[&str]) {
    let Some(id) = crate::ui::alerts::current_alert_id(state) else {
        state.flash(":reply must be used from the Alerts view".into());
        return;
    };
    if parts.len() < 2 {
        state.flash("Usage: :reply <message...>".into());
        return;
    }
    let message = parts[1..].join(" ");
    match client.alerts_reply(&id, &message).await {
        Ok(_) => {
            state.flash("Reply sent — waiting for AI response.".into());
            crate::refresh_alerts(client, state).await;
        }
        Err(e) => state.error = Some(format!("Reply failed: {e}")),
    }
}

/// `:dismiss` / `:resolve` — same semantics as the `d` / `R` keys, but
/// also reachable from any view via command mode.
async fn cmd_alert_action(state: &mut AppState, client: &ApiClient, action: &str) {
    let Some(id) = crate::ui::alerts::current_alert_id(state) else {
        state.flash(format!(":{action} must be used from the Alerts view"));
        return;
    };
    let result = match action {
        "dismiss" => client.alerts_dismiss(&id).await,
        "resolve" => client.alerts_resolve(&id).await,
        _ => return,
    };
    match result {
        Ok(_) => {
            state.flash(format!("Alert {action}ed."));
            crate::refresh_alerts(client, state).await;
        }
        Err(e) => state.error = Some(format!("{action} failed: {e}")),
    }
}

async fn cmd_secret_set(state: &mut AppState, client: &ApiClient, cmd: &str) {
    let Some((key, value)) = split_set(cmd) else {
        state.flash("Usage: :set <KEY> <value...>".into());
        return;
    };
    match client.set_secret(key, value).await {
        Ok(()) => {
            state.flash(format!("Secret {key} set"));
            crate::refresh_secrets_usage(client, state).await;
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

fn cmd_secret_rm(state: &mut AppState, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :rm <KEY>".into());
        return;
    }
    crate::confirm::arm(state, Confirm::DeleteSecret(parts[1].to_string()));
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
    crate::refresh_logs_named(client, state, &svc_name);
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

fn cmd_stop(state: &mut AppState, parts: &[&str]) {
    let name = if let Some(n) = parts.get(1) {
        (*n).to_string()
    } else if let Some(n) = state.selected_service_name() {
        n.to_string()
    } else {
        state.flash("Usage: :stop <service>".into());
        return;
    };
    crate::confirm::arm(state, Confirm::StopService(name));
}

fn cmd_stop_project(state: &mut AppState, parts: &[&str]) {
    if parts.len() < 2 {
        state.flash("Usage: :stop-project <project>".into());
        return;
    }
    crate::confirm::arm(state, Confirm::StopProject(parts[1].to_string()));
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

fn cmd_drain(state: &mut AppState, parts: &[&str]) {
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
    crate::confirm::arm(state, Confirm::DrainNode(node_id));
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
    undrain(client, state, node_id).await;
}

/// Undrain a node: it takes new work again. Not destructive, so no y/N.
pub(crate) async fn undrain(client: &ApiClient, state: &mut AppState, node_id: u64) {
    match client.undrain(node_id).await {
        Ok(()) => state.flash(format!("Undrained node {node_id}")),
        Err(e) => state.error = Some(format!("Undrain failed: {e}")),
    }
}

/// `set KEY VALUE...` -> (KEY, VALUE) with the value exactly as typed after
/// the single separating space. Splitting on whitespace and re-joining
/// collapsed runs of spaces and tabs inside secret values (#264).
pub(crate) fn split_set(cmd: &str) -> Option<(&str, &str)> {
    let rest = cmd.trim_start().strip_prefix("set")?;
    let rest = rest.strip_prefix(char::is_whitespace)?.trim_start();
    let (key, value) = rest.split_once(char::is_whitespace)?;
    let value = value.strip_suffix('\n').unwrap_or(value);
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::webhook_commands::build_webhook_body;

    #[test]
    fn set_keeps_the_value_exactly_as_typed() {
        // Splitting on whitespace and re-joining turned "a  b\tc" into "a b c".
        assert_eq!(split_set("set KEY a  b\tc"), Some(("KEY", "a  b\tc")));
        assert_eq!(split_set("set KEY  lead"), Some(("KEY", " lead")));
        assert_eq!(split_set("set KEY"), None);
        assert_eq!(split_set("set KEY "), None);
        assert_eq!(split_set("setx KEY v"), None);
    }

    /// Positional args land in their expected fields; flags default to off.
    /// Locks in the on-wire field names since the server's `WebhookConfig`
    /// uses `service_name` (not `service`) — easy to typo.
    #[test]
    fn build_webhook_body_positionals() {
        let body = build_webhook_body("acme/api", "main", "api", &[]);
        assert_eq!(body["repo"], "acme/api");
        assert_eq!(body["branch"], "main");
        assert_eq!(body["service_name"], "api");
        assert_eq!(body["infra"], false);
        assert!(body.get("secret").is_none());
    }

    /// `--secret X` captures the value following the flag.
    #[test]
    fn build_webhook_body_secret_flag() {
        let body = build_webhook_body("acme/api", "main", "api", &["--secret", "shhh"]);
        assert_eq!(body["secret"], "shhh");
    }

    /// `--infra` is a boolean flag with no value.
    #[test]
    fn build_webhook_body_infra_flag() {
        let body = build_webhook_body("acme/infra", "main", "infra", &["--infra"]);
        assert_eq!(body["infra"], true);
        assert!(body.get("secret").is_none());
    }

    /// Flags can appear in any order; an unknown token is ignored rather than
    /// erroring (which would be surprising mid-command for the user).
    #[test]
    fn build_webhook_body_flag_order_and_unknowns() {
        let body = build_webhook_body(
            "acme/api",
            "main",
            "api",
            &["--infra", "garbage", "--secret", "s"],
        );
        assert_eq!(body["infra"], true);
        assert_eq!(body["secret"], "s");
    }

    /// `--secret` without a value must not panic and must not set the field.
    #[test]
    fn build_webhook_body_secret_without_value() {
        let body = build_webhook_body("acme/api", "main", "api", &["--secret"]);
        assert!(body.get("secret").is_none());
    }
}
