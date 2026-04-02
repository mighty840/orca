//! Background container stats collector.
//!
//! Periodically queries the runtime for resource usage of running containers
//! and caches the results in `AppState::container_stats`.

use std::sync::Arc;
use std::time::Duration;

use tracing::debug;

use orca_core::types::{RuntimeKind, WorkloadStatus};

use crate::state::AppState;

const STATS_INTERVAL: Duration = Duration::from_secs(30);

/// Cached stats for a service (aggregated across instances).
#[derive(Debug, Clone)]
pub struct ContainerStats {
    /// Human-readable memory usage (e.g. "128Mi").
    pub memory_usage: String,
    /// CPU usage percentage.
    pub cpu_percent: f64,
}

/// Spawn the stats collector as a background tokio task.
pub fn spawn_stats_collector(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(STATS_INTERVAL).await;
            collect_all_stats(&state).await;
        }
    });
}

/// Collect stats for all running container instances.
async fn collect_all_stats(state: &AppState) {
    // Snapshot the service handles under a read lock, then release it.
    let targets: Vec<(String, Vec<StatsTarget>)> = {
        let services = state.services.read().await;
        services
            .values()
            .filter_map(|svc| {
                if svc.config.runtime != RuntimeKind::Container {
                    return None;
                }
                let running: Vec<StatsTarget> = svc
                    .instances
                    .iter()
                    .filter(|i| i.status == WorkloadStatus::Running)
                    .map(|i| StatsTarget {
                        handle: i.handle.clone(),
                    })
                    .collect();
                if running.is_empty() {
                    return None;
                }
                Some((svc.config.name.clone(), running))
            })
            .collect()
    };

    let runtime = state.container_runtime.as_ref();
    let mut new_stats = std::collections::HashMap::new();

    for (name, instances) in &targets {
        let mut total_mem: u64 = 0;
        let mut total_cpu: f64 = 0.0;
        let mut count: u32 = 0;

        for target in instances {
            match runtime.stats(&target.handle).await {
                Ok(rs) => {
                    total_mem += rs.memory_bytes;
                    total_cpu += rs.cpu_percent;
                    count += 1;
                }
                Err(e) => {
                    debug!(service = %name, "Stats unavailable: {e}");
                }
            }
        }

        if count > 0 {
            new_stats.insert(
                name.clone(),
                ContainerStats {
                    memory_usage: format_bytes(total_mem),
                    cpu_percent: (total_cpu * 100.0).round() / 100.0,
                },
            );
        }
    }

    let mut cache = state.container_stats.write().await;
    *cache = new_stats;
}

/// Format bytes into a human-readable string (Ki/Mi/Gi).
fn format_bytes(bytes: u64) -> String {
    const GI: u64 = 1024 * 1024 * 1024;
    const MI: u64 = 1024 * 1024;
    const KI: u64 = 1024;

    if bytes >= GI {
        format!("{}Gi", bytes / GI)
    } else if bytes >= MI {
        format!("{}Mi", bytes / MI)
    } else if bytes >= KI {
        format!("{}Ki", bytes / KI)
    } else {
        format!("{bytes}B")
    }
}

/// Internal target for stats collection.
struct StatsTarget {
    handle: orca_core::runtime::WorkloadHandle,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bytes_gi() {
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2Gi");
    }

    #[test]
    fn format_bytes_mi() {
        assert_eq!(format_bytes(512 * 1024 * 1024), "512Mi");
    }

    #[test]
    fn format_bytes_ki() {
        assert_eq!(format_bytes(64 * 1024), "64Ki");
    }

    #[test]
    fn format_bytes_small() {
        assert_eq!(format_bytes(42), "42B");
    }
}
