//! The node-local proxy's route table on a joined node, built from the
//! running containers' `orca.*` labels.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use tracing::info;

use orca_proxy::RouteTarget;

/// How long a domain whose last container on this node stopped keeps an
/// explicit empty route entry. The proxy answers such a host 503 ("known,
/// no backend right now") instead of the unknown-host 404, so clients retry
/// across a redeploy's container swap. OTLP exporters, for one, drop data
/// on a 404 and retry a 503.
pub(crate) const GONE_DOMAIN_GRACE: Duration = Duration::from_secs(300);

/// Route table keyed by domain, as the proxy reads it.
pub(crate) type RouteTable = HashMap<String, Vec<RouteTarget>>;

/// The proxy route table for the containers running on this node: one
/// target per declared path pattern, a single catch-all for a service
/// without `routes`.
pub(crate) fn route_table_from(routes: &[orca_agent::docker::LocalRoute]) -> RouteTable {
    let mut table = RouteTable::new();
    for r in routes {
        let patterns: Vec<Option<String>> = if r.routes.is_empty() {
            vec![None]
        } else {
            r.routes.iter().map(|p| Some(p.clone())).collect()
        };
        for pattern in patterns {
            table
                .entry(r.domain.clone())
                .or_default()
                .push(RouteTarget {
                    address: format!("127.0.0.1:{}", r.host_port),
                    service_name: r.service.clone(),
                    path_pattern: pattern,
                    strip_prefix: r.strip_prefix.clone(),
                    weight: 100,
                });
        }
    }
    table
}

/// Domains that recently lost their last running container on this node,
/// with the time each was first seen without one.
#[derive(Debug, Default)]
pub(crate) struct GoneDomains {
    since: HashMap<String, Instant>,
}

impl GoneDomains {
    /// Add an empty entry to `table` for every domain that was in the
    /// previously published table (`previous`) but has no running container
    /// now, for up to `grace` after it went. A domain that comes back is
    /// forgotten; one gone longer than `grace` is dropped, so it then
    /// answers 404 like any unknown host.
    pub(crate) fn apply(
        &mut self,
        previous: &HashSet<String>,
        table: &mut RouteTable,
        now: Instant,
        grace: Duration,
    ) {
        for domain in previous {
            if !table.contains_key(domain) && !self.since.contains_key(domain) {
                info!("{domain}: no running container on this node, answering 503 meanwhile");
                self.since.insert(domain.clone(), now);
            }
        }
        self.since.retain(|domain, gone_at| {
            if table.contains_key(domain) {
                return false;
            }
            let keep = now.duration_since(*gone_at) < grace;
            if !keep {
                info!("{domain}: still no container after {grace:?}, route dropped");
            }
            keep
        });
        for domain in self.since.keys() {
            table.insert(domain.clone(), Vec::new());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_agent::docker::LocalRoute;

    fn local(service: &str, domain: &str, port: u16, routes: &[&str]) -> LocalRoute {
        LocalRoute {
            service: service.into(),
            domain: domain.into(),
            host_port: port,
            routes: routes.iter().map(|r| r.to_string()).collect(),
            strip_prefix: None,
        }
    }

    fn domains(table: &RouteTable) -> HashSet<String> {
        table.keys().cloned().collect()
    }

    /// #305: the table built at startup must route every running service,
    /// so the proxy has targets before the refresher's first pass.
    #[test]
    fn route_table_routes_every_running_service() {
        let table = route_table_from(&[
            local("portal", "app.example.com", 32880, &[]),
            local("api", "api.example.com", 32881, &["/v1/*", "/health"]),
            local("docs", "api.example.com", 32882, &["/docs/*"]),
        ]);

        let app = &table["app.example.com"];
        assert_eq!(app.len(), 1);
        assert_eq!(app[0].address, "127.0.0.1:32880");
        assert_eq!(app[0].path_pattern, None);

        let api = &table["api.example.com"];
        let patterns: Vec<_> = api
            .iter()
            .map(|t| (t.service_name.as_str(), t.path_pattern.as_deref()))
            .collect();
        assert_eq!(
            patterns,
            [
                ("api", Some("/v1/*")),
                ("api", Some("/health")),
                ("docs", Some("/docs/*")),
            ]
        );
    }

    #[test]
    fn no_running_services_is_an_empty_table() {
        assert!(route_table_from(&[]).is_empty());
    }

    /// A container swap: the domain stays known (empty, so 503) while no
    /// container runs, and routes normally again once the new one is up.
    #[test]
    fn a_domain_between_containers_stays_known_and_empty() {
        let mut gone = GoneDomains::default();
        let t0 = Instant::now();
        let before = domains(&route_table_from(&[local(
            "otel",
            "otel.example.com",
            1,
            &[],
        )]));

        let mut during = route_table_from(&[]);
        gone.apply(&before, &mut during, t0, GONE_DOMAIN_GRACE);
        assert_eq!(during.get("otel.example.com").map(Vec::len), Some(0));

        let mut after = route_table_from(&[local("otel", "otel.example.com", 2, &[])]);
        gone.apply(
            &domains(&during),
            &mut after,
            t0 + Duration::from_secs(5),
            GONE_DOMAIN_GRACE,
        );
        assert_eq!(after["otel.example.com"][0].address, "127.0.0.1:2");
        assert!(
            gone.since.is_empty(),
            "a domain that came back is forgotten"
        );
    }

    /// The grace counts from when the domain went, not from the last pass,
    /// and a domain gone for longer is dropped (404 again).
    #[test]
    fn a_domain_gone_longer_than_the_grace_is_dropped() {
        let mut gone = GoneDomains::default();
        let t0 = Instant::now();
        let mut previous = HashSet::from(["old.example.com".to_string()]);
        for secs in [0, 120, 299] {
            let mut table = route_table_from(&[]);
            gone.apply(
                &previous,
                &mut table,
                t0 + Duration::from_secs(secs),
                GONE_DOMAIN_GRACE,
            );
            assert!(
                table.contains_key("old.example.com"),
                "still within grace at {secs}s"
            );
            previous = domains(&table);
        }
        let mut table = route_table_from(&[]);
        gone.apply(
            &previous,
            &mut table,
            t0 + GONE_DOMAIN_GRACE,
            GONE_DOMAIN_GRACE,
        );
        assert!(table.is_empty());
        assert!(gone.since.is_empty());
    }

    /// Domains that never had a container here are not invented.
    #[test]
    fn only_previously_published_domains_are_kept() {
        let mut gone = GoneDomains::default();
        let mut table = route_table_from(&[local("app", "app.example.com", 1, &[])]);
        gone.apply(
            &HashSet::new(),
            &mut table,
            Instant::now(),
            GONE_DOMAIN_GRACE,
        );
        assert_eq!(
            domains(&table),
            HashSet::from(["app.example.com".to_string()])
        );
    }
}
