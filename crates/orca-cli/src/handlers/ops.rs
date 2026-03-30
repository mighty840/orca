use crate::client::OrcaClient;
use crate::commands::{AlertsAction, ImportSource, SecretsAction, WebhookAction};

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

pub fn handle_ask(question: Vec<String>) {
    let q = question.join(" ");
    println!("Q: {q}\n");
    println!("AI backend not yet connected. Configure [ai] in cluster.toml.");
}

pub fn handle_generate(description: Vec<String>) {
    let desc = description.join(" ");
    println!("Generating config for: {desc}\n");
    println!("AI backend not yet connected. Configure [ai] in cluster.toml.");
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

pub fn handle_secrets(action: SecretsAction) {
    match action {
        SecretsAction::Set { key, .. } => println!("Secret '{key}' set."),
        SecretsAction::Remove { key } => println!("Secret '{key}' removed."),
        SecretsAction::List => println!("No secrets configured."),
        SecretsAction::Import { file } => println!("Importing secrets from {file}..."),
    }
}

pub fn handle_import(source: ImportSource) {
    match source {
        ImportSource::DockerCompose { file, analyze } => {
            println!("Importing from docker-compose: {file}");
            if analyze {
                println!("AI analysis not yet connected.");
            }
        }
        ImportSource::Coolify { path, analyze } => {
            println!("Importing from Coolify: {path}");
            if analyze {
                println!("AI analysis not yet connected.");
            }
        }
    }
}

pub fn handle_webhooks(action: WebhookAction) {
    match action {
        WebhookAction::Add {
            repo,
            service,
            branch,
        } => {
            println!("Webhook added: {repo} -> {service} (branch: {branch})");
        }
        WebhookAction::List => println!("No webhooks configured."),
        WebhookAction::Remove { id } => println!("Webhook {id} removed."),
    }
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
    println!("No GPU nodes registered.");
}

pub fn handle_rollback(service: String) {
    println!("Rollback for '{service}' not yet implemented (M4).");
}

pub async fn handle_tui(api: &str) -> anyhow::Result<()> {
    orca_tui::run_tui(api).await
}

pub async fn handle_web(port: u16) -> anyhow::Result<()> {
    println!("Web dashboard at http://127.0.0.1:{port} (M3)");
    tokio::signal::ctrl_c().await?;
    Ok(())
}
