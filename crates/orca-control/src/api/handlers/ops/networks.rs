//! Cluster networks dashboard: per-node `orca-*` Docker bridge listing plus
//! the public-edge domain routes registered with each node's proxy, and a
//! missing-alias pass that flags env references with no DNS path.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use tokio::net::lookup_host;
use tracing::warn;

use orca_core::api_types::{
    ClusterNetworksResponse, DockerNetwork, DomainRoute, NodeNetworks, NodeRole,
};
use orca_core::ws_types::{MasterMessage, NetworkStatusReportData};

use crate::state::AppState;

/// Per-agent collection timeout. Generous because docker `inspect_container`
/// over many containers can be slow on a loaded box.
const NETWORK_REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-domain DNS resolution timeout. Short — most resolvers answer in ms;
/// any domain that doesn't respond quickly is "unknown" for this snapshot
/// and the operator will see a `?` in the IP column.
const DNS_TIMEOUT: Duration = Duration::from_millis(800);

/// GET /api/v1/cluster/networks — aggregate Docker network + edge route info
/// across every node in the cluster, with DNS-resolved domain IPs and a
/// missing-alias pass that flags broken env references.
pub(crate) async fn cluster_networks(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Step 1: slice the master's route table by placement node so each
    // node row gets the edge routes for services it actually hosts.
    let routes_by_node = group_routes_by_placement(&state).await;

    let master = collect_master(&state, &routes_by_node).await;
    let agents = collect_agents(&state, &routes_by_node).await;

    let mut nodes = vec![master];
    nodes.extend(agents);

    // Step 2: resolve every unique domain in parallel. Cheap-ish (DNS is
    // milliseconds for cached entries, capped by DNS_TIMEOUT otherwise).
    resolve_domain_ips(&mut nodes).await;

    // Step 3: scan service env across the cluster and flag references with
    // no matching alias on a shared network.
    annotate_missing_aliases(&mut nodes, &state).await;

    Json(ClusterNetworksResponse { nodes })
}

/// Build `placement.node → [DomainRoute]`. The key is `Some(hostname)` for
/// agent-pinned services and `None` for services with no placement (i.e.,
/// the master). Routes whose service isn't in the services map (e.g. stale
/// route after a service was removed) fall under `None` defensively.
async fn group_routes_by_placement(state: &AppState) -> HashMap<Option<String>, Vec<DomainRoute>> {
    // Snapshot the data we need under tight, scoped reads, then drop both
    // guards before any further work. Holding `state.route_table.read()`
    // across iteration was the primary lock-contention source that stalled
    // proxy TLS handshakes by up to 20 seconds — tokio::sync::RwLock uses
    // write-priority, so once *any* writer (update_container_routes from
    // the reconciler/watchdog) queues behind this long-running reader, all
    // subsequent readers (including the serve-loop check that happens
    // between peek_sni and acceptor.accept) queue behind the writer too.
    // See project_v0_2_9_rc2_proxy_lock_contention for the diagnosis.
    let service_placements: HashMap<String, Option<String>> = {
        let services = state.services.read().await;
        services
            .values()
            .map(|s| {
                (
                    s.config.name.clone(),
                    s.config.placement.as_ref().and_then(|p| p.node.clone()),
                )
            })
            .collect()
    };
    let route_snapshot: Vec<(String, Vec<String>)> = {
        let routes = state.route_table.read().await;
        routes
            .iter()
            .map(|(domain, targets)| {
                (
                    domain.clone(),
                    targets.iter().map(|t| t.service_name.clone()).collect(),
                )
            })
            .collect()
    };

    let mut out: HashMap<Option<String>, Vec<DomainRoute>> = HashMap::new();
    for (domain, service_names) in &route_snapshot {
        for service_name in service_names {
            let node = service_placements.get(service_name).cloned().flatten();
            out.entry(node).or_default().push(DomainRoute {
                domain: domain.clone(),
                service: service_name.clone(),
                resolved_ip: None,
            });
        }
    }
    for v in out.values_mut() {
        v.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.service.cmp(&b.service)));
        v.dedup_by(|a, b| a.domain == b.domain && a.service == b.service);
    }
    out
}

async fn collect_master(
    _state: &AppState,
    routes_by_node: &HashMap<Option<String>, Vec<DomainRoute>>,
) -> NodeNetworks {
    let networks = enumerate_local_orca_networks().await;
    let domains = routes_by_node.get(&None).cloned().unwrap_or_default();
    NodeNetworks {
        node_id: None,
        hostname: crate::placement::master_hostname(),
        role: NodeRole::Master,
        networks,
        domains,
        reachable: true,
    }
}

