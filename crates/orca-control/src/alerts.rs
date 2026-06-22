//! AI alert pipeline integration for the control plane.
//!
//! Builds a `ConversationEngine` from `cluster.toml` `[ai]`, implements
//! `ContextProvider` against `AppState`, and spawns the `AiMonitor` as
//! a background task. The engine is held in `Option<SharedAlertEngine>`
//! on `AppState` so handlers can mutate it for replies / dismiss / etc.
//!
//! Conversation persistence is in-memory only — a server restart wipes
//! active alerts. The TUI (PR3) re-fetches via the HTTP API on startup.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tokio::sync::RwLock;
use tracing::info;

use orca_ai::backend::{LlmBackend, OpenAiCompatibleBackend};
use orca_ai::channels::Dispatcher;
use orca_ai::context::{ClusterContext, NodeSummary, ServiceSummary};
use orca_ai::conversation::ConversationEngine;
use orca_ai::monitor::{AiMonitor, ContextProvider};
use orca_core::config::AiConfig;
use orca_core::types::{RuntimeKind, WorkloadStatus};

use crate::state::AppState;

pub type AlertEngine = ConversationEngine<Box<dyn LlmBackend>>;
pub type SharedAlertEngine = Arc<RwLock<AlertEngine>>;

/// Build the alert engine if `[ai]` is configured with endpoint + model.
/// Returns `None` when AI is unconfigured so the caller can degrade
/// gracefully without erroring out the whole server startup.
pub fn try_build_alert_engine(cfg: Option<&AiConfig>) -> Option<SharedAlertEngine> {
    let ai = cfg?;
    let endpoint = ai.endpoint.as_ref()?;
    let model = ai.model.as_ref()?;

    let backend: Box<dyn LlmBackend> = Box::new(OpenAiCompatibleBackend::new(
        endpoint.clone(),
        model.clone(),
        ai.api_key.clone(),
    ));

    let dispatcher = ai
        .alerts
        .as_ref()
        .and_then(|a| a.channels.as_ref())
        .map(Dispatcher::from_config)
        .unwrap_or_default();
    let names = dispatcher.channel_names();
    if !names.is_empty() {
        info!("Alert delivery channels configured: {names:?}");
    }

    Some(Arc::new(RwLock::new(ConversationEngine::with_dispatcher(
        backend, dispatcher,
    ))))
}

/// Spawn `AiMonitor::run` as a background task. No-op when alerts are
/// disabled or the engine isn't built.
pub fn spawn_alert_monitor(state: Arc<AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let engine = state.alerts.as_ref()?.clone();
    let ai = state.cluster_config.ai.as_ref()?;
    let alerts_cfg = ai.alerts.as_ref()?;
    if !alerts_cfg.enabled {
        return None;
    }
    let interval = alerts_cfg.analysis_interval_secs;
    let grace = alerts_cfg.alert_grace_secs;
    let provider: Arc<dyn ContextProvider> =
        Arc::new(StateContextProvider::for_state(state.clone()));
    info!("Spawning AI alert monitor (interval: {interval}s, down-grace: {grace}s)");
    Some(tokio::spawn(async move {
        let monitor = AiMonitor::new(engine, interval, grace);
        monitor.run(provider).await;
    }))
}

/// Reads `AppState` snapshots into a `ClusterContext` the AI can reason about.
/// Kept deliberately lean: services + nodes (CPU/mem) are the signal that
/// matters for the current monitor heuristics. Logs / error counts / GPU
/// summaries can be enriched later as the heuristics need them.
pub struct StateContextProvider {
    state: Arc<AppState>,
}

impl StateContextProvider {
    pub fn for_state(state: Arc<AppState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl ContextProvider for StateContextProvider {
    async fn snapshot(&self) -> anyhow::Result<ClusterContext> {
        let cluster_name = self.state.cluster_config.cluster.name.clone();

        let services = self.state.services.read().await;
        let events = self.state.instance_events.read().await;
        let now = Utc::now();
        let one_hour = Duration::hours(1);
        let day = Duration::hours(24);
        let services: Vec<ServiceSummary> = services
            .values()
            .map(|svc| {
                let running = svc
                    .instances
                    .iter()
                    .filter(|i| matches!(i.status, WorkloadStatus::Running))
                    .count() as u32;
                let status = if svc.instances.is_empty() {
                    "stopped".into()
                } else if running == svc.desired_replicas {
                    "healthy".into()
                } else {
                    "degraded".into()
                };
                let (errors_1h, restarts_24h) = events
                    .get(&svc.config.name)
                    .map(|log| (log.failures_in(now, one_hour), log.restarts_in(now, day)))
                    .unwrap_or((0, 0));
                ServiceSummary {
                    name: svc.config.name.clone(),
                    runtime: match svc.config.runtime {
                        RuntimeKind::Container => "container".into(),
                        RuntimeKind::Wasm => "wasm".into(),
                    },
                    replicas_running: running,
                    replicas_desired: svc.desired_replicas,
                    status,
                    uses_gpu: false,
                    recent_logs: Vec::new(),
                    error_count_1h: errors_1h,
                    restart_count_24h: restarts_24h,
                }
            })
            .collect();

        let nodes = self.state.registered_nodes.read().await;
        let nodes: Vec<NodeSummary> = nodes
            .values()
            .map(|n| NodeSummary {
                id: n.node_id.to_string(),
                address: n.address.clone(),
                status: if n.drain {
                    "draining".into()
                } else {
                    "healthy".into()
                },
                cpu_percent: n.cpu_percent,
                memory_percent: if n.memory_total > 0 {
                    (n.memory_bytes as f64 / n.memory_total as f64) * 100.0
                } else {
                    0.0
                },
                gpu_summary: Vec::new(),
            })
            .collect();

        Ok(ClusterContext {
            cluster_name,
            nodes,
            services,
            recent_events: Vec::new(),
            active_alerts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::config::ClusterConfig;

    #[test]
    fn try_build_returns_none_when_no_ai_config() {
        assert!(try_build_alert_engine(None).is_none());
    }

    #[test]
    fn try_build_returns_none_when_endpoint_missing() {
        let ai = AiConfig {
            provider: "ollama".into(),
            endpoint: None,
            model: Some("llama3".into()),
            api_key: None,
            alerts: None,
            auto_remediate: None,
        };
        assert!(try_build_alert_engine(Some(&ai)).is_none());
    }

    #[test]
    fn try_build_returns_engine_when_minimum_config_set() {
        let ai = AiConfig {
            provider: "ollama".into(),
            endpoint: Some("http://127.0.0.1:11434".into()),
            model: Some("llama3".into()),
            api_key: None,
            alerts: None,
            auto_remediate: None,
        };
        assert!(try_build_alert_engine(Some(&ai)).is_some());
    }

    // Sanity that ClusterConfig::default() is still constructible; we don't
    // wire spawn_alert_monitor here because it requires an Arc<AppState>
    // with a configured AI backend that we can't easily fake in a unit test.
    #[test]
    fn default_cluster_config_has_no_ai() {
        let cfg = ClusterConfig::default();
        assert!(cfg.ai.is_none());
        assert!(try_build_alert_engine(cfg.ai.as_ref()).is_none());
    }
}
