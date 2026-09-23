use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::backend::{LLM_TIMEOUT, LlmBackend};
use crate::channels::AlertEvent;
use crate::context::ClusterContext;
use crate::conversation::ConversationEngine;
use crate::monitor_plan::plan_alerts;

/// The AI monitor runs as a background task. It periodically checks cluster health,
/// detects anomalies, and opens/updates conversational alerts.
///
/// Unlike traditional monitoring that fires static threshold alerts,
/// the AI monitor understands context:
/// - "CPU is 95% but this is a batch job that just started — normal"
/// - "CPU is 40% but latency tripled — something is wrong upstream"
/// - "This service has restarted 3 times in 10 minutes with OOM — needs more memory"
pub struct AiMonitor<B: LlmBackend> {
    engine: Arc<RwLock<ConversationEngine<B>>>,
    analysis_interval: Duration,
    /// A service must be observed down (0 running, >0 desired) for at least this
    /// long before a Critical "service down" alert opens. Without it, a deploy's
    /// brief 0-replica window opens + auto-remediates an alert every rollout.
    down_grace: Duration,
    /// First time each service was seen down in the current outage. Cleared when
    /// it recovers, so the grace clock restarts per outage. Interior-mutable so
    /// the monitor loop keeps a `&self` API.
    down_since: Mutex<HashMap<String, Instant>>,
    /// Deadline for one diagnosis; the alert goes out without it after this.
    llm_timeout: Duration,
}

impl<B: LlmBackend> AiMonitor<B> {
    pub fn new(
        engine: Arc<RwLock<ConversationEngine<B>>>,
        analysis_interval_secs: u64,
        alert_grace_secs: u64,
    ) -> Self {
        Self {
            engine,
            analysis_interval: Duration::from_secs(analysis_interval_secs),
            down_grace: Duration::from_secs(alert_grace_secs),
            down_since: Mutex::new(HashMap::new()),
            llm_timeout: LLM_TIMEOUT,
        }
    }

    /// Override the diagnosis deadline (tests use a short one).
    pub fn with_llm_timeout(mut self, timeout: Duration) -> Self {
        self.llm_timeout = timeout;
        self
    }

    /// Start the monitoring loop. Call this from the control plane as a background task.
    pub async fn run(&self, context_provider: Arc<dyn ContextProvider>) {
        info!(
            "AI monitor started (interval: {}s)",
            self.analysis_interval.as_secs()
        );

        loop {
            tokio::time::sleep(self.analysis_interval).await;

            match context_provider.snapshot().await {
                Ok(ctx) => {
                    if let Err(e) = self.analyze_cycle(&ctx).await {
                        warn!("AI monitor analysis failed: {e}");
                    }
                }
                Err(e) => {
                    warn!("AI monitor failed to get cluster context: {e}");
                }
            }
        }
    }

    /// One monitoring pass (#181). The engine lock is only held for brief,
    /// I/O-free steps (plan, record). The model call and alert delivery run
    /// without it, so `orca alerts` and the TUI never wait on a slow model.
    /// Every alert is handled independently: one failure never skips the rest.
    async fn analyze_cycle(&self, ctx: &ClusterContext) -> anyhow::Result<()> {
        let now = Instant::now();

        // 1. Plan under a short read lock.
        let (requests, resolved, backend, dispatcher) = {
            let engine = self.engine.read().await;
            let active: HashSet<String> = engine
                .active_conversations()
                .iter()
                .map(|c| c.service.clone())
                .collect();
            let requests = {
                let mut down_since = self.down_since.lock().expect("down_since lock poisoned");
                plan_alerts(ctx, now, self.down_grace, &mut down_since, &active)
            };
            let resolved: Vec<_> = engine
                .active_conversations()
                .iter()
                .filter(|conv| {
                    ctx.services.iter().any(|svc| {
                        svc.name == conv.service
                            && svc.replicas_running == svc.replicas_desired
                            && svc.error_count_1h == 0
                            && svc.restart_count_24h < 3
                    })
                })
                .map(|c| c.id)
                .collect();
            (requests, resolved, engine.backend(), engine.dispatcher())
        };

        for req in requests {
            info!(
                "Opening alert conversation for {}: {:?}",
                req.service, req.severity
            );
            // 2. Ask the model, with no lock held and a hard deadline.
            let prompt = ConversationEngine::<B>::open_prompt(&req.service, &req.trigger, ctx);
            let diagnosis =
                match tokio::time::timeout(self.llm_timeout, backend.chat(&prompt)).await {
                    Ok(Ok(r)) => Ok(r.content),
                    Ok(Err(e)) => Err(format!("{e:#}")),
                    Err(_) => Err(format!("no answer within {:?}", self.llm_timeout)),
                };
            if let Err(reason) = &diagnosis {
                warn!(
                    "AI diagnosis for {} unavailable, alerting without it: {reason}",
                    req.service
                );
            }
            // 3. Record under a brief write lock; 4. deliver without it.
            let snapshot = self.engine.write().await.record_open(
                &req.service,
                req.severity,
                &req.trigger,
                diagnosis,
            );
            if let Some(conv) = snapshot {
                dispatcher.dispatch(&conv, AlertEvent::Opened).await;
            }
        }

        for id in resolved {
            let snapshot = self
                .engine
                .write()
                .await
                .record_remediated(id, "Issue self-resolved — metrics returned to normal");
            if let Some(conv) = snapshot {
                dispatcher.dispatch(&conv, AlertEvent::Remediated).await;
            }
        }
        Ok(())
    }
}

