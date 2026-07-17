use std::path::PathBuf;

use crate::client::OrcaClient;
use crate::commands::{AlertsAction, WebhookAction};

/// Find the orca project directory by walking up from `base` looking for
/// `cluster.toml` or `services/`. Falls back to `~/.orca/`.
pub fn find_orca_dir_from(base: &std::path::Path) -> Option<PathBuf> {
    let mut dir = base.to_path_buf();
    loop {
        if dir.join("cluster.toml").exists() || dir.join("services").is_dir() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }
    let home_orca = dirs_next::home_dir()?.join("orca");
    if home_orca.join("cluster.toml").exists() {
        return Some(home_orca);
    }
    None
}

/// Find the orca project directory starting from the current working directory.
pub fn find_orca_dir() -> Option<PathBuf> {
    find_orca_dir_from(&std::env::current_dir().ok()?)
}

/// Resolve the API URL: on agent nodes, fall back to the saved leader URL
/// if the default localhost:6880 isn't reachable.
pub fn resolve_api(api: &str) -> String {
    if api != "http://127.0.0.1:6880" {
        return api.to_string();
    }
    let leader_file = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/leader.url");
    if let Ok(url) = std::fs::read_to_string(&leader_file) {
        let url = url.trim().to_string();
        if !url.is_empty() {
            return url;
        }
    }
    api.to_string()
}

pub async fn handle_stop(service: Option<String>, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match service {
        Some(name) => {
            client.stop(&name).await?;
            println!("Paused service: {name} (still defined; `orca start {name}` to resume)");
        }
        None => {
            client.stop_all().await?;
            println!("Paused all services.");
        }
    }
    Ok(())
}

/// Resume a paused service to its configured replica count.
pub async fn handle_start(service: &str, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.start(service).await?;
    println!("Started service: {service}");
    Ok(())
}

