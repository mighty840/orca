use std::path::Path;

use crate::client::OrcaClient;

pub async fn handle_deploy(
    file: &str,
    service_filter: Option<Vec<String>>,
    api: String,
) -> anyhow::Result<()> {
    // Resolve services path: use explicit --file, or walk up to find services/
    let resolved_path = if Path::new(file).exists() {
        file.to_string()
    } else if let Some(orca_dir) = crate::handlers::ops::find_orca_dir() {
        let candidate = orca_dir.join(file);
        if candidate.exists() {
            candidate.display().to_string()
        } else {
            file.to_string()
        }
    } else {
        file.to_string()
    };
    let path = Path::new(&resolved_path);

    let mut config = if path.is_dir() {
        orca_core::config::ServicesConfig::load_dir(path)?
    } else {
        orca_core::config::ServicesConfig::load(path)?
    };

    // Filter to specified services if any
    if let Some(names) = &service_filter {
        let before = config.service.len();
        config.service.retain(|s| names.contains(&s.name));
        if config.service.is_empty() {
            anyhow::bail!(
                "no matching services found for {:?} (scanned {before} in '{file}')",
                names
            );
        }
    }

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
