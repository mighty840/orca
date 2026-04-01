//! Coolify importer: scans Coolify's data directory for docker-compose files
//! and converts them to orca ServicesConfig using the compose parser.

use std::path::Path;

use tracing::{info, warn};

use crate::config::ServicesConfig;
use crate::import::compose;

/// Known subdirectories where Coolify stores docker-compose files.
const COMPOSE_DIRS: &[&str] = &["services", "applications", "compose"];

/// Scan a Coolify data directory and import all docker-compose files found.
///
/// Returns a merged `ServicesConfig` containing all discovered services.
pub fn import_coolify_dir(coolify_path: &Path) -> anyhow::Result<ServicesConfig> {
    let mut all_services = Vec::new();
    let mut found_any = false;

    // Try known subdirectories
    for subdir in COMPOSE_DIRS {
        let dir = coolify_path.join(subdir);
        if dir.is_dir() {
            scan_dir_for_compose(&dir, &mut all_services, &mut found_any)?;
        }
    }

    // Also scan root for any compose files
    scan_dir_for_compose(coolify_path, &mut all_services, &mut found_any)?;

    if !found_any {
        anyhow::bail!(
            "No docker-compose files found in {}",
            coolify_path.display()
        );
    }

    info!("Imported {} services from Coolify", all_services.len());
    Ok(ServicesConfig {
        service: all_services,
    })
}

/// Recursively scan a directory (one level deep) for docker-compose files.
fn scan_dir_for_compose(
    dir: &Path,
    services: &mut Vec<crate::config::ServiceConfig>,
    found: &mut bool,
) -> anyhow::Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Check direct compose files
        if is_compose_file(&path) {
            import_single_compose(&path, services, found);
            continue;
        }

        // Check one level of subdirectories
        if path.is_dir() {
            for sub_entry in std::fs::read_dir(&path).into_iter().flatten().flatten() {
                let sub_path = sub_entry.path();
                if is_compose_file(&sub_path) {
                    import_single_compose(&sub_path, services, found);
                }
            }
        }
    }
    Ok(())
}

/// Check if a path looks like a docker-compose file.
fn is_compose_file(path: &Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    matches!(
        name,
        "docker-compose.yml" | "docker-compose.yaml" | "compose.yml" | "compose.yaml"
    )
}

/// Import a single compose file, appending services to the vec.
fn import_single_compose(
    path: &Path,
    services: &mut Vec<crate::config::ServiceConfig>,
    found: &mut bool,
) {
    match compose::parse_compose_file(path) {
        Ok(config) => {
            info!(
                "Imported {} services from {}",
                config.service.len(),
                path.display()
            );
            services.extend(config.service);
            *found = true;
        }
        Err(e) => {
            warn!("Failed to parse {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_compose_file_check() {
        assert!(is_compose_file(Path::new("/foo/docker-compose.yml")));
        assert!(is_compose_file(Path::new("/foo/docker-compose.yaml")));
        assert!(is_compose_file(Path::new("/foo/compose.yml")));
        assert!(!is_compose_file(Path::new("/foo/config.toml")));
        assert!(!is_compose_file(Path::new("/foo/Dockerfile")));
    }

    #[test]
    fn import_coolify_missing_dir() {
        let result = import_coolify_dir(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }

    #[test]
    fn import_coolify_with_compose_file() {
        let dir = tempfile::tempdir().unwrap();
        let services_dir = dir.path().join("services");
        std::fs::create_dir(&services_dir).unwrap();
        let project_dir = services_dir.join("my-app");
        std::fs::create_dir(&project_dir).unwrap();

        let compose_content = r#"
services:
  web:
    image: nginx:latest
    ports:
      - "80:80"
"#;
        std::fs::write(project_dir.join("docker-compose.yml"), compose_content).unwrap();

        let config = import_coolify_dir(dir.path()).unwrap();
        assert_eq!(config.service.len(), 1);
        assert_eq!(config.service[0].name, "web");
    }
}
