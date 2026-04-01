//! Docker-compose.yml parser that converts compose services into orca ServiceConfig.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::config::{ServiceConfig, ServicesConfig};
use crate::types::Replicas;

/// Top-level docker-compose.yml structure (subset we care about).
#[derive(Debug, Deserialize)]
pub struct ComposeFile {
    #[serde(default)]
    pub services: HashMap<String, ComposeService>,
}

/// A single service in docker-compose.yml.
#[derive(Debug, Deserialize)]
pub struct ComposeService {
    pub image: Option<String>,
    #[serde(default)]
    pub ports: Vec<String>,
    #[serde(default)]
    pub environment: ComposeEnv,
    #[serde(default)]
    pub volumes: Vec<String>,
    #[serde(default)]
    pub depends_on: ComposeDependsOn,
}

/// Environment can be a list of "KEY=VALUE" strings or a map.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum ComposeEnv {
    #[default]
    None,
    List(Vec<String>),
    Map(HashMap<String, serde_yaml::Value>),
}

/// depends_on can be a list of strings or a map with condition keys.
#[derive(Debug, Deserialize, Default)]
#[serde(untagged)]
pub enum ComposeDependsOn {
    #[default]
    None,
    List(Vec<String>),
    Map(HashMap<String, serde_yaml::Value>),
}

/// Parse a docker-compose.yml file and return orca ServicesConfig.
pub fn parse_compose_file(path: &Path) -> anyhow::Result<ServicesConfig> {
    let content = std::fs::read_to_string(path)?;
    parse_compose_str(&content, path)
}

/// Parse docker-compose.yml content string into orca ServicesConfig.
pub fn parse_compose_str(content: &str, path: &Path) -> anyhow::Result<ServicesConfig> {
    let compose: ComposeFile = serde_yaml::from_str(content)?;
    let network_name = derive_network_name(path);
    let services = compose
        .services
        .into_iter()
        .map(|(name, svc)| convert_service(&name, &svc, &network_name))
        .collect();
    Ok(ServicesConfig { service: services })
}

/// Derive a network name from the compose file path or directory name.
fn derive_network_name(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("default")
        .to_string()
}

/// Convert a single compose service to an orca ServiceConfig.
fn convert_service(name: &str, svc: &ComposeService, network: &str) -> ServiceConfig {
    let (port, mounts_from_ports) = parse_ports(&svc.ports);
    let env = parse_env(&svc.environment);
    let aliases = vec![name.to_string()];

    ServiceConfig {
        name: name.to_string(),
        runtime: Default::default(),
        image: svc.image.clone(),
        module: None,
        replicas: Replicas::Fixed(1),
        port,
        host_port: mounts_from_ports,
        domain: None,
        routes: vec![],
        health: None,
        readiness: None,
        liveness: None,
        env,
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: Some(network.to_string()),
        aliases,
        mounts: svc.volumes.clone(),
        triggers: vec![],
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
    }
}

/// Parse compose port mappings. Returns (container_port, host_port).
fn parse_ports(ports: &[String]) -> (Option<u16>, Option<u16>) {
    for port_str in ports {
        // Handle "host:container" or just "container"
        let stripped = port_str.split('/').next().unwrap_or(port_str);
        if let Some((host, container)) = stripped.split_once(':') {
            let container_port = container.parse::<u16>().ok();
            let host_port = host.parse::<u16>().ok();
            if container_port.is_some() {
                return (container_port, host_port);
            }
        } else if let Ok(p) = stripped.parse::<u16>() {
            return (Some(p), None);
        }
    }
    (None, None)
}

/// Parse compose environment variables into a HashMap.
fn parse_env(env: &ComposeEnv) -> HashMap<String, String> {
    match env {
        ComposeEnv::None => HashMap::new(),
        ComposeEnv::List(list) => list
            .iter()
            .filter_map(|entry| {
                let (k, v) = entry.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect(),
        ComposeEnv::Map(map) => map
            .iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_yaml::Value::String(s) => s.clone(),
                    serde_yaml::Value::Null => String::new(),
                    other => format!("{other:?}"),
                };
                (k.clone(), val)
            })
            .collect(),
    }
}

/// Serialize a ServicesConfig to TOML string for writing services.toml.
pub fn services_to_toml(config: &ServicesConfig) -> anyhow::Result<String> {
    Ok(toml::to_string_pretty(config)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_compose() {
        let yaml = r#"
services:
  web:
    image: nginx:latest
    ports:
      - "8080:80"
    environment:
      - FOO=bar
  db:
    image: postgres:16
    ports:
      - "5432:5432"
    environment:
      POSTGRES_PASSWORD: secret
    volumes:
      - pgdata:/var/lib/postgresql/data
"#;
        let path = Path::new("/tmp/myproject/docker-compose.yml");
        let config = parse_compose_str(yaml, path).unwrap();
        assert_eq!(config.service.len(), 2);

        let web = config.service.iter().find(|s| s.name == "web").unwrap();
        assert_eq!(web.image.as_deref(), Some("nginx:latest"));
        assert_eq!(web.port, Some(80));
        assert_eq!(web.host_port, Some(8080));
        assert_eq!(web.env.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(web.network.as_deref(), Some("myproject"));
        assert!(web.aliases.contains(&"web".to_string()));

        let db = config.service.iter().find(|s| s.name == "db").unwrap();
        assert_eq!(db.image.as_deref(), Some("postgres:16"));
        assert_eq!(db.port, Some(5432));
        assert_eq!(
            db.env.get("POSTGRES_PASSWORD").map(String::as_str),
            Some("secret")
        );
    }

    #[test]
    fn parse_compose_no_ports() {
        let yaml = r#"
services:
  worker:
    image: myapp/worker:latest
    environment:
      - QUEUE=default
"#;
        let path = Path::new("/projects/app/docker-compose.yml");
        let config = parse_compose_str(yaml, path).unwrap();
        assert_eq!(config.service.len(), 1);
        let worker = &config.service[0];
        assert_eq!(worker.port, None);
        assert_eq!(worker.host_port, None);
    }

    #[test]
    fn services_to_toml_roundtrip() {
        let config = ServicesConfig {
            service: vec![ServiceConfig {
                name: "test".to_string(),
                runtime: Default::default(),
                image: Some("nginx:latest".to_string()),
                module: None,
                replicas: Replicas::Fixed(1),
                port: Some(80),
                host_port: None,
                domain: None,
                routes: vec![],
                health: None,
                readiness: None,
                liveness: None,
                env: HashMap::new(),
                resources: None,
                volume: None,
                deploy: None,
                placement: None,
                network: Some("default".to_string()),
                aliases: vec!["test".to_string()],
                mounts: vec![],
                triggers: vec![],
                assets: None,
                build: None,
                tls_cert: None,
                tls_key: None,
                internal: false,
            }],
        };
        let toml_str = services_to_toml(&config).unwrap();
        assert!(toml_str.contains("nginx:latest"));
    }
}
