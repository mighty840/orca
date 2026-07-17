//! Exact placement-pin resolution, shared by deploy targeting
//! (`reconciler::find_target_node`) and the per-node expected-services
//! filter (`ws_handler::reconcile::send_reconcile`) so the two always agree
//! on where a pinned service belongs.
//!
//! Regression context (#124): pins were matched with
//! `node.address.contains(pin)`, so `node = "ubuntu"` matched both a node
//! named `ubuntu` and one named `ubuntu-16gb-fsn1-1` — every pinned service
//! deployed to both machines (split-brain, duplicate databases), with the
//! winner of `orca deploy` decided by HashMap iteration order.

use std::collections::HashMap;

use crate::state::RegisteredNode;

/// Outcome of resolving a placement pin against the registered nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PlacementResolution {
    /// No registered node matches. The master itself is not in
    /// `registered_nodes`, so callers must then check
    /// [`pin_matches_master`]: deploy locally when the pin names the
    /// master, refuse when it names nothing — a pin for a vanished node
    /// must never silently land the service on the master.
    NoMatch,
    /// Exactly one node matches.
    Node(u64),
    /// More than one node matches the pin (duplicate identity). Never
    /// schedule on an arbitrary winner — callers must refuse loudly.
    /// Node IDs are sorted for deterministic messages.
    Ambiguous(Vec<u64>),
}

/// Exact-identity match of a placement pin against a single node — never a
/// substring. A pin matches when it equals the node ID, the full
/// `ip:port` address, the address's host portion, or the `hostname` label.
fn node_matches_pin(node: &RegisteredNode, pin: &str) -> bool {
    if pin == node.address || pin == node.node_id.to_string() {
        return true;
    }
    // Addresses are `ip:port` (or `host:port`); pins usually name just the
    // host — `node = "217.154.26.121"` must keep matching `…121:9443`.
    if let Some((host, _port)) = node.address.rsplit_once(':')
        && pin == host
    {
        return true;
    }
    node.labels.get("hostname").is_some_and(|h| h == pin)
}

/// Resolve a pin across all registered nodes. Drained nodes never match —
/// same rule the deploy path has always applied.
pub(crate) fn resolve_placement(
    nodes: &HashMap<u64, RegisteredNode>,
    pin: &str,
) -> PlacementResolution {
    let mut matches: Vec<u64> = nodes
        .values()
        .filter(|n| !n.drain && node_matches_pin(n, pin))
        .map(|n| n.node_id)
        .collect();
    match matches.len() {
        0 => PlacementResolution::NoMatch,
        1 => PlacementResolution::Node(matches[0]),
        _ => {
            matches.sort_unstable();
            PlacementResolution::Ambiguous(matches)
        }
    }
}

/// Node IDs of *drained* nodes the pin would otherwise match. Used only for
/// diagnostics: "your pin is fine, the node is draining" is a different
/// operator action than "your pin names nothing".
pub(crate) fn drained_matches(nodes: &HashMap<u64, RegisteredNode>, pin: &str) -> Vec<u64> {
    let mut ids: Vec<u64> = nodes
        .values()
        .filter(|n| n.drain && node_matches_pin(n, pin))
        .map(|n| n.node_id)
        .collect();
    ids.sort_unstable();
    ids
}

/// The master's own hostname — same convention the ops dashboards use to
/// label the master node.
pub(crate) fn master_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "master".to_string())
}

