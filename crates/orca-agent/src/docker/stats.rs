//! Helper functions for extracting resource statistics from Docker stats.

use bollard::Docker;
use bollard::container::{Stats, StatsOptions};
use chrono::Utc;
use futures_util::StreamExt;

use orca_core::error::{OrcaError, Result};
use orca_core::types::ResourceStats;

/// Calculate CPU usage percentage from Docker stats.
pub(crate) fn calculate_cpu_percent(stats: &Stats) -> f64 {
    let cpu_delta = stats.cpu_stats.cpu_usage.total_usage as f64
        - stats.precpu_stats.cpu_usage.total_usage as f64;
    let system_delta = stats.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
        - stats.precpu_stats.system_cpu_usage.unwrap_or(0) as f64;
    let num_cpus = stats.cpu_stats.online_cpus.unwrap_or(1) as f64;

    if system_delta > 0.0 && cpu_delta >= 0.0 {
        (cpu_delta / system_delta) * num_cpus * 100.0
    } else {
        0.0
    }
}

/// Extract network RX/TX bytes from Docker stats.
pub(crate) fn extract_network_stats(stats: &Stats) -> (u64, u64) {
    stats
        .networks
        .as_ref()
        .map(|networks| {
            networks.values().fold((0u64, 0u64), |(rx, tx), iface| {
                (rx + iface.rx_bytes, tx + iface.tx_bytes)
            })
        })
        .unwrap_or((0, 0))
}

/// Collect resource stats for a running container.
pub(crate) async fn collect_stats(docker: &Docker, container_id: &str) -> Result<ResourceStats> {
    let opts = StatsOptions {
        stream: false,
        one_shot: true,
    };
    let mut stream = docker.stats(container_id, Some(opts));
    let stats = stream
        .next()
        .await
        .and_then(|r| r.ok())
        .ok_or_else(|| OrcaError::Runtime("no stats available".into()))?;
    let (rx, tx) = extract_network_stats(&stats);
    Ok(ResourceStats {
        cpu_percent: calculate_cpu_percent(&stats),
        memory_bytes: stats.memory_stats.usage.unwrap_or(0),
        network_rx_bytes: rx,
        network_tx_bytes: tx,
        gpu_stats: Vec::new(),
        timestamp: Utc::now(),
    })
}