async fn collect_agents(
    state: &AppState,
    routes_by_node: &HashMap<Option<String>, Vec<DomainRoute>>,
) -> Vec<NodeNetworks> {
    let agent_ids: Vec<u64> = state.ws_agents.read().await.keys().copied().collect();
    if agent_ids.is_empty() {
        return Vec::new();
    }

    let (tx, mut rx) = tokio::sync::mpsc::channel::<NetworkStatusReportData>(agent_ids.len() + 1);
    let mut request_ids: Vec<String> = Vec::with_capacity(agent_ids.len());
    {
        let mut listeners = state.network_listeners.write().await;
        for _ in 0..agent_ids.len() {
            let req = uuid::Uuid::new_v4().to_string();
            listeners.insert(req.clone(), tx.clone());
            request_ids.push(req);
        }
    }

    let mut dispatched = HashMap::<u64, String>::new();
    {
        let agents = state.ws_agents.read().await;
        for (node_id, req_id) in agent_ids.iter().zip(request_ids.iter()) {
            if let Some(agent_tx) = agents.get(node_id) {
                let sent = agent_tx
                    .send(MasterMessage::NetworkStatusRequest {
                        request_id: req_id.clone(),
                    })
                    .await
                    .is_ok();
                if sent {
                    dispatched.insert(*node_id, req_id.clone());
                }
            }
        }
    }

    let mut reports = HashMap::<u64, NetworkStatusReportData>::new();
    let deadline = tokio::time::sleep(NETWORK_REPORT_TIMEOUT);
    tokio::pin!(deadline);
    while reports.len() < dispatched.len() {
        tokio::select! {
            biased;
            msg = rx.recv() => {
                match msg {
                    Some(data) => { reports.insert(data.node_id, data); }
                    None => break,
                }
            }
            _ = &mut deadline => {
                warn!(
                    "cluster_networks: timed out after {}s ({} of {} agents responded)",
                    NETWORK_REPORT_TIMEOUT.as_secs(),
                    reports.len(),
                    dispatched.len(),
                );
                break;
            }
        }
    }

    {
        let mut listeners = state.network_listeners.write().await;
        for req in &request_ids {
            listeners.remove(req);
        }
    }

    agent_ids
        .into_iter()
        .map(|node_id| match reports.remove(&node_id) {
            Some(data) => {
                let domains = routes_by_node
                    .get(&Some(data.hostname.clone()))
                    .cloned()
                    .unwrap_or_default();
                NodeNetworks {
                    node_id: Some(node_id),
                    hostname: data.hostname,
                    role: NodeRole::Agent,
                    networks: data.networks,
                    domains,
                    reachable: true,
                }
            }
            None => NodeNetworks {
                node_id: Some(node_id),
                hostname: format!("node-{node_id}"),
                role: NodeRole::Agent,
                networks: Vec::new(),
                domains: Vec::new(),
                reachable: false,
            },
        })
        .collect()
}

/// Walk every domain across every node in parallel and populate
/// `resolved_ip`. Bounded by [`DNS_TIMEOUT`] per domain so a stuck
/// resolver doesn't block the dashboard. Uses the OS resolver via
/// `tokio::net::lookup_host`.
async fn resolve_domain_ips(nodes: &mut [NodeNetworks]) {
    use futures_util::future::join_all;

    // Dedup domains so we don't pay multiple times for the same name.
    let mut unique: HashSet<String> = HashSet::new();
    for n in nodes.iter() {
        for d in &n.domains {
            unique.insert(d.domain.clone());
        }
    }
    let jobs = unique.into_iter().map(|domain| async move {
        let target = format!("{domain}:0");
        let result = tokio::time::timeout(DNS_TIMEOUT, lookup_host(target)).await;
        let ip = match result {
            Ok(Ok(mut iter)) => iter.next().map(|sa| sa.ip().to_string()),
            _ => None,
        };
        (domain, ip)
    });
    let resolved: HashMap<String, Option<String>> = join_all(jobs).await.into_iter().collect();
    for n in nodes.iter_mut() {
        for d in &mut n.domains {
            if let Some(ip) = resolved.get(&d.domain).and_then(|x| x.clone()) {
                d.resolved_ip = Some(ip);
            }
        }
    }
}

