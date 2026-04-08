//! [`ContainerRuntime`] struct and core helper methods.

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{ListContainersOptions, RemoveContainerOptions, StopContainerOptions};
use bollard::image::CreateImageOptions;
use bollard::network::{ConnectNetworkOptions, CreateNetworkOptions};
use futures_util::StreamExt;
use tracing::{debug, info, warn};

use orca_core::error::{OrcaError, Result};

use super::ORCA_LABEL;
use base64::Engine;
use bollard::auth::DockerCredentials;
use bollard::container::InspectContainerOptions;

/// Read registry credentials for an image's host from `~/.docker/config.json`.
///
/// Returns `None` if the image is on Docker Hub, the config file is missing,
/// or the host has no entry. Used to authenticate pulls from private registries.
fn registry_credentials(image: &str) -> Option<DockerCredentials> {
    let host = image.split_once('/').and_then(|(h, _)| {
        if h.contains('.') || h.contains(':') {
            Some(h.to_string())
        } else {
            None
        }
    })?;
    let path = std::env::var("HOME")
        .ok()
        .map(|h| format!("{h}/.docker/config.json"))?;
    let raw = std::fs::read_to_string(path).ok()?;
    let cfg: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let auth_b64 = cfg.get("auths")?.get(&host)?.get("auth")?.as_str()?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(auth_b64)
        .ok()?;
    let creds = String::from_utf8(decoded).ok()?;
    let (user, pass) = creds.split_once(':')?;
    Some(DockerCredentials {
        username: Some(user.to_string()),
        password: Some(pass.to_string()),
        ..Default::default()
    })
}

/// Docker container runtime.
pub struct ContainerRuntime {
    pub(crate) docker: Docker,
}

/// One entry returned by [`ContainerRuntime::list_local_routes`] — enough
/// state for a node-local proxy to add a `RouteTarget` without any
/// central coordination.
#[derive(Debug, Clone)]
pub struct LocalRoute {
    pub service: String,
    pub domain: String,
    pub host_port: u16,
    pub routes: Vec<String>,
    pub strip_prefix: Option<String>,
}

