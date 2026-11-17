//! Topological sorting for service dependency ordering.

use std::collections::HashSet;

use orca_core::config::ServiceConfig;

/// Sort services topologically so dependencies are reconciled first.
///
/// Services with no unresolved dependencies are placed first. A dependency
/// that is not in `configs` (it runs on another node, or is paused) does not
/// hold its dependents back: there is nothing in this list to wait for, and
/// treating it as unresolved used to drop the whole chain behind it into
/// input order (#311). Services in a cycle are appended to avoid deadlock.
pub fn topo_sort(configs: &[ServiceConfig]) -> Vec<ServiceConfig> {
    let mut ordered = Vec::new();
    let mut remaining: Vec<_> = configs.to_vec();
    let listed: HashSet<String> = configs.iter().map(|c| c.name.clone()).collect();
    let mut resolved: HashSet<String> = HashSet::new();

    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|c| {
            let ready = c
                .depends_on
                .iter()
                .all(|d| resolved.contains(d) || !listed.contains(d));
            if ready {
                ordered.push(c.clone());
                resolved.insert(c.name.clone());
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            // Circular dependency — append remaining to avoid hang
            ordered.append(&mut remaining);
        }
    }
    ordered
}

#[cfg(test)]
mod tests {
    use super::*;

    fn svc(name: &str, deps: &[&str]) -> ServiceConfig {
        serde_json::from_value(serde_json::json!({
            "name": name,
            "image": "nginx:latest",
            "depends_on": deps,
        }))
        .expect("valid service config")
    }

    fn names(configs: &[ServiceConfig]) -> Vec<&str> {
        configs.iter().map(|c| c.name.as_str()).collect()
    }

    #[test]
    fn a_chain_behind_an_unlisted_dependency_keeps_its_order() {
        // `cache` runs on another node: `api` must still come before `web`.
        let sorted = topo_sort(&[
            svc("web", &["api"]),
            svc("api", &["cache", "db"]),
            svc("db", &[]),
        ]);
        assert_eq!(names(&sorted), ["db", "api", "web"]);
    }

    #[test]
    fn a_cycle_is_appended_after_everything_that_can_be_ordered() {
        let sorted = topo_sort(&[svc("a", &["b"]), svc("b", &["a"]), svc("c", &[])]);
        assert_eq!(names(&sorted), ["c", "a", "b"]);
    }
}
