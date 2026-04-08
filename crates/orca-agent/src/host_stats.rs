//! Host-level resource stats (CPU, memory, disk, network).
//!
//! Used by both the master and joined nodes to report themselves in the
//! cluster info endpoint. Maintained as a single long-lived [`sysinfo::System`]
//! so CPU % samples reflect the delta since the previous refresh.

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, Disks, MemoryRefreshKind, Networks, RefreshKind, System};

/// Snapshot of a single host's resource usage at a point in time.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HostStats {
    /// Global CPU utilization as a 0..100 percentage.
    pub cpu_percent: f64,
    /// Used memory in bytes (total - available).
    pub memory_bytes: u64,
    /// Total memory on the host in bytes. Used by clients to render the
    /// memory sparkline on a real 0..total scale instead of auto-scaling
    /// to the peak sample (which looks like a flat block when usage is
    /// stable).
    pub memory_total: u64,
    /// Used disk space across all mounted partitions, in bytes.
    pub disk_used: u64,
    /// Total disk capacity across all mounted partitions, in bytes.
    pub disk_total: u64,
    /// Cumulative bytes received on all network interfaces.
    pub net_rx: u64,
    /// Cumulative bytes transmitted on all network interfaces.
    pub net_tx: u64,
}

/// Stateful stats sampler. Keeps a `sysinfo::System` alive between calls so
/// CPU% represents the delta since the previous `sample()` rather than an
/// always-zero first-read.
pub struct HostStatsCollector {
    system: Mutex<System>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
}

impl HostStatsCollector {
    pub fn new() -> Self {
        let system = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::everything())
                .with_memory(MemoryRefreshKind::everything()),
        );
        let disks = Disks::new_with_refreshed_list();
        let networks = Networks::new_with_refreshed_list();
        Self {
            system: Mutex::new(system),
            disks: Mutex::new(disks),
            networks: Mutex::new(networks),
        }
    }

    /// Take a fresh sample. Safe to call frequently; the underlying sysinfo
    /// crate handles its own `/proc` polling.
    pub fn sample(&self) -> HostStats {
        let mut sys = self.system.lock().expect("system mutex");
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        let cpu_percent = sys.global_cpu_usage() as f64;
        let memory_bytes = sys.used_memory();
        let memory_total = sys.total_memory();
        drop(sys);

        let mut disks = self.disks.lock().expect("disks mutex");
        disks.refresh(true);
        let (disk_used, disk_total): (u64, u64) =
            disks.list().iter().fold((0, 0), |(used, total), d| {
                let t = d.total_space();
                let u = t.saturating_sub(d.available_space());
                (used + u, total + t)
            });
        drop(disks);

        let mut networks = self.networks.lock().expect("networks mutex");
        networks.refresh(true);
        let (net_rx, net_tx) = networks
            .iter()
            .fold((0u64, 0u64), |(rx, tx), (_name, data)| {
                (rx + data.total_received(), tx + data.total_transmitted())
            });
        drop(networks);

        HostStats {
            cpu_percent,
            memory_bytes,
            memory_total,
            disk_used,
            disk_total,
            net_rx,
            net_tx,
        }
    }
}

impl Default for HostStatsCollector {
    fn default() -> Self {
        Self::new()
    }
}