/// Whether a placement pin explicitly names the master itself. Checked only
/// after no registered agent matched: the master runs workloads locally, so
/// these pins resolve to a local deploy instead of an error.
pub(crate) fn pin_matches_master(pin: &str) -> bool {
    pin == "master" || pin == "localhost" || pin == "127.0.0.1" || pin == master_hostname()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, address: &str, hostname: Option<&str>) -> RegisteredNode {
        let mut labels = HashMap::new();
        if let Some(h) = hostname {
            labels.insert("hostname".to_string(), h.to_string());
        }
        RegisteredNode {
            node_id: id,
            address: address.to_string(),
            labels,
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        }
    }

    fn registry(nodes: Vec<RegisteredNode>) -> HashMap<u64, RegisteredNode> {
        nodes.into_iter().map(|n| (n.node_id, n)).collect()
    }

    /// The #124 incident: hostnames where one is a prefix of the other.
    /// `"ubuntu"` must resolve to exactly the `ubuntu` node, never both.
    #[test]
    fn prefix_hostname_resolves_to_exactly_one_node() {
        let nodes = registry(vec![
            node(1, "217.154.26.121:9443", Some("ubuntu")),
            node(2, "178.105.159.224:9443", Some("ubuntu-16gb-fsn1-1")),
        ]);
        assert_eq!(
            resolve_placement(&nodes, "ubuntu"),
            PlacementResolution::Node(1)
        );
        assert_eq!(
            resolve_placement(&nodes, "ubuntu-16gb-fsn1-1"),
            PlacementResolution::Node(2)
        );
    }

    /// Same incident shape when the prefix collision is in the address
    /// itself (agents can register with a hostname address).
    #[test]
    fn prefix_address_resolves_to_exactly_one_node() {
        let nodes = registry(vec![
            node(1, "ubuntu:9443", None),
            node(2, "ubuntu-16gb-fsn1-1:9443", None),
        ]);
        assert_eq!(
            resolve_placement(&nodes, "ubuntu"),
            PlacementResolution::Node(1)
        );
    }

    /// The prod workaround pins by bare IP while addresses carry a port —
    /// host-portion matching must keep that working.
    #[test]
    fn ip_pin_matches_address_host_portion() {
        let nodes = registry(vec![
            node(1, "217.154.26.121:9443", Some("ubuntu")),
            node(2, "178.105.159.224:9443", Some("ubuntu-16gb-fsn1-1")),
        ]);
        assert_eq!(
            resolve_placement(&nodes, "217.154.26.121"),
            PlacementResolution::Node(1)
        );
        assert_eq!(
            resolve_placement(&nodes, "217.154.26.121:9443"),
            PlacementResolution::Node(1)
        );
    }

    #[test]
    fn node_id_pin_matches() {
        let nodes = registry(vec![node(42, "10.0.0.5:9443", None)]);
        assert_eq!(
            resolve_placement(&nodes, "42"),
            PlacementResolution::Node(42)
        );
    }

    /// Substrings that are not exact identities never match.
    #[test]
    fn substring_pin_no_longer_matches() {
        let nodes = registry(vec![node(1, "217.154.26.121:9443", Some("ubuntu"))]);
        assert_eq!(
            resolve_placement(&nodes, "154.26"),
            PlacementResolution::NoMatch
        );
        assert_eq!(
            resolve_placement(&nodes, "ubun"),
            PlacementResolution::NoMatch
        );
    }

    /// Genuinely duplicated identity is reported, sorted, never picked from.
    #[test]
    fn duplicate_hostname_is_ambiguous() {
        let nodes = registry(vec![
            node(2, "10.0.0.2:9443", Some("worker")),
            node(1, "10.0.0.1:9443", Some("worker")),
        ]);
        assert_eq!(
            resolve_placement(&nodes, "worker"),
            PlacementResolution::Ambiguous(vec![1, 2])
        );
    }

    /// Master pins resolve locally; anything else that matches no node is
    /// the caller's cue to refuse, not to fall through to a master deploy.
    #[test]
    fn master_pins_are_recognized() {
        assert!(pin_matches_master("master"));
        assert!(pin_matches_master("localhost"));
        assert!(pin_matches_master("127.0.0.1"));
        assert!(pin_matches_master(&master_hostname()));
        assert!(!pin_matches_master("ubuntu"));
        assert!(!pin_matches_master("10.0.0.99"));
    }

    #[test]
    fn drained_node_never_matches() {
        let mut drained = node(1, "10.0.0.1:9443", Some("worker"));
        drained.drain = true;
        let nodes = registry(vec![drained, node(2, "10.0.0.2:9443", Some("worker"))]);
        assert_eq!(
            resolve_placement(&nodes, "worker"),
            PlacementResolution::Node(2)
        );
    }
}