impl ContainerRuntime {
    /// Connect to the local Docker daemon.
    ///
    /// # Errors
    ///
    /// Returns an error if the Docker socket is not available.
    pub fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| OrcaError::Runtime(format!("failed to connect to Docker: {e}")))?;
        Ok(Self { docker })
    }

    /// Pull an image if it does not exist locally.
    pub(crate) async fn ensure_image(&self, image: &str) -> Result<()> {
        // For mutable tags (`:latest` or no tag), always pull to pick up
        // upstream updates. For pinned tags (e.g. `:1.2.3` or `:sha`),
        // skip the pull if the image is already present locally.
        let tag = image.rsplit_once(':').map(|(_, t)| t).unwrap_or("latest");
        let is_mutable = tag == "latest";
        if !is_mutable && self.docker.inspect_image(image).await.is_ok() {
            debug!("Image {image} already available locally");
            return Ok(());
        }

        info!("Pulling image: {image}");
        let opts = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };

        let creds = registry_credentials(image);
        let mut stream = self.docker.create_image(Some(opts), None, creds);
        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = &info.status {
                        debug!("Pull {image}: {status}");
                    }
                }
                Err(e) => {
                    return Err(OrcaError::Runtime(format!(
                        "failed to pull image {image}: {e}"
                    )));
                }
            }
        }

        info!("Image pulled: {image}");
        Ok(())
    }

    /// Get a container's IP address on a specific Docker network.
    pub async fn get_container_ip(
        &self,
        container_id: &str,
        network: &str,
    ) -> Result<Option<String>> {
        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| OrcaError::Runtime(format!("inspect failed: {e}")))?;

        let ip = info
            .network_settings
            .and_then(|ns| ns.networks)
            .and_then(|nets| nets.get(network).cloned())
            .and_then(|net| net.ip_address)
            .filter(|ip| !ip.is_empty());

        Ok(ip)
    }

    /// Inspect a container and return its assigned host port for the primary port.
    pub async fn get_host_port(
        &self,
        container_id: &str,
        container_port: u16,
    ) -> Result<Option<u16>> {
        let info = self
            .docker
            .inspect_container(container_id, None::<InspectContainerOptions>)
            .await
            .map_err(|e| OrcaError::Runtime(format!("inspect failed: {e}")))?;

        let port_key = format!("{container_port}/tcp");
        let host_port = info
            .network_settings
            .and_then(|ns| ns.ports)
            .and_then(|ports| ports.get(&port_key).cloned())
            .and_then(|bindings| bindings)
            .and_then(|bindings| bindings.into_iter().next())
            .and_then(|b| b.host_port)
            .and_then(|p| p.parse::<u16>().ok());

        Ok(host_port)
    }

    /// Scan all orca-managed containers and return one entry per
    /// `(service_name, domain, host_port, routes, strip_prefix)` for any
    /// container that has an `orca.domain` + host-port binding.
    /// Used by node-local proxies to bootstrap their route table from
    /// container metadata alone — no central state.
    pub async fn list_local_routes(&self) -> Result<Vec<LocalRoute>> {
        let mut filters = HashMap::new();
        filters.insert("label", vec![ORCA_LABEL]);
        let opts = ListContainersOptions {
            all: false,
            filters,
            ..Default::default()
        };
        let containers = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| OrcaError::Runtime(format!("list containers failed: {e}")))?;

        let mut routes = Vec::new();
        for c in containers {
            let id = match c.id {
                Some(id) => id,
                None => continue,
            };
            let labels = match &c.labels {
                Some(l) => l,
                None => continue,
            };
            let domain = match labels.get("orca.domain") {
                Some(d) => d.clone(),
                None => continue,
            };
            let port = match labels.get("orca.port").and_then(|p| p.parse::<u16>().ok()) {
                Some(p) => p,
                None => continue,
            };
            let service = labels
                .get("orca.service")
                .cloned()
                .unwrap_or_else(|| domain.clone());
            let route_patterns: Vec<String> = labels
                .get("orca.routes")
                .map(|s| {
                    s.split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
            let strip_prefix = labels.get("orca.strip_prefix").cloned();
            // Resolve the dynamic host port assignment for this container's port.
            if let Ok(Some(host_port)) = self.get_host_port(&id, port).await {
                routes.push(LocalRoute {
                    service,
                    domain,
                    host_port,
                    routes: route_patterns,
                    strip_prefix,
                });
            }
        }
        Ok(routes)
    }

    /// List all orca-managed containers.
    pub async fn list_managed_containers(&self) -> Result<Vec<String>> {
        let mut filters = HashMap::new();
        filters.insert("label", vec![ORCA_LABEL]);
        let opts = ListContainersOptions {
            all: true,
            filters,
            ..Default::default()
        };
        let containers = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| OrcaError::Runtime(format!("list containers failed: {e}")))?;

        Ok(containers.into_iter().filter_map(|c| c.id).collect())
    }

    /// Ensure a Docker network exists, creating it if needed.
    pub async fn ensure_network(&self, name: &str) -> Result<()> {
        if self
            .docker
            .inspect_network::<&str>(name, None)
            .await
            .is_ok()
        {
            return Ok(());
        }
        info!("Creating Docker network: {name}");
        self.docker
            .create_network(CreateNetworkOptions {
                name: name.to_string(),
                ..Default::default()
            })
            .await
            .map_err(|e| OrcaError::Runtime(format!("create network failed: {e}")))?;
        Ok(())
    }

    /// Connect a container to a network with optional aliases.
    pub async fn connect_to_network(
        &self,
        container_id: &str,
        network: &str,
        aliases: &[String],
    ) -> Result<()> {
        let endpoint = bollard::models::EndpointSettings {
            aliases: if aliases.is_empty() {
                None
            } else {
                Some(aliases.to_vec())
            },
            ..Default::default()
        };
        self.docker
            .connect_network(
                network,
                ConnectNetworkOptions {
                    container: container_id.to_string(),
                    endpoint_config: endpoint,
                },
            )
            .await
            .map_err(|e| OrcaError::Runtime(format!("connect network failed: {e}")))?;
        debug!("Connected {container_id} to network {network}");
        Ok(())
    }

    /// Find existing orca-managed containers matching a name prefix.
    ///
    /// Returns `WorkloadHandle`s for running containers whose name starts with
    /// `orca-{name_prefix}`. Used during restart recovery to avoid duplicates.
    pub async fn find_existing(
        &self,
        name_prefix: &str,
    ) -> Result<Vec<orca_core::runtime::WorkloadHandle>> {
        let filter_name = format!("orca-{name_prefix}");
        let mut filters = HashMap::new();
        filters.insert("name", vec![filter_name.as_str()]);
        filters.insert("status", vec!["running"]);
        let opts = ListContainersOptions {
            all: false,
            filters,
            ..Default::default()
        };
        let containers = self
            .docker
            .list_containers(Some(opts))
            .await
            .map_err(|e| OrcaError::Runtime(format!("list containers failed: {e}")))?;

        let mut handles = Vec::new();
        for c in containers {
            let id = match &c.id {
                Some(id) => id.clone(),
                None => continue,
            };
            let name = c
                .names
                .as_ref()
                .and_then(|n| n.first())
                .map(|n| n.trim_start_matches('/').to_string())
                .unwrap_or_default();

            // Docker's name filter is substring-match — enforce exact match
            // here so "keycloak" doesn't pick up "keycloak-db".
            if name != filter_name {
                continue;
            }

            // Extract host port mappings into metadata
            let mut metadata = HashMap::new();
            if let Some(ports) = &c.ports
                && let Some(p) = ports.iter().find(|p| p.public_port.is_some())
            {
                metadata.insert("host_port".to_string(), p.public_port.unwrap().to_string());
            }

            handles.push(orca_core::runtime::WorkloadHandle {
                runtime_id: id,
                name,
                metadata,
            });
        }
        Ok(handles)
    }

    /// Stop and remove all orca-managed containers. Used for graceful shutdown.
    pub async fn cleanup_all(&self) {
        match self.list_managed_containers().await {
            Ok(ids) => {
                for id in ids {
                    let short = &id[..12.min(id.len())];
                    debug!("Cleaning up container {short}");
                    let _ = self
                        .docker
                        .stop_container(&id, Some(StopContainerOptions { t: 5 }))
                        .await;
                    let _ = self
                        .docker
                        .remove_container(
                            &id,
                            Some(RemoveContainerOptions {
                                force: true,
                                ..Default::default()
                            }),
                        )
                        .await;
                }
            }
            Err(e) => {
                warn!("Failed to list containers for cleanup: {e}");
            }
        }
    }
}
