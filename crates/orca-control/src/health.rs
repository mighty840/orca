//! Background health checker for service instances.
//!
//! Respects `ProbeConfig` from liveness configuration (interval, timeout,
//! failure threshold). Falls back to defaults when no liveness probe is set.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};

use orca_core::config::ProbeConfig;
use orca_core::runtime::Runtime;
use orca_core::types::{HealthState, RuntimeKind, WorkloadStatus};

use crate::routes::service_config_to_spec;
use crate::state::AppState;

const DEFAULT_FAILURES: u32 = 3;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_INTERVAL: Duration = Duration::from_secs(10);

/// Spawn the health checker as a background tokio task.
pub fn spawn_health_checker(state: Arc<AppState>) {
    tokio::spawn(async move {
        let checker = HealthChecker::new(state);
        checker.run(DEFAULT_INTERVAL).await;
    });
}

/// Runs periodic health checks against service instances and restarts failed ones.
pub struct HealthChecker {
    state: Arc<AppState>,
    client: reqwest::Client,
}

impl HealthChecker {
    /// Create a new health checker.
    pub fn new(state: Arc<AppState>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .no_proxy()
            .build()
            .expect("failed to build reqwest client");
        Self { state, client }
    }

    /// Run the health check loop at the given interval.
    ///
    /// This is intended to be spawned as a background task.
    pub async fn run(&self, interval: Duration) {
        info!("Health checker started (interval: {}s)", interval.as_secs());
        let mut failure_counts: HashMap<String, u32> = HashMap::new();

        loop {
            self.check_all(&mut failure_counts).await;
            tokio::time::sleep(interval).await;
        }
    }

    /// Check all services and their instances.
    pub async fn check_all(&self, failure_counts: &mut HashMap<String, u32>) {
        let check_targets: Vec<CheckTarget> = {
            let services = self.state.services.read().await;
            services
                .values()
                .filter_map(|svc| {
                    // Use liveness probe path, fall back to health path.
                    let (health_path, probe) = if let Some(lp) = &svc.config.liveness {
                        (lp.path.clone(), Some(lp.clone()))
                    } else {
                        let path = svc.config.health.as_deref()?;
                        (path.to_string(), None)
                    };
                    let targets: Vec<InstanceTarget> = svc
                        .instances
                        .iter()
                        .enumerate()
                        .filter(|(_, inst)| inst.status == WorkloadStatus::Running)
                        .filter_map(|(idx, inst)| {
                            let port = inst.host_port?;
                            Some(InstanceTarget {
                                index: idx,
                                runtime_id: inst.handle.runtime_id.clone(),
                                host_port: port,
                            })
                        })
                        .collect();
                    if targets.is_empty() {
                        return None;
                    }
                    Some(CheckTarget {
                        service_name: svc.config.name.clone(),
                        health_path,
                        probe_config: probe,
                        runtime_kind: svc.config.runtime,
                        targets,
                    })
                })
                .collect()
        };

        for target in &check_targets {
            let threshold = target
                .probe_config
                .as_ref()
                .map_or(DEFAULT_FAILURES, |p| p.failure_threshold);
            let timeout = target
                .probe_config
                .as_ref()
                .map_or(DEFAULT_TIMEOUT, |p| Duration::from_secs(p.timeout_secs));

            for inst in &target.targets {
                let healthy = self
                    .probe_with_timeout(inst.host_port, &target.health_path, timeout)
                    .await;
                let count = failure_counts.entry(inst.runtime_id.clone()).or_insert(0);

                if healthy {
                    if *count > 0 {
                        info!(
                            runtime_id = %inst.runtime_id,
                            service = %target.service_name,
                            "Instance recovered, resetting failure count"
                        );
                    }
                    *count = 0;
                    self.set_health(&target.service_name, inst.index, HealthState::Healthy)
                        .await;
                } else {
                    *count += 1;
                    warn!(
                        runtime_id = %inst.runtime_id,
                        service = %target.service_name,
                        consecutive_failures = *count,
                        "Health check failed"
                    );
                    self.set_health(&target.service_name, inst.index, HealthState::Unhealthy)
                        .await;

                    if *count >= threshold {
                        info!(
                            runtime_id = %inst.runtime_id,
                            service = %target.service_name,
                            "Restarting after {} consecutive failures",
                            *count
                        );
                        self.restart_instance(
                            &target.service_name,
                            inst.index,
                            target.runtime_kind,
                        )
                        .await;
                        failure_counts.remove(&inst.runtime_id);
                    }
                }
            }
        }
    }

