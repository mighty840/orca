use std::path::Path;

use crate::client::OrcaClient;

pub async fn handle_deploy(file: &str, api: String) -> anyhow::Result<()> {
    let path = Path::new(file);

    let config = if path.is_dir() {
        // Directory mode: scan subdirs for service.toml
        orca_core::config::ServicesConfig::load_dir(path)?
    } else {
        orca_core::config::ServicesConfig::load(path)?
    };

    let client = OrcaClient::new(api);

    println!("Deploying {} services...", config.service.len());
    match client.deploy(&config).await {
        Ok(resp) => {
            for name in &resp.deployed {
                println!("  + {name}");
            }
            for err in &resp.errors {
                tracing::warn!("Deploy error: {err}");
            }
            println!(
                "Deployed: {}, Errors: {}",
                resp.deployed.len(),
                resp.errors.len()
            );
        }
        Err(e) => {
            tracing::error!("Deploy failed: {e}");
            tracing::error!("Is `orca server` running?");
            std::process::exit(1);
        }
    }

    Ok(())
}
