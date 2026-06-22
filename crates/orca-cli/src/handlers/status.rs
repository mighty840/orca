use crate::client::OrcaClient;

pub async fn handle_status(api: String) -> anyhow::Result<()> {
    let client = OrcaClient::new(api);
    match client.status().await {
        Ok(resp) => {
            println!("Cluster: {}", resp.cluster_name);
            println!();
            if resp.services.is_empty() {
                println!("No services deployed.");
            } else {
                let header = format!(
                    "{:<20} {:<12} {:<10} {:<10} {:<20}",
                    "SERVICE", "RUNTIME", "REPLICAS", "STATUS", "DOMAIN"
                );
                println!("{header}");
                for svc in &resp.services {
                    let domain = if svc.domains.is_empty() {
                        svc.domain.clone().unwrap_or_else(|| "-".to_string())
                    } else {
                        svc.domains.join(", ")
                    };
                    println!(
                        "{:<20} {:<12} {}/{:<7} {:<10} {}",
                        svc.name,
                        format!("{:?}", svc.runtime).to_lowercase(),
                        svc.running_replicas,
                        svc.desired_replicas,
                        svc.status,
                        domain
                    );
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to get status: {e}");
            tracing::error!("Is `orca server` running?");
            std::process::exit(1);
        }
    }

    Ok(())
}
