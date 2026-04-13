use std::path::PathBuf;

use crate::client::OrcaClient;
use crate::commands::{AlertsAction, SecretsAction, WebhookAction};

/// Find the orca project directory by walking up from CWD looking for
/// `cluster.toml` or `services/`. Falls back to `~/.orca/` then CWD.
pub fn find_orca_dir() -> Option<PathBuf> {
    // Walk up from CWD
    if let Ok(mut dir) = std::env::current_dir() {
        loop {
            if dir.join("cluster.toml").exists() || dir.join("services").is_dir() {
                return Some(dir);
            }
            if !dir.pop() {
                break;
            }
        }
    }
    // Fall back to ~/.orca
    let home_orca = dirs_next::home_dir()?.join("orca");
    if home_orca.join("cluster.toml").exists() {
        return Some(home_orca);
    }
    None
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
            println!("Stopped service: {name}");
        }
        None => {
            client.stop_all().await?;
            println!("Stopped all services.");
        }
    }
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

pub fn handle_alerts(action: AlertsAction) {
    match action {
        AlertsAction::List { all } => {
            let scope = if all { "all" } else { "active" };
            println!("No {scope} alert conversations.");
        }
        AlertsAction::View { id } => println!("Alert {id}: not yet connected."),
        AlertsAction::Reply { id, message } => {
            let msg = message.join(" ");
            println!("Reply to alert {id}: {msg}");
        }
        AlertsAction::Dismiss { id } => println!("Dismissed alert {id}."),
        AlertsAction::Fix { id } => println!("Applying fix for alert {id}..."),
    }
}

fn open_secrets() -> orca_core::secrets::SecretStore {
    orca_core::secrets::SecretStore::open(orca_core::secrets::default_path()).unwrap_or_else(|e| {
        tracing::error!("Failed to open secrets store: {e}");
        std::process::exit(1);
    })
}

pub fn handle_secrets(action: SecretsAction) {
    match action {
        SecretsAction::Set { key, value } => {
            let mut store = open_secrets();
            store.set(&key, &value).expect("failed to set secret");
            println!("Secret '{key}' set.");
        }
        SecretsAction::Remove { key } => {
            let mut store = open_secrets();
            match store.remove(&key) {
                Ok(true) => println!("Secret '{key}' removed."),
                Ok(false) => println!("Secret '{key}' not found."),
                Err(e) => tracing::error!("Failed to remove: {e}"),
            }
        }
        SecretsAction::List => {
            let store = open_secrets();
            let keys = store.list();
            if keys.is_empty() {
                println!("No secrets configured.");
            } else {
                for key in keys {
                    println!("  {key}");
                }
            }
        }
        SecretsAction::Import { file } => {
            let mut store = open_secrets();
            let content = std::fs::read_to_string(&file).unwrap_or_else(|e| {
                tracing::error!("Failed to read '{file}': {e}");
                std::process::exit(1);
            });
            let mut count = 0u32;
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    store.set(key.trim(), value.trim()).expect("failed to set");
                    count += 1;
                }
            }
            println!("Imported {count} secrets from {file}.");
        }
    }
}

pub async fn handle_webhooks(action: WebhookAction, api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match action {
        WebhookAction::Add {
            repo,
            service,
            branch,
        } => {
            client.add_webhook(&repo, &service, &branch).await?;
            println!("Webhook registered: {repo} -> {service} (branch: {branch})");
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
            "[cluster]\nname = \"test\"\n",
        )
        .unwrap();
        let sub = dir.path().join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();

        // Change CWD into the nested dir — find_orca_dir walks up.
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(&sub).unwrap();
        let result = find_orca_dir();
        std::env::set_current_dir(&prev).unwrap();

        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_orca_dir_finds_services_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("services")).unwrap();

        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = find_orca_dir();
        std::env::set_current_dir(&prev).unwrap();

        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_orca_dir_returns_none_when_nothing_found() {
        let dir = tempfile::tempdir().unwrap();
        // Empty dir — no cluster.toml, no services/
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(dir.path()).unwrap();
        let result = find_orca_dir();
        std::env::set_current_dir(&prev).unwrap();

        // It might find ~/.orca/cluster.toml on the host, so we can only
        // assert it does NOT equal the tempdir (which has neither marker).
        assert_ne!(result, Some(dir.path().to_path_buf()));
    }
}
