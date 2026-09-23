//! #181: the monitor alerts even when the model it would ask is down or
//! hanging, never holds the engine lock across the model call, and one
//! failing alert doesn't skip the rest of the cycle.

use std::sync::Mutex as StdMutex;
use std::time::Duration;

use super::*;
use crate::backend::{ChatMessage, ChatResponse};
use crate::channels::{Channel, Dispatcher};
use crate::context::ServiceSummary;
use orca_core::types::AlertConversation;

/// A model endpoint that is down.
struct DownBackend;

#[async_trait::async_trait]
impl LlmBackend for DownBackend {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<ChatResponse> {
        anyhow::bail!("connection refused")
    }
    fn name(&self) -> &str {
        "down"
    }
}

/// A model endpoint that accepts the request and never answers.
struct HangBackend;

#[async_trait::async_trait]
impl LlmBackend for HangBackend {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<ChatResponse> {
        std::future::pending().await
    }
    fn name(&self) -> &str {
        "hang"
    }
}

/// Records every delivered alert: (service, all message texts).
#[derive(Clone, Default)]
struct Recorder(Arc<StdMutex<Vec<(String, String)>>>);

#[async_trait::async_trait]
impl Channel for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    async fn deliver(&self, conv: &AlertConversation, _: AlertEvent) -> anyhow::Result<()> {
        let text = conv
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        self.0.lock().unwrap().push((conv.service.clone(), text));
        Ok(())
    }
}

fn down(name: &str) -> ServiceSummary {
    ServiceSummary {
        name: name.into(),
        runtime: "container".into(),
        replicas_running: 0,
        replicas_desired: 1,
        status: "down".into(),
        uses_gpu: false,
        recent_logs: Vec::new(),
        error_count_1h: 0,
        restart_count_24h: 0,
    }
}

fn ctx(services: Vec<ServiceSummary>) -> ClusterContext {
    ClusterContext {
        cluster_name: "t".into(),
        nodes: Vec::new(),
        services,
        recent_events: Vec::new(),
        active_alerts: Vec::new(),
    }
}

fn monitor<B: LlmBackend>(backend: B, rec: &Recorder) -> AiMonitor<B> {
    let engine =
        ConversationEngine::with_dispatcher(backend, Dispatcher::new(vec![Box::new(rec.clone())]));
    AiMonitor::new(Arc::new(RwLock::new(engine)), 60, 0)
}

#[tokio::test]
async fn a_down_model_still_opens_and_delivers_a_degraded_alert() {
    // The 2026 failure mode: litellm on the monitored master is one of the
    // unhealthy services, so the diagnosis call fails. Before, no email went out.
    let rec = Recorder::default();
    let mon = monitor(DownBackend, &rec);
    mon.analyze_cycle(&ctx(vec![down("litellm")]))
        .await
        .unwrap();

    let delivered = rec.0.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, "litellm");
    assert!(
        delivered[0].1.contains("0/1 replicas running"),
        "raw signal: {}",
        delivered[0].1
    );
    assert!(
        delivered[0]
            .1
            .contains("AI diagnosis unavailable (connection refused)")
    );
}

#[tokio::test]
async fn one_failing_diagnosis_does_not_skip_the_other_services() {
    // Before, `open_alert(...)?` aborted the cycle at the first error.
    let rec = Recorder::default();
    let mon = monitor(DownBackend, &rec);
    mon.analyze_cycle(&ctx(vec![down("litellm"), down("gitea"), down("keycloak")]))
        .await
        .unwrap();
    let mut services: Vec<String> = rec.0.lock().unwrap().iter().map(|d| d.0.clone()).collect();
    services.sort();
    assert_eq!(services, ["gitea", "keycloak", "litellm"]);
}

#[tokio::test]
async fn a_hanging_model_is_cut_off_and_never_blocks_readers() {
    let rec = Recorder::default();
    let mon = Arc::new(monitor(HangBackend, &rec).with_llm_timeout(Duration::from_millis(300)));
    let engine = Arc::clone(&mon.engine);

    let cycle = {
        let mon = Arc::clone(&mon);
        tokio::spawn(async move { mon.analyze_cycle(&ctx(vec![down("litellm")])).await })
    };

    // While the model call hangs, `orca alerts` / the TUI (readers of the
    // engine) must not wait on it. Before, the write lock was held throughout.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let read = tokio::time::timeout(Duration::from_millis(50), engine.read()).await;
    assert!(read.is_ok(), "engine lock held across the model call");
    drop(read);

    tokio::time::timeout(Duration::from_secs(5), cycle)
        .await
        .expect("cycle must finish once the diagnosis deadline passes")
        .unwrap()
        .unwrap();
    let delivered = rec.0.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert!(
        delivered[0].1.contains("no answer within"),
        "{}",
        delivered[0].1
    );
}

#[tokio::test]
async fn an_alert_is_not_duplicated_across_cycles() {
    let rec = Recorder::default();
    let mon = monitor(DownBackend, &rec);
    let c = ctx(vec![down("gitea")]);
    mon.analyze_cycle(&c).await.unwrap();
    mon.analyze_cycle(&c).await.unwrap();
    assert_eq!(rec.0.lock().unwrap().len(), 1);
}