/// Heuristic missing-alias detection. For each service that has env vars,
/// look for tokens matching another known service's name. If the referenced
/// service isn't aliased as that name on a shared Docker network, record the
/// missing alias on the *referenced* service's row so the dashboard can
/// highlight where the gap is.
///
/// The detection is intentionally conservative — false positives are worse
/// than false negatives for an ops dashboard (the operator stops trusting
/// the warning), so we only flag exact substring matches that look like
/// hostnames (whole words bounded by typical URL chars).
async fn annotate_missing_aliases(nodes: &mut [NodeNetworks], state: &AppState) {
    let services = state.services.read().await;
    if services.is_empty() {
        return;
    }
    // service_name → its set of aliases across all networks. Used both as
    // "is this name a known service?" and to compare references against.
    let mut all_aliases: HashMap<String, HashSet<String>> = HashMap::new();
    for n in nodes.iter() {
        for net in &n.networks {
            for svc in &net.services {
                let entry = all_aliases.entry(svc.name.clone()).or_default();
                for a in &svc.aliases {
                    entry.insert(a.clone());
                }
            }
        }
    }
    // Build a referrer map: referenced_service → set of names it's looked
    // up as. A name is "looked up as X" when service A's env contains a
    // bare token X that matches another service name.
    let mut references: HashMap<String, HashSet<String>> = HashMap::new();
    let known_names: HashSet<&str> = services.keys().map(|s| s.as_str()).collect();
    for svc in services.values() {
        for value in svc.config.env.values() {
            for token in tokens_from_env_value(value) {
                if known_names.contains(token.as_str()) && token != svc.config.name {
                    references.entry(token.clone()).or_default().insert(token);
                }
            }
        }
    }
    // For each container row in the rendered networks, see whether its
    // aliases cover everything it's referenced as. The container name
    // produced by orca is `orca-<service>` or `orca-<service>-<n>`; map
    // back to the service name by trimming.
    for n in nodes.iter_mut() {
        for net in &mut n.networks {
            for svc in &mut net.services {
                let svc_short = service_name_from_container(&svc.name);
                let Some(refs) = references.get(&svc_short) else {
                    continue;
                };
                let alias_set: HashSet<&str> = svc.aliases.iter().map(|s| s.as_str()).collect();
                let missing: Vec<String> = refs
                    .iter()
                    .filter(|r| !alias_set.contains(r.as_str()))
                    .cloned()
                    .collect();
                if !missing.is_empty() {
                    let mut m = missing;
                    m.sort();
                    m.dedup();
                    svc.missing_aliases = m;
                }
            }
        }
    }
}

/// Extract bare-word tokens from an env value that *look* like hostnames.
/// Splits on common separators in URLs and connection strings; keeps tokens
/// composed of `[a-z0-9-]` characters. Aggressive enough to catch
/// `postgres://db:5432/x` (extracts "db") and `http://api/v1` (extracts
/// "api"), but conservative enough not to fire on prose values.
pub(crate) fn tokens_from_env_value(value: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for ch in value.chars() {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' {
            cur.push(ch);
        } else {
            if !cur.is_empty() && cur.len() > 1 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
        }
    }
    if !cur.is_empty() && cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// Orca container names are `orca-<service>` (and `orca-<service>-<N>` for
/// replicas). Strip the prefix and trailing replica suffix so we can match
/// against `services.keys()`.
fn service_name_from_container(container: &str) -> String {
    let rest = container.trim_start_matches("orca-");
    // Drop a trailing `-<digits>` suffix if present.
    if let Some((stem, tail)) = rest.rsplit_once('-')
        && tail.chars().all(|c| c.is_ascii_digit())
    {
        return stem.to_string();
    }
    rest.to_string()
}

/// Walk the master's own Docker daemon. Delegates to the agent crate's
/// enumerator so master and agent always see the same shape.
async fn enumerate_local_orca_networks() -> Vec<DockerNetwork> {
    orca_agent::ws_client::network_status::list_local_orca_networks().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_extract_hostnames_from_url() {
        assert_eq!(
            tokens_from_env_value("postgres://db:5432/app"),
            vec!["postgres", "db", "5432", "app"]
        );
        assert_eq!(
            tokens_from_env_value("http://api-gateway/v1"),
            vec!["http", "api-gateway", "v1"]
        );
    }

    #[test]
    fn tokens_skip_single_chars_and_uppercase_garbage() {
        // Single-char tokens are dropped (`len > 1`), and uppercase
        // content (`A`, `B`) doesn't contribute since we match `[a-z0-9-]`
        // only. So `c` is excluded too — single char.
        assert_eq!(tokens_from_env_value("A=B;c=d-e"), vec!["d-e"]);
    }

    #[test]
    fn service_name_strips_orca_prefix_and_replica_suffix() {
        assert_eq!(service_name_from_container("orca-api"), "api");
        assert_eq!(service_name_from_container("orca-api-3"), "api");
        assert_eq!(
            service_name_from_container("orca-api-gateway-1"),
            "api-gateway"
        );
        // No prefix → pass through. Trailing non-numeric suffix → keep the
        // service-name part after stripping `orca-`.
        assert_eq!(service_name_from_container("standalone"), "standalone");
        assert_eq!(service_name_from_container("orca-api-canary"), "api-canary");
    }
}