pub async fn handle_logs(
    service: String,
    tail: u64,
    summarize: bool,
    api: String,
) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match client.logs(&service, tail).await {
        Ok(logs) => {
            if summarize {
                let ai_config = crate::handlers::ai_ops::load_ai_config();
                match ai_config {
                    Some(config) => {
                        let prompt = format!(
                            "Analyze and summarize these logs for the service '{service}'. \
                             Highlight errors, warnings, and anomalies:\n\n{logs}"
                        );
                        match orca_ai::ops::ask(&config, &prompt, "", "").await {
                            Ok(summary) => println!("{summary}"),
                            Err(e) => {
                                tracing::error!("AI summarization failed: {e}");
                                print!("{logs}");
                            }
                        }
                    }
                    None => {
                        println!("No AI configuration found. Configure [ai] in cluster.toml.");
                        print!("{logs}");
                    }
                }
            } else {
                print!("{logs}");
            }
        }
        Err(e) => {
            tracing::error!("Failed to get logs for '{service}': {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn handle_scale(service: String, replicas: u32, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match client.scale(&service, replicas).await {
        Ok(resp) => {
            println!("Scaled {} to {} replicas", resp.service, resp.replicas);
        }
        Err(e) => {
            tracing::error!("Failed to scale '{service}': {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

pub async fn handle_alerts(action: AlertsAction, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match action {
        AlertsAction::List { all } => {
            let alerts = client.alerts_list(all).await?;
            if alerts.is_empty() {
                let scope = if all { "all" } else { "active" };
                println!("No {scope} alert conversations.");
                return Ok(());
            }
            println!(
                "{:<36}  {:<20}  {:<9}  {:<16}  Started",
                "ID", "Service", "Severity", "State"
            );
            for a in alerts {
                println!(
                    "{:<36}  {:<20}  {:<9?}  {:<16?}  {}",
                    a.id,
                    truncate(&a.service, 20),
                    a.severity,
                    a.state,
                    a.started_at.format("%Y-%m-%d %H:%M:%S")
                );
            }
        }
        AlertsAction::View { id } => {
            let conv = client.alerts_view(&id).await?;
            print_conversation(&conv);
        }
        AlertsAction::Reply { id, message } => {
            let msg = message.join(" ");
            if msg.trim().is_empty() {
                anyhow::bail!("reply message cannot be empty");
            }
            let conv = client.alerts_reply(&id, &msg).await?;
            println!("Reply sent. Latest exchange:");
            print_latest_exchange(&conv);
        }
        AlertsAction::Dismiss { id } => {
            client.alerts_dismiss(&id).await?;
            println!("Dismissed alert {id}.");
        }
        AlertsAction::Resolve { id } => {
            client.alerts_resolve(&id).await?;
            println!("Resolved alert {id}.");
        }
        AlertsAction::Fix { id } => {
            let conv = client.alerts_view(&id).await?;
            let cmd = conv
                .messages
                .iter()
                .rev()
                .find_map(|m| m.suggested_command.as_deref());
            match cmd {
                Some(c) => {
                    println!("Suggested fix for alert {id}:\n  {c}");
                    println!(
                        "\nReview before running. To apply: `orca alerts reply {id} approve` once interactive\n\
                         confirm is wired, or run the command directly."
                    );
                }
                None => println!("No suggested command on alert {id}."),
            }
        }
    }
    Ok(())
}

fn print_conversation(conv: &orca_core::types::AlertConversation) {
    println!("Alert {} — {}", conv.id, conv.service);
    println!(
        "Severity: {:?}  State: {:?}  Started: {}",
        conv.severity,
        conv.state,
        conv.started_at.format("%Y-%m-%d %H:%M:%S")
    );
    if let Some(resolved) = conv.resolved_at {
        println!("Resolved: {}", resolved.format("%Y-%m-%d %H:%M:%S"));
    }
    println!("\nConversation:");
    for msg in &conv.messages {
        let who = match msg.sender {
            orca_core::types::AlertSender::Orca => "orca",
            orca_core::types::AlertSender::Operator => "you",
            orca_core::types::AlertSender::System => "system",
        };
        println!(
            "  [{} {}] {}",
            msg.timestamp.format("%H:%M:%S"),
            who,
            msg.content
        );
        if let Some(cmd) = &msg.suggested_command {
            println!("    fix: {cmd}");
        }
    }
}

fn print_latest_exchange(conv: &orca_core::types::AlertConversation) {
    for msg in conv.messages.iter().rev().take(2).rev() {
        let who = match msg.sender {
            orca_core::types::AlertSender::Orca => "orca",
            orca_core::types::AlertSender::Operator => "you",
            orca_core::types::AlertSender::System => "system",
        };
        println!("  [{who}] {}", msg.content);
        if let Some(cmd) = &msg.suggested_command {
            println!("    fix: {cmd}");
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

/// Generate a random 32-byte hex secret for webhook HMAC signing.
fn generate_secret() -> String {
    format!("{:x}{:x}", rand::random::<u128>(), rand::random::<u128>())
}

pub async fn handle_webhooks(action: WebhookAction, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match action {
        WebhookAction::Add {
            repo,
            service,
            branch,
            secret,
            infra,
        } => {
            let (final_secret, generated) = match secret {
                Some(s) => (s, false),
                None => (generate_secret(), true),
            };
            client
                .add_webhook(&repo, &service, &branch, &final_secret, infra)
                .await?;
            println!("Webhook registered: {repo} -> {service} (branch: {branch})");
            if generated {
                println!();
                println!("Generated secret (save this — it won't be shown again):");
                println!("  {final_secret}");
            }
        }
        WebhookAction::List => {
            let resp = client.list_webhooks().await?;
            let webhooks = resp["webhooks"].as_array();
            match webhooks {
                Some(hooks) if hooks.is_empty() => println!("No webhooks configured."),
                Some(hooks) => {
                    let header = format!("{:<30} {:<20} {:<10}", "REPO", "SERVICE", "BRANCH");
                    println!("{header}");
                    for h in hooks {
                        println!(
                            "{:<30} {:<20} {:<10}",
                            h["repo"].as_str().unwrap_or("-"),
                            h["service_name"].as_str().unwrap_or("-"),
                            h["branch"].as_str().unwrap_or("-"),
                        );
                    }
                }
                None => println!("No webhooks configured."),
            }
        }
        WebhookAction::Remove { id } => {
            client.remove_webhook(&id).await?;
            println!("Webhook removed for service: {id}");
        }
    }
    Ok(())
}

pub async fn handle_nodes(_gpus: bool, api: String) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    match client
        .get(format!("{}/api/v1/cluster/info", api.trim_end_matches('/')))
        .send()
        .await
    {
        Ok(resp) => {
            let json: serde_json::Value = resp.json().await?;
            println!("Cluster: {}", json["cluster_name"]);
            let nodes = json["nodes"].as_array();
            if let Some(nodes) = nodes {
                if nodes.is_empty() {
                    println!("No nodes registered.");
                } else {
                    let header = format!("{:<20} {:<25} {:<10}", "NODE ID", "ADDRESS", "STATUS");
                    println!("{header}");
                    for n in nodes {
                        println!(
                            "{:<20} {:<25} {:<10}",
                            n["node_id"], n["address"], n["last_heartbeat"]
                        );
                    }
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get cluster info: {e}");
            tracing::error!("Is `orca server` running?");
        }
    }
    Ok(())
}

pub fn handle_gpus() {
    println!("GPU monitoring: use `orca nodes --gpus`");
}

pub async fn handle_redeploy(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.redeploy(&service).await?;
    println!("Redeployed service: {service}");
    Ok(())
}

pub async fn handle_rollback(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.rollback(&service).await?;
    println!("Rolled back service: {service}");
    Ok(())
}

pub async fn handle_promote(service: String, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    client.promote(&service).await?;
    println!("Promoted canary to stable for: {service}");
    Ok(())
}

pub async fn handle_tui(api: &str) -> anyhow::Result<()> {
    let api = resolve_api(api);
    orca_tui::run_tui(&api).await
}
pub async fn handle_web(_port: u16) -> anyhow::Result<()> {
    println!("Use `orca tui` instead.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_orca_dir_finds_cluster_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("cluster.toml"),
            "[cluster]\nname=\"test\"\n",
        )
        .unwrap();
        let sub = dir.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_orca_dir_from(&sub), Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_orca_dir_finds_services_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("services")).unwrap();
        assert_eq!(
            find_orca_dir_from(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }

    #[test]
    fn find_orca_dir_returns_none_when_nothing_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(
            find_orca_dir_from(dir.path()),
            Some(dir.path().to_path_buf())
        );
    }
}
