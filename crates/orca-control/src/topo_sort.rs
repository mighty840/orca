//! Topological sorting for service dependency ordering.

use std::collections::HashSet;

use orca_core::config::ServiceConfig;

/// Sort services topologically so dependencies are reconciled first.
///
/// Services with no unresolved dependencies are placed first. If circular
/// dependencies or missing deps are detected, remaining services are appended
/// to avoid deadlock.
pub fn topo_sort(configs: &[ServiceConfig]) -> Vec<ServiceConfig> {
    let mut ordered = Vec::new();
    let mut remaining: Vec<_> = configs.to_vec();
    let mut resolved: HashSet<String> = HashSet::new();

    while !remaining.is_empty() {
        let before = remaining.len();
        remaining.retain(|c| {
            if c.depends_on.iter().all(|d| resolved.contains(d)) {
                ordered.push(c.clone());
                resolved.insert(c.name.clone());
                false
            } else {
                true
            }
        });
        if remaining.len() == before {
            // Circular dependency or missing dep — append remaining to avoid hang
            ordered.append(&mut remaining);
        }
    }
    ordered
}
