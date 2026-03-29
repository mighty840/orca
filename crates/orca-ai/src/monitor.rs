use std::sync::Arc;
use std::time::Duration;

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
}

impl<B: LlmBackend> AiMonitor<B> {
    pub fn new(engine: Arc<RwLock<ConversationEngine<B>>>, analysis_interval_secs: u64) -> Self {
        Self {
            engine,
            analysis_interval: Duration::from_secs(analysis_interval_secs),
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
        let mut engine = self.engine.write().await;

        // Check each service for anomalies
        for svc in &ctx.services {
            // Service down / crash-looping
            if svc.replicas_running == 0 && svc.replicas_desired > 0 {
                let already_tracking = engine
                    .active_conversations()
                    .iter()
                    .any(|c| c.service == svc.name);

                if !already_tracking {
                    info!(
                        "Opening alert conversation for {}: no running replicas",
                        svc.name
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
                    engine.mark_remediated(id, "Issue self-resolved — metrics returned to normal");
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
