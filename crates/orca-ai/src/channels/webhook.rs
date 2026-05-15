//! Generic webhook delivery — POSTs JSON `{event, conversation}` to a URL.
//!
//! No signature/auth out of the box (callers should run behind their own
//! auth proxy or accept the public path). A future iteration can add
//! HMAC-signed payloads — orca already has the helpers from #36.

use async_trait::async_trait;
use serde::Serialize;
use std::time::Duration;

use orca_core::types::AlertConversation;

use super::{AlertEvent, Channel};

pub struct WebhookChannel {
    url: String,
    client: reqwest::Client,
}

#[derive(Serialize)]
struct Payload<'a> {
    event: &'a str,
    conversation: &'a AlertConversation,
}

impl WebhookChannel {
    pub fn new(url: String) -> Self {
        Self {
            url,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(15))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl Channel for WebhookChannel {
    fn name(&self) -> &'static str {
        "webhook"
    }

    async fn deliver(&self, conv: &AlertConversation, event: AlertEvent) -> anyhow::Result<()> {
        let payload = Payload {
            event: event.label(),
            conversation: conv,
        };
        let resp = self.client.post(&self.url).json(&payload).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("webhook returned {status}: {body}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::{AlertSeverity, AlertState};

    fn fake_conv() -> AlertConversation {
        AlertConversation {
            id: uuid::Uuid::now_v7(),
            service: "svc".into(),
            severity: AlertSeverity::Warning,
            state: AlertState::Investigating,
            started_at: chrono::Utc::now(),
            resolved_at: None,
            messages: Vec::new(),
        }
    }

    #[test]
    fn payload_serializes_event_label_and_conversation() {
        let conv = fake_conv();
        let payload = Payload {
            event: AlertEvent::Opened.label(),
            conversation: &conv,
        };
        let json = serde_json::to_value(&payload).unwrap();
        assert_eq!(json["event"], "Opened");
        assert_eq!(json["conversation"]["service"], "svc");
    }
}
