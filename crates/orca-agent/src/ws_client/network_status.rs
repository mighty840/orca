//! Network status reporter: respond to `MasterMessage::NetworkStatusRequest`
//! by enumerating this node's `orca-*` Docker bridge networks and sending the
//! report back as `AgentMessage::NetworkStatusReport`.

use bollard::Docker;
use tokio::sync::mpsc;
use tracing::warn;

use orca_core::api_types::{DockerNetwork, NetworkService};
use orca_core::ws_types::{AgentMessage, NetworkStatusReportData};

/// Convenience for callers (e.g. `orca-control`'s master endpoint) that just
/// want the enumeration result without having to open a Docker client first.
/// Returns an empty list if Docker is unreachable.
pub async fn list_local_orca_networks() -> Vec<DockerNetwork> {
    match Docker::connect_with_local_defaults() {
        Ok(docker) => enumerate_orca_networks(&docker).await,
        Err(e) => {
            warn!("docker connect failed: {e}");
            Vec::new()
        }
    }
}

/// Enumerate `orca-*` networks and send the report. Spawned as a task by the
/// dispatch loop so it doesn't block heartbeat or other traffic.
pub(super) async fn send_network_status(
    request_id: String,
    node_id: u64,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let networks = match Docker::connect_with_local_defaults() {
        Ok(docker) => enumerate_orca_networks(&docker).await,
        Err(e) => {
            warn!("WS: cannot enumerate networks — docker connect failed: {e}");
            Vec::new()
        }
    };
    let _ = out_tx
        .send(AgentMessage::NetworkStatusReport {
            request_id,
            data: NetworkStatusReportData {
                node_id,
                hostname: node_hostname(),
                networks,
            },
        })
        .await;
}

/// Build the per-`orca-*`-network listing. Two-step because Docker's network
/// inspection omits per-container aliases — those live on each container's
/// `EndpointSettings`. So:
///   1. `list_networks` seeds the map (catches empty bridges too).
///   2. `list_containers` + `inspect_container` populates the services with
///      their aliases.
///
/// Failures on individual containers/networks are silently skipped — partial
/// data is better than no data when the dashboard is open.
pub async fn enumerate_orca_networks(docker: &Docker) -> Vec<DockerNetwork> {
    use std::collections::BTreeMap;

    let mut network_map: BTreeMap<String, Vec<NetworkService>> = BTreeMap::new();

    // Step 1: seed every orca-* network (including empty ones).
    match docker.list_networks::<&str>(None).await {
        Ok(networks) => {
            for net in networks {
                if let Some(name) = net.name
                    && name.starts_with("orca-")
                {
                    network_map.entry(name).or_default();
                }
            }
        }
        Err(e) => warn!("WS: list_networks failed: {e}"),
    }

    // Step 2: walk orca-* containers, pulling each one's per-network aliases
    // out of `inspect_container.NetworkSettings.Networks`.
    let containers = match docker
        .list_containers::<&str>(Some(bollard::container::ListContainersOptions {
            all: true,
            ..Default::default()
        }))
        .await
    {
        Ok(c) => c,
        Err(e) => {
            warn!("WS: list_containers failed: {e}");
            return network_map
                .into_iter()
                .map(|(name, services)| DockerNetwork { name, services })
                .collect();
        }
    };

    // Inspect every orca-* container concurrently. Sequential per-container
    // calls used to dominate the dashboard load time on boxes with many
    // services — `inspect_container` round-trips to the Docker socket, and
    // 20+ serial calls felt like a UI hang.
    use futures_util::future::join_all;
    let inspect_jobs = containers.into_iter().filter_map(|summary| {
        let names = summary.names?;
        let raw = names.first()?;
        let container_name = raw.trim_start_matches('/').to_string();
        if !container_name.starts_with("orca-") {
            return None;
        }
        let id = summary.id?;
        Some(async move {
            let detail = docker
                .inspect_container(&id, None::<bollard::container::InspectContainerOptions>)
                .await
                .ok()?;
            let nets = detail.network_settings.and_then(|s| s.networks)?;
            Some((container_name, nets))
        })
    });

    for result in join_all(inspect_jobs).await.into_iter().flatten() {
        let (container_name, nets) = result;
        for (net_name, endpoint) in nets {
            if !net_name.starts_with("orca-") {
                continue;
            }
            let aliases = endpoint.aliases.unwrap_or_default();
            network_map
                .entry(net_name)
                .or_default()
                .push(NetworkService {
                    name: container_name.clone(),
                    aliases,
                    // Populated by the master's `annotate_missing_aliases`
                    // pass once it cross-references env across services.
                    missing_aliases: Vec::new(),
                });
        }
    }

    let mut out: Vec<DockerNetwork> = network_map
        .into_iter()
        .map(|(name, mut services)| {
            services.sort_by(|a, b| a.name.cmp(&b.name));
            DockerNetwork { name, services }
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn node_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    /// Even when Docker is unreachable, the agent must reply with a
    /// well-formed empty report so the master's collector loop doesn't time
    /// out waiting for a response. The test relies on `Docker::connect_with_local_defaults`
    /// either succeeding (no orca-* nets on a CI runner → empty) or failing
    /// (no docker socket → empty); either way the receiver gets one message.
    #[tokio::test]
    async fn send_network_status_replies_even_with_no_docker() {
        let (tx, mut rx) = mpsc::channel::<AgentMessage>(4);
        send_network_status("req-1".into(), 42, tx).await;
        let got = rx.try_recv().expect("expected a report");
        match got {
            AgentMessage::NetworkStatusReport { request_id, data } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(data.node_id, 42);
                let _ = data.networks;
            }
            _ => panic!("expected NetworkStatusReport"),
        }
    }
}
