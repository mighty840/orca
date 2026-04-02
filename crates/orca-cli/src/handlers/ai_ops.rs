//! CLI handlers for AI-powered operations: ask and generate.

use crate::client::OrcaClient;

/// Handle `orca ask "question"` — sends question with cluster context to LLM.
pub async fn handle_ask(question: Vec<String>, api: String) -> anyhow::Result<()> {
    let q = question.join(" ");
    let ai_config = match load_ai_config() {
        Some(c) => c,
        None => {
            println!("No AI configuration found in cluster.toml");
            return Ok(());
        }
    };

    // Gather context: try to get cluster status and logs of degraded services
    let client = OrcaClient::new(api);
    let (status_text, logs_text) = gather_context(&client).await;

    match orca_ai::ops::ask(&ai_config, &q, &status_text, &logs_text).await {
        Ok(response) => println!("{response}"),
        Err(e) => {
            tracing::error!("AI request failed: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Handle `orca generate "description"` — generates service.toml from description.
pub async fn handle_generate(description: Vec<String>) -> anyhow::Result<()> {
    let desc = description.join(" ");
    let ai_config = match load_ai_config() {
        Some(c) => c,
        None => {
            println!("No AI configuration found in cluster.toml");
            return Ok(());
        }
    };

    match orca_ai::ops::generate(&ai_config, &desc).await {
        Ok(toml_output) => println!("{toml_output}"),
        Err(e) => {
            tracing::error!("AI request failed: {e}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Load the [ai] section from cluster.toml, searching common locations.
fn load_ai_config() -> Option<orca_core::config::AiConfig> {
    let candidates = [
        std::path::PathBuf::from("cluster.toml"),
        std::path::PathBuf::from("/etc/orca/cluster.toml"),
    ];
    for path in &candidates {
        if path.exists()
            && let Ok(config) = orca_core::config::ClusterConfig::load(path)
        {
            return config.ai;
        }
    }
    None
}

/// Gather status text and logs of degraded services for AI context.
async fn gather_context(client: &OrcaClient) -> (String, String) {
    let status_text = match client.status().await {
        Ok(resp) => {
            let mut out = format!("Cluster: {}\n", resp.cluster_name);
            for svc in &resp.services {
                out.push_str(&format!(
                    "  {} [{}] {}/{} replicas, status={}\n",
                    svc.name,
                    format!("{:?}", svc.runtime).to_lowercase(),
                    svc.running_replicas,
                    svc.desired_replicas,
                    svc.status,
                ));
            }
            out
        }
        Err(_) => String::new(),
    };

    // Fetch recent logs for degraded services
    let mut logs_text = String::new();
    if let Ok(resp) = client.status().await {
        for svc in &resp.services {
            if (svc.status != "running" || svc.running_replicas < svc.desired_replicas)
                && let Ok(logs) = client.logs(&svc.name, 20).await
            {
                logs_text.push_str(&format!("--- {} ---\n{}\n", svc.name, logs));
            }
        }
    }

    (status_text, logs_text)
}
