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

use tokio::sync::RwLock;
use tracing::info;

use orca_ai::backend::{LlmBackend, OpenAiCompatibleBackend};
use orca_ai::channels::Dispatcher;
use orca_ai::conversation::ConversationEngine;
use orca_ai::monitor::{AiMonitor, ContextProvider};
use orca_core::config::AiConfig;

pub use crate::alert_context::StateContextProvider;
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

/// Raise a critical alert for a failed backup (#197) through every configured
/// channel. It uses the deterministic path (no LLM call), so it's delivered
/// even when the model endpoint is down. Without `[ai.alerts]` there are no
/// channels, and the failure is only logged.
pub async fn alert_backup_failure(state: &AppState, node: &str, message: &str) {
    let Some(engine) = &state.alerts else {
        tracing::warn!("Backup failed on {node}, but no alert channels are configured: {message}");
        return;
    };
    engine
        .write()
        .await
        .open_system_alert(
            &format!("backup:{node}"),
            orca_core::types::AlertSeverity::Critical,
            &format!(
                "The scheduled backup on {node} failed. Until this is fixed, the \
                 affected data is not being backed up.\n\n{message}"
            ),
        )
        .await;
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
