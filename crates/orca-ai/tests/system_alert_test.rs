//! A system alert (#197) reaches every channel even when the LLM endpoint is
//! down: the case where the AI-opened path (`open_alert`) delivers nothing
//! (#181).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use orca_ai::backend::{ChatMessage, ChatResponse, LlmBackend};
use orca_ai::channels::{AlertEvent, Channel, Dispatcher};
use orca_ai::context::ClusterContext;
use orca_ai::conversation::ConversationEngine;
use orca_core::types::{AlertConversation, AlertSender, AlertSeverity};

/// An LLM endpoint that is down.
struct DownBackend;

#[async_trait]
impl LlmBackend for DownBackend {
    async fn chat(&self, _: &[ChatMessage]) -> anyhow::Result<ChatResponse> {
        anyhow::bail!("connection refused")
    }
    fn name(&self) -> &str {
        "down"
    }
}

/// Records every delivery.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<(String, String)>>>);

#[async_trait]
impl Channel for Recorder {
    fn name(&self) -> &'static str {
        "recorder"
    }
    async fn deliver(&self, conv: &AlertConversation, _: AlertEvent) -> anyhow::Result<()> {
        let text = conv
            .messages
            .first()
            .map(|m| m.content.clone())
            .unwrap_or_default();
        self.0.lock().unwrap().push((conv.service.clone(), text));
        Ok(())
    }
}

#[tokio::test]
async fn system_alert_is_delivered_with_the_llm_down() {
    let rec = Recorder::default();
    let mut engine = ConversationEngine::with_dispatcher(
        DownBackend,
        Dispatcher::new(vec![Box::new(rec.clone())]),
    );

    let conv = engine
        .open_system_alert(
            "backup:breakpilot-infra-vm1",
            AlertSeverity::Critical,
            "Backup FAILED (1): S3 upload of orca-gitea-db-data failed: 403",
        )
        .await;
    assert_eq!(conv.severity, AlertSeverity::Critical);
    assert_eq!(conv.messages[0].sender, AlertSender::System);

    let delivered = rec.0.lock().unwrap().clone();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].0, "backup:breakpilot-infra-vm1");
    assert!(delivered[0].1.contains("403"));
    assert_eq!(engine.active_conversations().len(), 1);
}

#[tokio::test]
async fn the_ai_path_now_degrades_instead_of_delivering_nothing() {
    // Before #181, `open_alert` errored with the model down and nothing was
    // dispatched. Now it opens a degraded alert carrying the raw signal.
    let rec = Recorder::default();
    let mut engine = ConversationEngine::with_dispatcher(
        DownBackend,
        Dispatcher::new(vec![Box::new(rec.clone())]),
    );
    let ctx = ClusterContext {
        cluster_name: "t".into(),
        nodes: vec![],
        services: vec![],
        recent_events: vec![],
        active_alerts: vec![],
    };
    let conv = engine
        .open_alert("x", AlertSeverity::Critical, "boom", &ctx)
        .await
        .expect("a model failure is no longer an error");
    assert!(
        conv.messages[1]
            .content
            .contains("AI diagnosis unavailable")
    );
    assert_eq!(rec.0.lock().unwrap().len(), 1);
}
