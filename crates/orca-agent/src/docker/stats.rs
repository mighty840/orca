//! Helper functions for extracting resource statistics from Docker stats.

use bollard::Docker;
use bollard::container::{Stats, StatsOptions};
use chrono::Utc;
use futures_util::StreamExt;

use orca_core::error::{OrcaError, Result};
use orca_core::types::ResourceStats;

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
///
/// Docker's one-shot stats API returns zeroed `precpu_stats`, which makes
/// the CPU-delta formula always resolve to ~0%. We instead take two samples
/// 500ms apart and compute the delta between them manually — same math
/// Docker's own CLI uses when you run `docker stats --no-stream`.
pub(crate) async fn collect_stats(docker: &Docker, container_id: &str) -> Result<ResourceStats> {
    let first = fetch_raw(docker, container_id).await?;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let second = fetch_raw(docker, container_id).await?;

    let cpu_delta = second.cpu_stats.cpu_usage.total_usage as f64
        - first.cpu_stats.cpu_usage.total_usage as f64;
    let system_delta = second.cpu_stats.system_cpu_usage.unwrap_or(0) as f64
        - first.cpu_stats.system_cpu_usage.unwrap_or(0) as f64;
    let num_cpus = second.cpu_stats.online_cpus.unwrap_or(1) as f64;
    let cpu_percent = if system_delta > 0.0 && cpu_delta >= 0.0 {
        (cpu_delta / system_delta) * num_cpus * 100.0
    } else {
        0.0
    };

    let (rx, tx) = extract_network_stats(&second);
    Ok(ResourceStats {
        cpu_percent,
        memory_bytes: second.memory_stats.usage.unwrap_or(0),
        network_rx_bytes: rx,
        network_tx_bytes: tx,
        gpu_stats: Vec::new(),
        timestamp: Utc::now(),
    })
}

async fn fetch_raw(docker: &Docker, container_id: &str) -> Result<Stats> {
    let opts = StatsOptions {
        stream: false,
        one_shot: true,
    };
    let mut stream = docker.stats(container_id, Some(opts));
    stream
        .next()
        .await
        .and_then(|r| r.ok())
        .ok_or_else(|| OrcaError::Runtime("no stats available".into()))
}
