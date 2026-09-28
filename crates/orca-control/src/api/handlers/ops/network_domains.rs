//! Which node serves which domain, for the networks dashboard.
//!
//! The dashboard used to slice the master's proxy route table by placement.
//! That table only holds the routes the master serves itself: a service on
//! an agent is routed by the agent's proxy, so every agent row came back
//! with no domains, although checking an agent's DNS is what the view is
//! for. Domains now come from the service definitions and go to the node
//! each service's placement resolves to.

use std::collections::HashMap;

use orca_core::api_types::DomainRoute;

use crate::placement::{PlacementResolution, pin_matches_master, resolve_placement};
use crate::state::AppState;

/// The dashboard row a domain belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum NodeKey {
    /// No placement, or a pin naming the master itself.
    Master,
    /// A registered agent the pin resolves to.
    Node(u64),
    /// A pin no single registered node matches right now (the agent is
    /// offline, draining, or the pin is ambiguous). Shown on an agent row
    /// whose hostname equals the pin, like before.
    Pin(String),
}

/// One service's domains and placement pin.
pub(crate) struct ServiceDomains {
    pub name: String,
    pub domains: Vec<String>,
    pub pin: Option<String>,
}

/// Group `services`' domains by the node that serves them. `resolve` maps a
/// pin to a registered node id; `is_master` says whether a pin names the
/// master. Both are parameters so the grouping is testable.
pub(crate) fn group(
    services: &[ServiceDomains],
    resolve: impl Fn(&str) -> Option<u64>,
    is_master: impl Fn(&str) -> bool,
) -> HashMap<NodeKey, Vec<DomainRoute>> {
    let mut out: HashMap<NodeKey, Vec<DomainRoute>> = HashMap::new();
    for svc in services.iter().filter(|s| !s.domains.is_empty()) {
        let key = match svc.pin.as_deref() {
            None => NodeKey::Master,
            Some(pin) if is_master(pin) => NodeKey::Master,
            Some(pin) => resolve(pin).map_or_else(|| NodeKey::Pin(pin.to_string()), NodeKey::Node),
        };
        let routes = out.entry(key).or_default();
        for domain in &svc.domains {
            routes.push(DomainRoute {
                domain: domain.clone(),
                service: svc.name.clone(),
                resolved_ip: None,
            });
        }
    }
    for routes in out.values_mut() {
        routes.sort_by(|a, b| a.domain.cmp(&b.domain).then(a.service.cmp(&b.service)));
        routes.dedup_by(|a, b| a.domain == b.domain && a.service == b.service);
    }
    out
}

/// [`group`] over the cluster's services. Reads are short and scoped: no
/// lock is held while the result is built, and the proxy's route table
/// isn't touched at all (holding it stalled TLS handshakes once).
pub(crate) async fn domains_by_node(state: &AppState) -> HashMap<NodeKey, Vec<DomainRoute>> {
    let services: Vec<ServiceDomains> = state
        .services
        .read()
        .await
        .values()
        .map(|s| ServiceDomains {
            name: s.config.name.clone(),
            domains: s.config.all_domains(),
            pin: s.config.placement.as_ref().and_then(|p| p.node.clone()),
        })
        .collect();
    let nodes = state.registered_nodes.read().await.clone();
    group(
        &services,
        |pin| match resolve_placement(&nodes, pin) {
            PlacementResolution::Node(id) => Some(id),
            _ => None,
        },
        pin_matches_master,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, domains: &[&str], pin: Option<&str>) -> ServiceDomains {
        ServiceDomains {
            name: name.into(),
            domains: domains.iter().map(|d| d.to_string()).collect(),
            pin: pin.map(str::to_string),
        }
    }

    fn names(routes: Option<&Vec<DomainRoute>>) -> Vec<&str> {
        routes
            .map(|r| r.iter().map(|d| d.domain.as_str()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn an_agents_services_land_on_the_agents_row() {
        // The bug: agent rows were always empty, because their domains are
        // not in the master's proxy route table.
        let services = [
            svc("gitea", &["git.example.com"], None),
            svc("nextcloud", &["cloud.example.com"], Some("agent-1")),
            svc(
                "erp",
                &["erp.example.com", "www.erp.example.com"],
                Some("agent-1"),
            ),
        ];
        let by_node = group(&services, |pin| (pin == "agent-1").then_some(7), |_| false);
        assert_eq!(names(by_node.get(&NodeKey::Master)), ["git.example.com"]);
        assert_eq!(
            names(by_node.get(&NodeKey::Node(7))),
            [
                "cloud.example.com",
                "erp.example.com",
                "www.erp.example.com"
            ]
        );
    }

    #[test]
    fn a_pin_naming_the_master_is_the_masters() {
        let services = [svc("keycloak", &["auth.example.com"], Some("infra-vm1"))];
        let by_node = group(&services, |_| None, |pin| pin == "infra-vm1");
        assert_eq!(names(by_node.get(&NodeKey::Master)), ["auth.example.com"]);
    }

    #[test]
    fn an_unresolved_pin_keeps_its_domains() {
        // Agent offline or draining: the domains stay visible under the pin.
        let services = [svc("jitsi", &["meet.example.com"], Some("agent-2"))];
        let by_node = group(&services, |_| None, |_| false);
        assert_eq!(
            names(by_node.get(&NodeKey::Pin("agent-2".into()))),
            ["meet.example.com"]
        );
    }

    #[test]
    fn services_without_domains_are_skipped() {
        let services = [svc("db", &[], Some("agent-1"))];
        assert!(group(&services, |_| Some(7), |_| false).is_empty());
    }
}
