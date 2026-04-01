//! Handler for the `orca build` CLI command.

use orca_agent::builder::DockerBuilder;
use orca_core::config::ServicesConfig;

/// Build Docker images from source for one or all services.
pub async fn handle_build(file: &str, service: Option<String>) -> anyhow::Result<()> {
    let config = ServicesConfig::load(file.as_ref())?;
    let builder = DockerBuilder::default_dir()?;

    let services: Vec<_> = config
        .service
        .iter()
        .filter(|s| s.build.is_some())
        .filter(|s| service.as_ref().is_none_or(|name| &s.name == name))
        .collect();

    if services.is_empty() {
        if let Some(name) = &service {
            anyhow::bail!("service '{name}' not found or has no build config");
        }
        println!("No services with build config found in {file}");
        return Ok(());
    }

    for svc in &services {
        let build_config = svc.build.as_ref().expect("filtered above");
        println!("Building {}...", svc.name);
        match builder.build_service(build_config, &svc.name).await {
            Ok(tag) => println!("  Built: {tag}"),
            Err(e) => {
                tracing::error!("Build failed for {}: {e}", svc.name);
                println!("  FAILED: {e}");
            }
        }
    }

    Ok(())
}
