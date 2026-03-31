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
use bollard::container::InspectContainerOptions;

/// Docker container runtime.
pub struct ContainerRuntime {
    pub(crate) docker: Docker,
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
        // Check if image exists locally
        if self.docker.inspect_image(image).await.is_ok() {
            debug!("Image {image} already available locally");
            return Ok(());
        }

        info!("Pulling image: {image}");
        let opts = CreateImageOptions {
            from_image: image,
            ..Default::default()
        };

        let mut stream = self.docker.create_image(Some(opts), None, None);
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