/// Provides cluster context snapshots to the monitor.
/// Implemented by the control plane to feed real data.
#[async_trait::async_trait]
pub trait ContextProvider: Send + Sync + 'static {
    async fn snapshot(&self) -> anyhow::Result<ClusterContext>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{ChatMessage, ChatResponse, LlmBackend};
    use crate::context::ServiceSummary;
    use crate::conversation::ConversationEngine;

    /// Canned backend so `open_alert` doesn't hit the network.
    struct StubBackend;

    #[async_trait::async_trait]
    impl LlmBackend for StubBackend {
        async fn chat(&self, _messages: &[ChatMessage]) -> anyhow::Result<ChatResponse> {
            Ok(ChatResponse {
                content: "Service appears down. Fix: `orca redeploy api`".to_string(),
                tokens_used: None,
            })
        }
        fn name(&self) -> &str {
            "stub"
        }
    }

    fn ctx_with_down_service() -> ClusterContext {
        ClusterContext {
            cluster_name: "test".into(),
            nodes: Vec::new(),
            services: vec![ServiceSummary {
                name: "api".into(),
                runtime: "container".into(),
                replicas_running: 0,
                replicas_desired: 2,
                status: "degraded".into(),
                uses_gpu: false,
                recent_logs: Vec::new(),
                error_count_1h: 0,
                restart_count_24h: 0,
            }],
            recent_events: Vec::new(),
            active_alerts: Vec::new(),
        }
    }

    fn monitor(grace_secs: u64) -> AiMonitor<StubBackend> {
        let engine = Arc::new(RwLock::new(ConversationEngine::new(StubBackend)));
        AiMonitor::new(engine, 60, grace_secs)
    }

    #[tokio::test]
    async fn down_within_grace_does_not_open_alert() {
        // A webhook deploy briefly drops replicas to 0. With a grace window the
        // monitor must NOT page on that transient blip — this is the regression
        // that was spamming open+remediate emails on every deploy.
        let mon = monitor(120);
        let ctx = ctx_with_down_service();
        mon.analyze_cycle(&ctx).await.unwrap();
        mon.analyze_cycle(&ctx).await.unwrap();
        assert!(
            mon.engine.read().await.active_conversations().is_empty(),
            "a service down only within the deploy grace window must not open an alert"
        );
    }

    #[tokio::test]
    async fn down_past_grace_opens_single_alert() {
        // grace=0 → a genuine outage still pages, exactly once.
        let mon = monitor(0);
        let ctx = ctx_with_down_service();
        mon.analyze_cycle(&ctx).await.unwrap();
        mon.analyze_cycle(&ctx).await.unwrap();
        assert_eq!(
            mon.engine.read().await.active_conversations().len(),
            1,
            "a sustained outage must open exactly one alert (not one per cycle)"
        );
    }

    #[tokio::test]
    async fn recovery_resets_grace_clock() {
        // Down past grace opens an alert; once healthy the timer is cleared so a
        // later outage is measured from its own start, not the first one.
        let mon = monitor(0);
        let mut ctx = ctx_with_down_service();
        mon.analyze_cycle(&ctx).await.unwrap();
        assert_eq!(mon.engine.read().await.active_conversations().len(), 1);

        // Service recovers — the grace timer for "api" must be dropped.
        ctx.services[0].replicas_running = 2;
        mon.analyze_cycle(&ctx).await.unwrap();
        assert!(
            mon.down_since.lock().unwrap().is_empty(),
            "recovery must clear the per-service down timer"
        );
    }
}

#[cfg(test)]
#[path = "monitor_resilience_tests.rs"]
mod resilience_tests;
