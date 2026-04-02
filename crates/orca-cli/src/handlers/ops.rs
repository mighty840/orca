use crate::client::OrcaClient;
use crate::commands::{AlertsAction, SecretsAction, WebhookAction};

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
    if summarize {
        println!("AI log summarization not yet connected.");
        println!("Configure [ai] in cluster.toml to enable.");
    } else {
        match client.logs(&service, tail).await {
            Ok(logs) => print!("{logs}"),
            Err(e) => {
                tracing::error!("Failed to get logs for '{service}': {e}");
                std::process::exit(1);
            }
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
    orca_core::secrets::SecretStore::open("secrets.json").unwrap_or_else(|e| {
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
    // On agent nodes, fall back to saved leader URL if default API isn't reachable
    let api = if api == "http://127.0.0.1:6880" {
        let leader_file = dirs_next::home_dir()
            .unwrap_or_else(|| ".".into())
            .join(".orca/leader.url");
        if let Ok(url) = std::fs::read_to_string(&leader_file) {
            let url = url.trim().to_string();
            if !url.is_empty() {
                url
            } else {
                api.to_string()
            }
        } else {
            api.to_string()
        }
    } else {
        api.to_string()
    };
    orca_tui::run_tui(&api).await
}
pub async fn handle_web(_port: u16) -> anyhow::Result<()> {
    println!("Use `orca tui` instead.");
    Ok(())
}
