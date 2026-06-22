//! Adoption scanner: respond to `MasterMessage::AdoptionScanRequest` by
//! enumerating every `orca.managed=true` container on this node so the master
//! can adopt orphans missing from its registry (#95).

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::ListContainersOptions;
use tokio::sync::mpsc;
use tracing::warn;

use orca_core::ws_types::{AdoptionReportData, AgentMessage, ManagedContainer};

use crate::docker::ORCA_LABEL;

/// Enumerate orca-managed containers and send an `AdoptionReport`. Spawned as a
/// task by the dispatch loop so it never blocks heartbeat or other traffic.
pub(super) async fn send_adoption_report(
    request_id: String,
    node_id: u64,
    out_tx: mpsc::Sender<AgentMessage>,
) {
    let containers = match Docker::connect_with_local_defaults() {
        Ok(docker) => list_managed_containers(&docker).await,
        Err(e) => {
            warn!("WS: cannot scan for adoption — docker connect failed: {e}");
            Vec::new()
        }
    };
    let _ = out_tx
        .send(AgentMessage::AdoptionReport {
            request_id,
            data: AdoptionReportData {
                node_id,
                hostname: node_hostname(),
                containers,
            },
        })
        .await;
}

/// Build one `ManagedContainer` per `orca.managed=true` container, pulling the
/// service metadata out of the `orca.*` labels plus the container image/state.
/// Containers without an `orca.service` label are skipped — they can't be
/// adopted without a service name.
async fn list_managed_containers(docker: &Docker) -> Vec<ManagedContainer> {
    let mut filters = HashMap::new();
    filters.insert("label", vec![ORCA_LABEL]);
    let opts = ListContainersOptions {
        all: true,
        filters,
        ..Default::default()
    };
    let summaries = match docker.list_containers(Some(opts)).await {
        Ok(c) => c,
        Err(e) => {
            warn!("WS: adoption scan list_containers failed: {e}");
            return Vec::new();
        }
    };

    let mut out = Vec::new();
    for c in summaries {
        let labels = c.labels.unwrap_or_default();
        let service_name = match labels.get("orca.service") {
            Some(s) if !s.is_empty() => s.clone(),
            _ => continue,
        };
        let routes = split_label(labels.get("orca.routes"));
        // Prefer the multi-domain list; fall back to the single `orca.domain`.
        let mut domains = split_label(labels.get("orca.domains"));
        if domains.is_empty()
            && let Some(d) = labels.get("orca.domain")
        {
            domains.push(d.clone());
        }
        out.push(ManagedContainer {
            service_name,
            image: c.image.unwrap_or_default(),
            status: c.state.unwrap_or_default(),
            container_id: c.id.unwrap_or_default(),
            port: labels.get("orca.port").and_then(|p| p.parse::<u16>().ok()),
            domains,
            network: labels.get("orca.network").cloned(),
            routes,
            strip_prefix: labels.get("orca.strip_prefix").cloned(),
        });
    }
    out
}

/// Split a comma-joined label value into a clean list, dropping empties.
fn split_label(value: Option<&String>) -> Vec<String> {
    value
        .map(|s| {
            s.split(',')
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
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

    /// Even when Docker is unreachable, the agent must still reply with a
    /// well-formed (empty) report so the master's collector loop doesn't time
    /// out waiting for this node.
    #[tokio::test]
    async fn send_adoption_report_replies_even_with_no_docker() {
        let (tx, mut rx) = mpsc::channel::<AgentMessage>(4);
        send_adoption_report("req-1".into(), 42, tx).await;
        let got = rx.try_recv().expect("expected a report");
        match got {
            AgentMessage::AdoptionReport { request_id, data } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(data.node_id, 42);
                let _ = data.containers;
            }
            _ => panic!("expected AdoptionReport"),
        }
    }
}