    /// Send an HTTP GET probe with a custom timeout.
    async fn probe_with_timeout(&self, port: u16, path: &str, timeout: Duration) -> bool {
        let url = format!("http://127.0.0.1:{port}{path}");
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .no_proxy()
            .build()
            .unwrap_or_else(|_| self.client.clone());
        match client.get(&url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        }
    }

    /// Update the health state of a specific instance.
    async fn set_health(&self, service_name: &str, index: usize, health: HealthState) {
        let mut services = self.state.services.write().await;
        if let Some(svc) = services.get_mut(service_name)
            && let Some(inst) = svc.instances.get_mut(index)
        {
            inst.health = health;
        }
    }

    /// Restart a failed instance by stopping/removing the old one and creating a new one.
    async fn restart_instance(&self, service_name: &str, index: usize, runtime_kind: RuntimeKind) {
        let runtime: &dyn Runtime = match runtime_kind {
            RuntimeKind::Container => self.state.container_runtime.as_ref(),
            RuntimeKind::Wasm => match &self.state.wasm_runtime {
                Some(r) => r.as_ref(),
                None => {
                    error!(service = %service_name, "Wasm runtime not available for restart");
                    return;
                }
            },
        };

        // Extract the old handle and config under a write lock, then drop it.
        let (old_handle, spec, port) = {
            let services = self.state.services.read().await;
            let Some(svc) = services.get(service_name) else {
                return;
            };
            let Some(inst) = svc.instances.get(index) else {
                return;
            };
            let spec = match service_config_to_spec(&svc.config) {
                Ok(s) => s,
                Err(e) => {
                    error!(service = %service_name, "Failed to build spec: {e}");
                    return;
                }
            };
            (inst.handle.clone(), spec, svc.config.port)
        };

        // Stop and remove the old container.
        if let Err(e) = runtime.stop(&old_handle, Duration::from_secs(10)).await {
            warn!(service = %service_name, "Failed to stop old instance: {e}");
        }
        if let Err(e) = runtime.remove(&old_handle).await {
            warn!(service = %service_name, "Failed to remove old instance: {e}");
        }

        // Create and start a new one.
        match runtime.create(&spec).await {
            Ok(new_handle) => {
                if let Err(e) = runtime.start(&new_handle).await {
                    error!(service = %service_name, "Failed to start new instance: {e}");
                    return;
                }
                let host_port = if let Some(p) = port {
                    runtime
                        .resolve_host_port(&new_handle, p)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                };

                let mut services = self.state.services.write().await;
                if let Some(svc) = services.get_mut(service_name)
                    && let Some(inst) = svc.instances.get_mut(index)
                {
                    inst.handle = new_handle;
                    inst.status = WorkloadStatus::Running;
                    inst.host_port = host_port;
                    inst.health = HealthState::Unknown;
                }
                info!(service = %service_name, "Instance restarted successfully");
            }
            Err(e) => {
                error!(service = %service_name, "Failed to create replacement instance: {e}");
            }
        }
    }
}

/// Internal struct for collecting check targets without holding locks.
struct CheckTarget {
    service_name: String,
    health_path: String,
    probe_config: Option<ProbeConfig>,
    runtime_kind: RuntimeKind,
    targets: Vec<InstanceTarget>,
}

/// Internal struct for a single instance to probe.
struct InstanceTarget {
    index: usize,
    runtime_id: String,
    host_port: u16,
}
