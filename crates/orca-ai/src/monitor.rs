use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::backend::LlmBackend;
use crate::context::ClusterContext;
use crate::conversation::ConversationEngine;
use orca_core::types::AlertSeverity;

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
        }
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

    async fn analyze_cycle(&self, ctx: &ClusterContext) -> anyhow::Result<()> {
        let now = Instant::now();
        let mut engine = self.engine.write().await;

        // Check each service for anomalies
        for svc in &ctx.services {
            // Service down — but debounce transient deploy/rollout blips. A
            // webhook deploy briefly drops replicas to 0; we only page once the
            // outage has outlasted `down_grace`, so a normal rollout never opens
            // (and then auto-remediates) an alert.
            if svc.replicas_running == 0 && svc.replicas_desired > 0 {
                let down_for = {
                    let mut down_since = self.down_since.lock().expect("down_since lock poisoned");
                    let since = down_since.entry(svc.name.clone()).or_insert(now);
                    now.saturating_duration_since(*since)
                };

                if down_for < self.down_grace {
                    info!(
                        "Service '{}' has 0/{} replicas but only down {}s (< {}s grace) — deferring alert (likely a deploy)",
                        svc.name,
                        svc.replicas_desired,
                        down_for.as_secs(),
                        self.down_grace.as_secs()
                    );
                } else {
                    let already_tracking = engine
                        .active_conversations()
                        .iter()
                        .any(|c| c.service == svc.name);

                    if !already_tracking {
                        info!(
                            "Opening alert conversation for {}: no running replicas for {}s",
                            svc.name,
                            down_for.as_secs()
                        );
                        engine
                            .open_alert(
                                &svc.name,
                                AlertSeverity::Critical,
                                &format!(
                                    "Service '{}' has 0/{} replicas running. Restarts in 24h: {}. Recent errors: {}",
                                    svc.name, svc.replicas_desired, svc.restart_count_24h, svc.error_count_1h
                                ),
                                ctx,
                            )
                            .await?;
                    }
                }
            } else {
                // Recovered or scaled up — reset the grace clock so the next
                // outage measures from its own start, not a stale timestamp.
                self.down_since
                    .lock()
                    .expect("down_since lock poisoned")
                    .remove(&svc.name);
            }

            // High restart count (crash-looping)
            if svc.restart_count_24h > 10 && svc.replicas_running > 0 {
                let already_tracking = engine
                    .active_conversations()
                    .iter()
                    .any(|c| c.service == svc.name);

                if !already_tracking {
                    info!("Opening alert conversation for {}: crash-looping", svc.name);
                    engine
                        .open_alert(
                            &svc.name,
                            AlertSeverity::Warning,
                            &format!(
                                "Service '{}' has restarted {} times in the last 24 hours. \
                                 Currently {}/{} replicas are running.",
                                svc.name,
                                svc.restart_count_24h,
                                svc.replicas_running,
                                svc.replicas_desired
                            ),
                            ctx,
                        )
                        .await?;
                }
            }

            // High error rate
            if svc.error_count_1h > 100 {
                let already_tracking = engine
                    .active_conversations()
                    .iter()
                    .any(|c| c.service == svc.name);

                if !already_tracking {
                    info!(
                        "Opening alert conversation for {}: high error rate",
                        svc.name
                    );
                    engine
                        .open_alert(
                            &svc.name,
                            AlertSeverity::Warning,
                            &format!(
                                "Service '{}' has {} errors in the last hour. Recent log lines:\n{}",
                                svc.name,
                                svc.error_count_1h,
                                svc.recent_logs.iter().take(5).cloned().collect::<Vec<_>>().join("\n")
                            ),
                            ctx,
                        )
                        .await?;
                }
            }
        }

        // Drop grace timers for services no longer in the cluster (pruned while
        // down) so the map can't grow unbounded.
        {
            let live: std::collections::HashSet<&str> =
                ctx.services.iter().map(|s| s.name.as_str()).collect();
            self.down_since
                .lock()
                .expect("down_since lock poisoned")
                .retain(|name, _| live.contains(name.as_str()));
        }

        // Check nodes for GPU issues
        for node in &ctx.nodes {
            for gpu in &node.gpu_summary {
                if let Some(temp) = gpu.temperature
                    && temp > 90.0
                {
                    let alert_name = format!("node-{}-gpu-{}", node.id, gpu.index);
                    let already_tracking = engine
                        .active_conversations()
                        .iter()
                        .any(|c| c.service == alert_name);

                    if !already_tracking {
                        info!("Opening alert conversation for GPU thermal: {alert_name}");
                        engine
                            .open_alert(
                                &alert_name,
                                AlertSeverity::Warning,
                                &format!(
                                    "GPU {} on node {} ({}) temperature is {:.0}C (>90C threshold). \
                                     Utilization: {:.0}%, VRAM: {}/{}MB",
                                    gpu.index, node.id, gpu.model, temp,
                                    gpu.utilization, gpu.vram_used_mb, gpu.vram_total_mb
                                ),
                                ctx,
                            )
                            .await?;
                    }
                }

                // GPU VRAM nearly full
                if gpu.vram_total_mb > 0 {
                    let usage_pct = (gpu.vram_used_mb as f64 / gpu.vram_total_mb as f64) * 100.0;
                    if usage_pct > 95.0 {
                        let alert_name = format!("node-{}-gpu-{}-vram", node.id, gpu.index);
                        let already_tracking = engine
                            .active_conversations()
                            .iter()
                            .any(|c| c.service == alert_name);

                        if !already_tracking {
                            engine
                                .open_alert(
                                    &alert_name,
                                    AlertSeverity::Warning,
                                    &format!(
                                        "GPU {} on node {} VRAM is {:.0}% full ({}/{}MB). \
                                         Workloads may OOM.",
                                        gpu.index,
                                        node.id,
                                        usage_pct,
                                        gpu.vram_used_mb,
                                        gpu.vram_total_mb
                                    ),
                                    ctx,
                                )
                                .await?;
                        }
                    }
                }
            }
        }

        // Update existing conversations with fresh context
        let active_ids: Vec<_> = engine.active_conversations().iter().map(|c| c.id).collect();

        for id in active_ids {
            // Check if the issue self-resolved
            if let Some(conv) = engine.get_conversation(id) {
                let svc_name = conv.service.clone();
                if let Some(svc) = ctx.services.iter().find(|s| s.name == svc_name)
                    && svc.replicas_running == svc.replicas_desired
                    && svc.error_count_1h == 0
                    && svc.restart_count_24h < 3
                {
                    engine
                        .mark_remediated(id, "Issue self-resolved — metrics returned to normal")
                        .await;
                }
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
